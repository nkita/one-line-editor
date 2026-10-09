# lvt 設計書

EmEditor のように **高速・省メモリ・シンプル** な Windows 専用テキストエディタ。

## 1. 目標と前提

| 項目 | 目標 |
|---|---|
| 対象ファイル | 数百MB まで (GB 級は将来対応。§6 参照) |
| 起動 | 100ms 程度、単一 exe (ランタイム DLL 不要) |
| メモリ | 空の状態で十数MB、ファイルを開いた後はファイルサイズの約 1.1〜1.2 倍 |
| 対象 OS | Windows 10/11 x64 |

**実測値 (WSL 上、lvt-core 単体)**: 411MB / 300万行の UTF-8 ファイルの読み込みは 1.06 秒で、ピークメモリは 473MB でした。全文検索 (一致なし) は 0.19 秒です。

## 2. 技術選定

| 領域 | 採用 | 理由 |
|---|---|---|
| 言語 | Rust | 速度・安全性・保守性 |
| UI | Win32 API を直接使用 (`windows` crate 0.62) | GUI フレームワークを使わないので軽量で、IME とも素直に連携できる |
| 描画 | Direct2D + DirectWrite | ClearType、フォントフォールバック、カラー絵文字、高 DPI に対応 |
| テキストバッファ | `ropey` (B木 rope) | 挿入・削除・行⇔位置の変換が O(log n) |
| 文字コード | `encoding_rs` + `chardetng` | 高速な変換と自動判定 |
| 検索 | `regex` | 線形時間が保証される |

非同期ランタイムや GUI フレームワークのような重い依存は持ちません。

## 3. 構成

```
crates/
├─ lvt-core/   OS 非依存の中核 (Linux 上で cargo test できる)
│  ├─ buffer.rs     TextBuffer (rope のラッパー。内部の改行は LF)
│  ├─ selection.rs  Selection {anchor, head}
│  ├─ history.rs    Undo/Redo (トランザクション、連続入力の結合、保存位置の追跡)
│  ├─ document.rs   Document = バッファ + 選択 + 履歴 + ファイル情報 + ハイライト状態
│  ├─ editor.rs     EditCommand の実行 (移動・入力・削除・インデントなど)
│  ├─ command.rs    Command = Edit(EditCommand) | App(AppCommand)
│  ├─ keymap.rs     KeyChord / Keymap / InputMode トレイト / StandardMode
│  ├─ search.rs     ブロック単位の正規表現検索・全置換
│  ├─ encoding.rs   文字コード判定、ストリーミング読み込み、安全な保存
│  ├─ syntax/       Highlighter トレイト、行状態キャッシュ、Markdown ハイライタ
│  ├─ theme.rs      配色 (TokenKind → TextStyle)
│  └─ width.rs      表示桁 (全角=2)、単語境界の文字種
└─ lvt-app/    Windows フロントエンド (bin: lvt.exe)
   └─ src/win/
      ├─ mod.rs      ウィンドウ生成、wndproc、モーダルを伴う処理 (開く/保存/閉じる)
      ├─ app.rs      App 状態: タブ・入力・マウス・スクロール・検索・IME
      ├─ paint.rs    描画 (表示行のみレイアウト)
      ├─ render.rs   Direct2D/DirectWrite 資源と描画プリミティブ
      ├─ menu.rs     メニュー (Command に対応付け)
      ├─ prompt.rs   下部の入力欄 (検索・置換・行移動)
      ├─ dialogs.rs  ファイルダイアログ、メッセージボックス、クリップボード
      └─ input.rs    仮想キー → Key 変換
```

### データの流れ

```
WM_KEYDOWN / WM_CHAR (IME確定文字列を含む)
      │  input.rs: 仮想キー → KeyChord
      ▼
InputMode::on_key / on_text  ──→  Vec<Command>
      │                              │
      │  Command::Edit ──→ editor::execute(&mut Document)   (OS 非依存)
      │  Command::App  ──→ App::execute_app                 (ファイル/タブ/検索/表示)
      │                      └ モーダルが必要なら Deferred を返す → mod.rs が借用外で実行
      ▼
App::refresh → InvalidateRect → WM_PAINT → paint.rs
   (Document + Highlighter + Theme → 表示行だけ DirectWrite でレイアウトして描画)
```

### 設計上の要点

