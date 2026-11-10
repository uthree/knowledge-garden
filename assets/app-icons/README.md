# アプリアイコン

Knowledge Garden のロゴ（記号 **Kn**、緑）。[design-language](https://github.com/uthree/design-language)
の app icon 規約（元素セル風、`doc/app-icons.md`）に従い、同リポジトリの
`tools/app-icon/make_icons.py` で生成したもの。手で描き直さないこと。

| ファイル | 用途 |
| --- | --- |
| `knowledge-garden/icon.svg` | 標準のロゴ（256 px 以上で番号・版・名前を表示） |
| `knowledge-garden/icon-medium.svg`, `icon-small.svg` | 小さいサイズ向けの簡略版 |
| `knowledge-garden/icon.ico` | favicon / Windows 用 |
| `knowledge-garden/title-bar.png` | デスクトップ版のタイトルバー左上（`TitleBar::app_icon`、Windows・Linux のみ。小さい版を 64 px で） |
| `knowledge-garden/png/<size>.png` | 16〜1024 px の PNG（`apple-touch-icon` は 256/512 を流用可） |
| `knowledge-garden/favicon.ico` | Web UI の favicon（`icon.svg` をそのまま 16/32/48 px にラスタライズしたもの） |
| `knowledge-garden/symbolic.svg` | 単色のシンボル（トレイ・メニューバー等） |
| `knowledge-garden/icon.icns`, `icon-macos.svg` | macOS 用 |

## 再生成

```sh
git clone https://github.com/uthree/design-language
pip install -r design-language/tools/app-icon/requirements.txt
# Inter Display Bold / SemiBold を用意（Inter 4.1 の extras/otf/）
python3 design-language/tools/app-icon/make_icons.py \
  --apps assets/app-icons/apps.toml --out assets/app-icons --no-sheets --font-dir <inter-dir>
```

版を上げたときは `apps.toml` の `version` を更新して再生成する。

Web UI のファビコンとヘッダーには、Auris Studio のサイトと同じく簡略版ではなく `icon.svg`
（通し番号・版・名前入り）を使う。`favicon.ico` は再生成後に `icon.svg` を 16/32/48 px で
ラスタライズして作り直す（例: 各サイズの PNG を書き出して `magick 16.png 32.png 48.png favicon.ico`）。
