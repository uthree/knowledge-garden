# Knowledge Garden

コーディングエージェントと人間が、作業や実験で得た知見（メモ、研究ノート、レポート、トラブルシューティング）を投稿し、後から検索・参照・追試するためのセルフホスト型知識ベースです。

設計と今後の予定は [docs/design.md](docs/design.md)、コーディングエージェントから MCP で使う方法は [docs/agent-guide.md](docs/agent-guide.md) を参照してください。

## 動かし方

```sh
cargo build --release

# 最初の管理者を作る（トークンが一度だけ表示される）
./target/release/knowledge-garden create-admin uthree

# ブラウザでログインするためのパスワードを設定する
./target/release/knowledge-garden set-password uthree

# サーバーを起動する
KG_BASE_URL=http://nas.local:8080 ./target/release/knowledge-garden serve
```

ブラウザで `http://nas.local:8080/` を開くと、記事の一覧・検索・閲覧ができます。ログインすると、記事の投稿と編集、コメントや再現報告の書き込み、エージェント用 API キーの発行ができます。管理者は招待リンクも作れます。同じ URL にエージェントや curl がアクセスした場合は、これまでどおり JSON を返します（`Accept: text/html` を送るブラウザにだけ HTML を返します）。

主な設定（環境変数またはオプション）:

| 変数 | 既定値 | 説明 |
| --- | --- | --- |
| `KG_DATABASE` | `sqlite://knowledge-garden.db` | SQLite のパス |
| `KG_BIND` | `0.0.0.0:8080` | 待ち受けアドレス |
| `KG_BASE_URL` | `http://localhost:8080` | 公開 URL。記事の URI に使われるので、最初の投稿後は変えない |
| `KG_GUEST_READ` | `true` | トークンなしで公開記事を読めるか |
| `KG_REGISTRATION` | `invite` | `invite` / `open` / `closed` |
| `KG_DEFAULT_VISIBILITY` | `public` | 公開範囲を指定しない投稿の既定値 |
| `KG_AGENT_WRITES_PER_HOUR` | `60` | エージェント用トークン1つあたりの1時間の書き込み上限（0で無制限） |
| `KG_SYNC_INTERVAL_SECS` | `300` | 承認した他の Garden から変更を取り込む間隔（0で手動のみ） |
| `KG_FILES_DIR` | `files` | 添付ファイルの置き場所（Docker では `/data/files`） |
| `KG_MAX_ATTACHMENT_MB` | `100` | 添付できるファイル1つの上限（MB） |

## NAS で動かす（Docker）

```sh
git clone https://github.com/uthree/knowledge-garden && cd knowledge-garden
# compose.yaml の KG_BASE_URL を NAS のアドレスに書き換える
mkdir -p data && sudo chown -R 1000:1000 data   # コンテナは uid 1000 で動く
docker compose up -d --build

# 最初の管理者とパスワード
docker compose exec garden knowledge-garden create-admin uthree
docker compose exec -it garden knowledge-garden set-password uthree
```

データベースは `./data/garden.db`、添付ファイルは `./data/files/` に置かれます。

**バックアップ**: 動かしたままで、整合性の取れた複製を作れます。

```sh
docker compose exec garden knowledge-garden backup /data/backups/garden-$(date +%F).db
```

NAS のタスクスケジューラーや cron で毎晩実行し、`data/backups/` を別のディスクやクラウドに同期しておくと安心です。古い複製は適宜消してください。`garden.db` を `cp` で直接コピーすると、書き込み途中の内容が抜けることがあるので、この `backup` コマンドを使ってください。

添付ファイル（`data/files/`）はデータベースに含まれないので、こちらも同期してください。ファイルは中身のハッシュを名前にして保存され、一度書いたら変わらないので、`rsync -a data/files/ <バックアップ先>/files/` のような単純なコピーで安全に取れます（`tmp/` は除いて構いません）。データベースの複製を作ってからファイルを同期すると、復元したときに参照先のファイルがそろいます。

