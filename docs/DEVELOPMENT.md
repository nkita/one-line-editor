# 開発ガイド

## 1. 開発環境

- WSL (Ubuntu) + Rust stable + `x86_64-pc-windows-gnu` ターゲット + `mingw-w64`
- セットアップとビルドの手順は [BUILD_AND_VERIFY.md](BUILD_AND_VERIFY.md) を参照してください。
- GUI の動作確認は Windows 実機で行います (`scripts/deploy.sh` → `%USERPROFILE%\lvt\lvt.exe`)。

よく使うコマンド:

| 目的 | コマンド |
|---|---|
| コアの単体テスト | `cargo test -p lvt-core` |
| lint (Windows 向けのコードも含む) | `cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu` |
| lint (Linux) | `cargo clippy --workspace --all-targets` |
| フォーマット | `cargo fmt` |
| Windows 向けビルド | `cargo build-win` (デバッグ版。コンソール付き) / `cargo build-win-release` |
| Windows へ配置 | `scripts/deploy.sh` |
| 性能計測 | `cargo run -p lvt-core --release --example load_bench -- <file> [pattern]` |
| テストデータ生成 | `python3 scripts/gen_testdata.py <出力先> [MB]` |

## 2. コードの置き場所の判断

```
その処理は Windows API を使うか?
 ├─ いいえ → lvt-core (単体テストを必ず書く)
 └─ はい   → lvt-app/src/win/
              ├─ モーダル (ダイアログ・メッセージボックス) を伴う → Deferred + win/mod.rs の run_deferred
              ├─ 描画       → paint.rs (描画プリミティブが足りなければ render.rs)
              └─ それ以外   → app.rs
```

## 3. コーディング規約

- コメントと UI の文言は **日本語** で書きます。識別子は英語です。
- 各ファイルの先頭に `//!` で「このモジュールの責務」を書きます。
- 公開する型と関数には、`///` で「何を表すか・前提条件」を書きます。実装の説明が自明な箇所にはコメントを書きません。
- `unwrap` は不変条件で保証される箇所に限ります。ユーザーの入力やファイル I/O の失敗はエラーとして扱い、UI では、ダイアログかステータスバーで知らせます。
- `unsafe` は Windows API の呼び出しに限り、ブロックを小さく保ちます。
- **lint の警告ゼロ** を保ちます (clippy は両ターゲットで実行)。
- 依存クレートを追加するときは、ワークスペースの `[workspace.dependencies]` で版を一元管理します。

## 4. 完了の定義 (Definition of Done)

1. `cargo test -p lvt-core` が成功する。新しいロジックには単体テストがある。
2. `cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu` と `cargo clippy --workspace --all-targets` で警告がない。
3. `cargo build-win-release` が成功する。
4. UI に関わる変更は Windows 実機で確認し、必要なら [BUILD_AND_VERIFY.md](BUILD_AND_VERIFY.md) のチェックリストに項目を追加する。
5. 設計に関わる変更は `docs/` を更新する (拡張点を増やしたら [design/extending.md](design/extending.md) も更新する)。
6. 性能に影響しうる変更 (読み込み・検索・描画) は `load_bench` または 300MB のテストデータで、劣化がないことを確認する。

## 5. コミット

- 1 コミット 1 目的。メッセージの 1 行目は変更内容の要約にします (英語・日本語どちらでも可)。
- `target/` はコミットしません (`.gitignore` 済み)。`Cargo.lock` はコミットします (アプリケーションなので)。
