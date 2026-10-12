#!/usr/bin/env bash
# GitHub Releases 用の zip を作る (WSL)。
#   使い方: scripts/package.sh
#   出力: dist/lvt-v<version>-windows-x64.zip (lvt.exe・LICENSE・README.md) と、その SHA256
# バージョンは Cargo.toml の [workspace.package] version。リリースの手順は docs/BUILD_AND_VERIFY.md §7。
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' Cargo.toml)
name="lvt-v${version}-windows-x64"
cargo build -p lvt-app --target x86_64-pc-windows-gnu --release

rm -rf "dist/$name" "dist/$name.zip"
mkdir -p "dist/$name"
cp target/x86_64-pc-windows-gnu/release/lvt.exe LICENSE README.md "dist/$name/"
# zip コマンドがなくても作れるように Python の zipfile を使う
python3 - "dist" "$name" <<'PY'
import os, sys, zipfile
root, name = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(os.path.join(root, name + ".zip"), "w", zipfile.ZIP_DEFLATED) as z:
    for f in sorted(os.listdir(os.path.join(root, name))):
        z.write(os.path.join(root, name, f), f"{name}/{f}")
PY
(cd dist && sha256sum "$name.zip" > "$name.zip.sha256")
echo "作成しました: dist/$name.zip"
cat "dist/$name.zip.sha256"
