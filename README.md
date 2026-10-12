# lvt

軽量・高速な Windows 用テキストエディタ (Rust + Win32 + Direct2D/DirectWrite)。

- 数百MB のファイルを 1 秒前後で開ける (ストリーミング読み込み。メモリはファイルサイズの約 1.15 倍)
- タブ、日本語 IME、文字コードの自動判定と変換 (UTF-8 / UTF-16 / Shift_JIS / EUC-JP)
- 正規表現による検索・置換、行番号への移動、Markdown のハイライト、ダークテーマ
- .txt / .lvt の装飾表示: `*detail[…]` を隠して行番号の横のバッジで示す 1 行 TODO と詳細パネル (Vim で編集可)、`[label](url)`・`` `code` ``・`**強調**`・`# 見出し` の記号を隠す表示
- Vim キーバインド (既定でオン。表示メニュー / Ctrl+Alt+V で切り替え)、URL の自動リンク (Ctrl+クリック)、タブのドラッグでの並べ替え
- 単一 exe、ランタイム不要

ドキュメント:
- [設計書](docs/DESIGN.md): 要件・構成・設計原則・ロードマップ (詳細仕様は docs/design/)
- [機能追加の手順書](docs/design/extending.md): コマンド・キー・言語・Vim の拡張・Markdown デザイン・設定ファイル
- [開発ガイド](docs/DEVELOPMENT.md): 規約・テスト・完了の定義
- [ビルドと動作検証の手順](docs/BUILD_AND_VERIFY.md)

## インストール

1. [Releases](https://github.com/nkita/one-line-editor/releases) から `lvt-v<バージョン>-windows-x64.zip` をダウンロードして展開する
2. `lvt.exe` を任意の場所に置いて起動する (例: `%LOCALAPPDATA%\Programs\lvt\`)。インストーラーやランタイムは不要
   - 署名していないため、初回は「Windows によって PC が保護されました」と出ることがあります。「詳細情報」→「実行」で起動できます
3. 必要ならタスクバーにピン留めする (ジャンプリストに最近使ったファイルが出る)。.txt / .lvt は「プログラムから開く」で lvt.exe を選ぶ

設定・最近使ったファイル・前回のタブは `%APPDATA%\lvt\` に保存されます (exe の場所によらない)。アンインストールは exe と このフォルダを削除するだけです。

## クイックスタート (WSL)

```bash
sudo apt install -y mingw-w64
rustup target add x86_64-pc-windows-gnu
cargo test -p lvt-core
scripts/deploy.sh          # %USERPROFILE%\lvt\lvt.exe にビルドしてコピー
```