- **変更経路の一本化**: テキストの変更は必ず `Document::apply` / `replace_selection` / `undo` / `redo` を通ります。これにより、履歴・ハイライトキャッシュ・リビジョン番号が常に整合します。
- **再入対策**: `App` は `thread_local! RefCell` に置き、メッセージ処理ごとに借用します。モーダルダイアログ (ファイル選択・確認) は内部でメッセージループを回すため、`Deferred` として借用を解いてから実行します。
- **描画は見える範囲だけ**: 1 行ずつ `IDWriteTextLayout` を作り、行番号・選択範囲・検索一致・キャレットを描きます。1 行が極端に長い場合は、先頭 20,000 文字までを描画対象にします。
- **読み込みはストリーミング**: 先頭 1MB で文字コードを判定し、1MB ずつデコードと改行の正規化をしながら rope を構築します。ファイル全体の一時コピーを作らないので、ピークメモリは最終的な rope のサイズとほぼ同じです。
- **保存は安全に**: 一時ファイルに書き出してから rename します。指定の文字コードで表せない文字があれば、保存せずに確認します (UTF-8 に切り替えて保存することもできます)。

## 4. 拡張ポイント

| 将来機能 | 拡張点 | 実装の方針 |
|---|---|---|
| **Vim キーバインド** | `keymap::InputMode` トレイト | `VimMode { state: Normal/Insert/Visual, pending: Vec<KeyChord> }` を実装し、`on_key`/`on_text` が `Command` を返す。`name()` でモード名をステータスバーに表示し、`caret_style()` でブロックキャレットにする。`d2w` のような演算子とモーションの組み合わせは、`EditCommand` に `DeleteRange(Motion)` などを足して表現する。`:` コマンドラインには `prompt.rs` を流用し、`PromptKind::Command` を追加する。切り替えは `win/mod.rs` の `mode` 生成箇所 (将来は設定ファイル) で行う |
| **Markdown 用デザイン** | `syntax::Highlighter` + `theme::Theme` | 現在は色・太字・斜体・下線まで。見出しの文字を大きくしたりコードブロックに背景を付けたりするには、次の 2 段階で拡張する: ① `Highlighter` に `fn line_decoration(&self, line, state) -> LineDecoration { scale, background, left_border }` を追加 (既定実装は「装飾なし」)。② 描画とスクロールを「行番号 × 固定の行の高さ」から、各行の高さを累積したマップ (`LineHeights`: 行の高さが均一なら現在の計算、可変なら区間木) へ抽象化する。①を先に入れれば、②の前でも背景色や罫線は実現できる |
| 言語ごとのハイライト | `syntax::highlighter_for_path` | 言語ごとに `Highlighter` を実装して登録する。行をまたぐ状態は `LineState(u32)` で表す |
| キー設定・設定ファイル | `KeyChord::parse`, `Keymap::bind` | TOML (`%APPDATA%\lvt\config.toml` または exe と同じフォルダ) からキー割り当て、フォント、テーマ、タブ幅を読む。コマンドを名前で指定できるよう `Command::from_name` を追加する |
| コマンドパレット | すべての操作が `Command` を経由する | コマンド名の一覧からあいまい検索して実行する |
| テーマ | `Theme` | TOML から読み込む |
| GB 級ファイル | `TextBuffer` の内部 | mmap + ピースツリー + 疎な行インデックスに差し替える。公開 API (文字インデックス・行 API) は維持する |
| マルチカーソル | `Selection` | `Document` を `Vec<Selection>` を持つ形に拡張する |

## 5. v0.1 の既知の制限

- 折り返し表示はない (横スクロールのみ)
- 1 行の描画対象は先頭 20,000 文字まで (それ以降は表示されない)
- 改行コードが混在するファイルは、保存時に最も多い改行コードへ統一される
- 読み込みは同期処理 (数百MB で 1〜数秒。待機カーソルを表示する)
- IME は IMM32 で、変換ウィンドウの位置はキャレットに合わせる (TSF は将来)
- 検索は約 1MB のブロック単位で行うため、ブロックをまたぐ複数行マッチは見つからない
- ビジュアルスタイルのマニフェストを埋め込んでいないため、メッセージボックスが旧来の見た目になる

## 6. 今後のロードマップ案

1. 設定ファイル (TOML) とキー割り当てのカスタマイズ
2. 折り返し表示
3. Vim モード (`InputMode` 実装)
4. Markdown の行装飾 (`LineDecoration`) と可変の行の高さ
5. ファイルのバックグラウンド読み込みと進捗表示
6. GB 級ファイル対応 (mmap + ピースツリー)
