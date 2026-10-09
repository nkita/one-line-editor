# lvt

軽量・高速な Windows 用テキストエディタ (Rust + Win32 + Direct2D/DirectWrite)。

- 数百MB のファイルを 1 秒前後で開ける (ストリーミング読み込み。メモリはファイルサイズの約 1.15 倍)
- タブ、日本語 IME、文字コードの自動判定と変換 (UTF-8 / UTF-16 / Shift_JIS / EUC-JP)
- 正規表現による検索・置換、行番号への移動、Markdown のハイライト、ダークテーマ
- 単一 exe、ランタイム不要

ドキュメント:
- [設計書](docs/DESIGN.md): アーキテクチャと拡張ポイント (Vim キーバインド・Markdown デザインなど)
- [ビルドと動作検証の手順](docs/BUILD_AND_VERIFY.md)

## クイックスタート (WSL)

```bash
sudo apt install -y mingw-w64
rustup target add x86_64-pc-windows-gnu
cargo test -p lvt-core
scripts/deploy.sh          # %USERPROFILE%\lvt\lvt.exe にビルドしてコピー
```