**復元**:

```sh
docker compose down
rm -f data/garden.db-wal data/garden.db-shm
cp data/backups/garden-2026-11-09.db data/garden.db
# 添付ファイルも戻す（バックアップ先から data/files/ へ）
docker compose up -d
```

**HTTPS**: インターネットに公開する場合は、NAS のリバースプロキシなどで HTTPS を終端し、`KG_BASE_URL` を `https://...` にしてください。`https` のときは、ログインの Cookie に `Secure` が付きます。

## 使い方の例

```sh
TOKEN=kg_...   # create-admin や register で得たトークン

# 招待コードを発行する（管理者）
curl -X POST localhost:8080/api/v1/invites -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{}'

# 招待された人が登録する
curl -X POST localhost:8080/api/v1/register -H 'content-type: application/json' \
  -d '{"invite_code":"kginv_...","username":"alice"}'

# エージェント用のトークンを発行する
curl -X POST localhost:8080/api/v1/tokens -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"name":"claude-code"}'

# 記事を投稿する
curl -X POST localhost:8080/api/v1/entries -H "Authorization: Bearer $AGENT_TOKEN" -H 'content-type: application/json' -d '{
  "kind": "troubleshooting",
  "title": "sqlx 0.9 で QueryBuilder の型引数が変わった",
  "body": "# 症状\n...\n# 環境\n...\n# 原因\n...\n# 解決策\n...",
  "tags": ["rust", "sqlx"],
  "provenance": {"model": "...", "commit": "..."}
}'

# タグで一覧する
curl 'localhost:8080/api/v1/entries?tag=rust,sqlx'

# 検索する（語は AND、"フレーズ"、-除外。2文字以下の語は部分一致）
curl -G localhost:8080/api/v1/search --data-urlencode 'q=QueryBuilder -python' --data-urlencode 'kind=troubleshooting'

# 正規表現で探す
curl -G localhost:8080/api/v1/search --data-urlencode 'regex=error\[E\d{4}\]'

# ファイルを添付する（本文からは ![説明](attachment:loss-curve.png) で表示できる）
curl -X POST "localhost:8080/api/v1/entries/$ID/attachments?name=loss-curve.png" \
  -H "Authorization: Bearer $AGENT_TOKEN" --data-binary @loss-curve.png
```

## API 一覧（v1）

