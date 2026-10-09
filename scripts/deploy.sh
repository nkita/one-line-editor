#!/usr/bin/env bash
# WSL でリリースビルドし、Windows 側のフォルダへ lvt.exe をコピーする。
#   使い方: scripts/deploy.sh [コピー先 (WSL パス)]
#   既定のコピー先: %USERPROFILE%\lvt  (例: /mnt/c/Users/<名前>/lvt)
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v x86_64-w64-mingw32-gcc >/dev/null; then
  echo "mingw-w64 が見つかりません。先に: sudo apt install -y mingw-w64" >&2
  exit 1
fi

cargo build -p lvt-app --target x86_64-pc-windows-gnu --release

if [[ $# -ge 1 ]]; then
  dest="$1"
else
  winprofile=$(cmd.exe /c "echo %USERPROFILE%" 2>/dev/null | tr -d '\r')
  dest="$(wslpath "$winprofile")/lvt"
fi
mkdir -p "$dest"
cp target/x86_64-pc-windows-gnu/release/lvt.exe "$dest/"
echo "コピーしました: $dest/lvt.exe ($(du -h "$dest/lvt.exe" | cut -f1))"
echo "Windows 側のパス: $(wslpath -w "$dest/lvt.exe")"
