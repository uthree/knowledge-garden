#!/bin/sh
# Knowledge Garden のエージェント用スキルを、Claude Code 以外のエージェントが読む場所にコピーする。
#
#   scripts/install-agent-skills.sh            # ~/.agents/skills（Codex と OpenCode が読む）
#   scripts/install-agent-skills.sh DIR        # 任意の場所（例: プロジェクトの .agents/skills）
#
# Claude Code ではプラグインとして入れるので、このスクリプトは不要。
set -eu

src="$(cd "$(dirname "$0")/.." && pwd)/plugins/knowledge-garden/skills"
dest="${1:-${HOME}/.agents/skills}"

mkdir -p "$dest"
for dir in "$src"/*/; do
    name="$(basename "$dir")"
    rm -rf "${dest:?}/$name"
    cp -R "$dir" "$dest/$name"
    echo "installed $name -> $dest/$name"
done