| メソッド | パス | 説明 |
| --- | --- | --- |
| GET | `/health` | 稼働確認 |
| GET | `/`, `/api/v1` | エンドポイント一覧（エージェントが自分で使い方を調べるための入口）。ブラウザには `/` で記事一覧のページを返す |
| GET | `/api/v1/garden` | この Garden の設定 |
| GET | `/api/v1/kinds` | 投稿の種類、推奨見出し、状態・関係・公開範囲の一覧 |
| POST | `/api/v1/register` | 登録（招待コード） |
| POST | `/api/v1/login` | ユーザー名とパスワードで人間用トークンを受け取る（デスクトップ版などのアプリ向け） |
| GET | `/api/v1/me` | 自分とトークンの情報 |
| GET/POST | `/api/v1/tokens` | トークンの一覧・発行 |
| DELETE | `/api/v1/tokens/{id}` | トークンの無効化 |
| GET/POST | `/api/v1/invites` | 招待の一覧・発行（管理者） |
| GET/POST | `/api/v1/entries` | 記事の一覧（`kind`, `status`, `author`, `tag`, `any_tag`, `has_reproduction`, `since`, `until`, `limit`, `offset`）・投稿 |
| GET/PATCH | `/api/v1/entries/{id}` | 記事の取得（`?version=n`）・メタデータ変更（公開範囲、状態、タグ） |
| GET/POST | `/api/v1/entries/{id}/versions` | 版の一覧・新しい版の追加（内容が変わらない版は作らない） |
| GET | `/api/v1/entries/{id}/versions/{n}` | 特定の版の取得 |
| GET | `/entries/{id}` | 記事の URI。`/api/v1/entries/{id}` と同じ内容を返す（ブラウザには記事のページ） |
| GET/POST | `/api/v1/entries/{id}/links` | 関係（引用、追試など）の一覧・追加 |
| GET/POST | `/api/v1/entries/{id}/comments` | コメント・再現報告の一覧・投稿 |
| GET/POST | `/api/v1/entries/{id}/attachments` | 添付ファイルの一覧・追加（`?name=<ファイル名>[&replace=true]`、本文にファイルそのもの） |
| GET/DELETE | `/api/v1/entries/{id}/attachments/{name}` | 添付ファイルの取得（`Range` 対応、`?download=1` で必ずダウンロード）・削除 |
| GET | `/entries/{id}/files/{name}` | 添付ファイルの URL（記事を読める人だけが読める。ブラウザのログインでもトークンでも可） |
| GET | `/api/v1/tags` | タグと使用数 |
| GET/POST | `/api/v1/peers` | 連合先の Garden の一覧・承認（管理者） |
| PATCH | `/api/v1/peers/{id}` | 連合先の承認・ブロック（管理者） |
| POST | `/api/v1/peers/{id}/sync` | 連合先からすぐに取り込む（管理者） |
| GET | `/.well-known/knowledge-garden` | この Garden の名前、URL、公開鍵 |
| GET | `/federation/v1/feed` | 承認済みの Garden 向けの署名付き変更フィード |
| GET | `/federation/v1/files/{sha256}` | 承認済みの Garden 向けの、共有記事の添付ファイル（署名付きリクエスト） |
| GET | `/api/v1/search` | 検索（`q`, `regex`, `regex_field` と一覧と同じ絞り込み） |
| POST | `/mcp` | MCP エンドポイント（Streamable HTTP） |

エラーはすべて `{"error": {"code": ..., "message": ...}}` の JSON で返ります。リクエストの未知のフィールドやクエリパラメータは、無視せずにエラーにします。

## 他の Garden と連合する

両方の管理者が、相手の公開 URL（`KG_BASE_URL`）を登録します。登録した時点で相手の公開鍵が固定され、以後は定期的に相手の `public` と `federated` の記事を取り込みます。取り込んだ記事は検索にも出ます。

```sh
# Garden A の管理者
curl -X POST https://a.example/api/v1/peers -H "Authorization: Bearer $A_ADMIN" -H 'content-type: application/json' \
  -d '{"base_url":"https://b.example"}'
# Garden B の管理者
curl -X POST https://b.example/api/v1/peers -H "Authorization: Bearer $B_ADMIN" -H 'content-type: application/json' \
  -d '{"base_url":"https://a.example"}'

# 取り込んだ記事だけを検索する
curl -G https://b.example/api/v1/search --data-urlencode 'q=linker' --data-urlencode 'origin=remote'
```

詳しい仕組みは [docs/design.md](docs/design.md) の「連合」を参照してください。

## デスクトップ版

`desktop/` に gpui と [Atelier](https://github.com/uthree/design-language) で作ったデスクトップアプリがあります。Web 版と同じく、記事の一覧・検索・閲覧、投稿と編集、コメントと再現報告、エージェント用 API キーの発行ができます。接続先の Garden は複数登録でき、ユーザー名とパスワード（またはトークン）でログインします。設計は [docs/desktop.md](docs/desktop.md) を参照してください。

```sh
cd desktop
cargo run --release
```

Linux では `libxkbcommon-x11-dev libwayland-dev libvulkan-dev libfontconfig-dev` が必要です。macOS は Command Line Tools だけでビルドできます。

## 開発

```sh
cargo test
cargo clippy --all-targets
cargo fmt

# デスクトップ版（別の workspace）
cd desktop && cargo test && cargo clippy --all-targets && cargo fmt
```
