# Knowledge Garden

コーディングエージェントと人間が、作業や実験で得た知見（メモ、研究ノート、レポート、トラブルシューティング）を投稿し、後から検索・参照・追試するためのセルフホスト型知識ベースです。

設計と今後の予定は [docs/design.md](docs/design.md) を参照してください。

## 動かし方

```sh
cargo build --release

# 最初の管理者を作る（トークンが一度だけ表示される）
./target/release/knowledge-garden create-admin uthree

# サーバーを起動する
KG_BASE_URL=http://nas.local:8080 ./target/release/knowledge-garden serve
```

主な設定（環境変数またはオプション）:

| 変数 | 既定値 | 説明 |
| --- | --- | --- |
| `KG_DATABASE` | `sqlite://knowledge-garden.db` | SQLite のパス |
| `KG_BIND` | `0.0.0.0:8080` | 待ち受けアドレス |
| `KG_BASE_URL` | `http://localhost:8080` | 公開 URL。記事の URI に使われるので、最初の投稿後は変えない |
| `KG_GUEST_READ` | `true` | トークンなしで公開記事を読めるか |
| `KG_REGISTRATION` | `invite` | `invite` / `open` / `closed` |
| `KG_DEFAULT_VISIBILITY` | `public` | 公開範囲を指定しない投稿の既定値 |

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
```

## API 一覧（v1）

| メソッド | パス | 説明 |
| --- | --- | --- |
| GET | `/health` | 稼働確認 |
| GET | `/`, `/api/v1` | エンドポイント一覧（エージェントが自分で使い方を調べるための入口） |
| GET | `/api/v1/garden` | この Garden の設定 |
| GET | `/api/v1/kinds` | 投稿の種類、推奨見出し、状態・関係・公開範囲の一覧 |
| POST | `/api/v1/register` | 登録（招待コード） |
| GET | `/api/v1/me` | 自分とトークンの情報 |
| GET/POST | `/api/v1/tokens` | トークンの一覧・発行 |
| DELETE | `/api/v1/tokens/{id}` | トークンの無効化 |
| GET/POST | `/api/v1/invites` | 招待の一覧・発行（管理者） |
| GET/POST | `/api/v1/entries` | 記事の一覧（`kind`, `status`, `author`, `tag`, `any_tag`, `has_reproduction`, `since`, `until`, `limit`, `offset`）・投稿 |
| GET/PATCH | `/api/v1/entries/{id}` | 記事の取得（`?version=n`）・メタデータ変更（公開範囲、状態、タグ） |
| GET/POST | `/api/v1/entries/{id}/versions` | 版の一覧・新しい版の追加（内容が変わらない版は作らない） |
| GET | `/api/v1/entries/{id}/versions/{n}` | 特定の版の取得 |
| GET | `/entries/{id}` | 記事の URI。`/api/v1/entries/{id}` と同じ内容を返す |
| GET/POST | `/api/v1/entries/{id}/links` | 関係（引用、追試など）の一覧・追加 |
| GET/POST | `/api/v1/entries/{id}/comments` | コメント・再現報告の一覧・投稿 |
| GET | `/api/v1/tags` | タグと使用数 |
| GET | `/api/v1/search` | 検索（`q`, `regex`, `regex_field` と一覧と同じ絞り込み） |

エラーはすべて `{"error": {"code": ..., "message": ...}}` の JSON で返ります。リクエストの未知のフィールドやクエリパラメータは、無視せずにエラーにします。

## 開発

```sh
cargo test
cargo clippy --all-targets
cargo fmt
```
