# 機能追加の手順書

よくある拡張の手順をまとめます。どの拡張も、[DESIGN.md §3 設計原則](../DESIGN.md#3-設計原則) に従ってください。

- [A. 編集コマンドを追加する](#a-編集コマンドを追加する)
- [B. アプリコマンドとメニュー項目を追加する](#b-アプリコマンドとメニュー項目を追加する)
- [C. キー割り当てを追加・変更する](#c-キー割り当てを追加変更する)
- [D. 言語のハイライトを追加する](#d-言語のハイライトを追加する)
- [E. Vim キーバインドを拡張する](#e-vim-キーバインドを拡張する)
- [F. Markdown 用の表示デザインを追加する](#f-markdown-用の表示デザインを追加する)
- [G. 設定ファイルを導入する](#g-設定ファイルを導入する)
- [H. 時間のかかる処理をバックグラウンドで行う](#h-時間のかかる処理をバックグラウンドで行う)
- [I. GB 級ファイルに対応する](#i-gb-級ファイルに対応する)

---

## A. 編集コマンドを追加する

例: 「行を上へ移動」(`MoveLineUp`)

1. `lvt-core/src/command.rs` の `EditCommand` にバリアントを追加する。
2. `lvt-core/src/editor.rs` の `execute` に分岐を追加し、実装する。
   - テキストは必ず `doc.apply(edits, MergeKind::None, |buf, _| 適用後の選択)` で変更する。
   - 複数箇所を変更しても 1 回の Undo で戻るよう、edits を 1 回の `apply` にまとめる。
3. `editor.rs` のテストに「実行結果」と「Undo で元に戻ること」を追加する。
4. キーを割り当てる (→ C)。必要ならメニューにも追加する (→ B)。

## B. アプリコマンドとメニュー項目を追加する

例: 「文字コードを自動判定して開き直す」

1. `command.rs` の `AppCommand` にバリアントを追加する。
2. `lvt-app/src/win/app.rs` の `App::execute_app` に処理を追加する。
   - その場で完了する処理は、ここで直接行う (最後に `refresh` が呼ばれる)。
   - **モーダルダイアログやメッセージボックスが必要な処理** は、`Deferred` にバリアントを追加してそれを返す。実処理は `win/mod.rs` の `run_deferred` に書く (frontend.md §2)。
3. `win/menu.rs` の `Menus::build` で `m.item(親, "表示名(&X)", コマンド, mode)?` を呼ぶ。ショートカットの表示は、キー割り当てから自動で付く。
4. チェック状態や有効/無効が状態によって変わる場合は、`Menus::update` に条件を追加する。

## C. キー割り当てを追加・変更する

`lvt-core/src/keymap.rs` の `Keymap::windows_default()` に `m.bind("Ctrl+Shift+K", コマンド)` を追加します。

- 表記は `KeyChord::parse` に従います: `Ctrl` / `Shift` / `Alt` + `A`〜`Z`, `0`〜`9`, 記号, `F1`〜`F24`, `Left`, `PageUp`, `Enter`, `Esc` など。
- **修飾なし (または Shift のみ) の文字キーは `on_key` に届きません** (WM_CHAR 側で処理されるため)。割り当てられるのは Ctrl/Alt 付きのキーか、文字を生まないキーだけです。
- 記号キーはキーボード配列で変わります (JIS 配列の `+` は `;` キー)。記号キーを使う場合は、US 配列と JIS 配列の両方を割り当ててください (例: `Ctrl+=` と `Ctrl+;`)。
- `keymap.rs` のテストで、割り当てが期待どおりに引けることを確認してください。

## D. 言語のハイライトを追加する

例: Rust

1. `lvt-core/src/syntax/rust.rs` を作り、`Highlighter` を実装する。
   ```rust
   pub struct RustHighlighter;
   const NORMAL: LineState = LineState(0);
   const IN_BLOCK_COMMENT: LineState = LineState(1);
   impl Highlighter for RustHighlighter {
       fn name(&self) -> &'static str { "Rust" }
       fn highlight_line(&self, line: &str, st: LineState, out: &mut Vec<StyleSpan>) -> LineState {
           // line を走査し、StyleSpan { range: バイト範囲, kind: TokenKind::Keyword } などを out に追加する。
           // 行末で /* */ が閉じていなければ IN_BLOCK_COMMENT を返す
       }
   }
   ```
2. `syntax/mod.rs` で `mod rust;` と宣言し、`highlighter_for_path` の拡張子判定に `"rs" => Box::new(RustHighlighter)` を追加する。
3. 新しい種類のトークンが必要なら、`TokenKind` に追加し、`COUNT` と `Theme::light()/dark()` の `tokens` 配列も更新する。
4. テストを追加する。行をまたぐ状態の受け渡しも確認すること (markdown.rs のテストが参考になる)。

注意: `highlight_line` は描画のたびに、表示している行の数だけ呼ばれます。正規表現を毎回コンパイルするなど、重い処理は避けてください。

## E. Vim キーバインドを拡張する

Vim キーバインドは `lvt-core/src/vim/` に実装済みです (構成と仕様は [core.md §14](core.md#14-vim--vim-キーバインド))。よくある拡張の手順は次のとおりです。

### モーションを追加する (例: `ge`)
1. `vim/mod.rs` の `VimMotion` にバリアントを追加する。
2. `vim/motion.rs` の `VimMotion::kind` で種類 (Exclusive / Inclusive / Linewise) を決め、`target` に移動先の計算を書く。
3. `vim/parse.rs` の `motion` (または `g` で始まるキーの分岐) にキーを追加する。演算子の後ろでも自動で使えるようになる。
4. `motion.rs` のテストに移動先を、`vim/tests.rs` に `d` との組み合わせを追加する。

### 演算子・テキストオブジェクトを追加する
- 演算子: `Operator` に追加し、`parse.rs` の演算子の判定 (2 回押しで行単位になる文字も) と、`exec.rs` の `apply_op` を実装する。
- テキストオブジェクト: `TextObject` に追加し、`parse.rs` の `text_object` と `motion.rs` の `object_range` を実装する。

### `:` コマンドを追加する (例: `:e ファイル名`)
1. `vim/ex.rs` の `ExCommand` にバリアントを追加し、`parse` で解析する (テストも追加)。
2. `lvt-app/src/win/app.rs` の `App::run_ex` で実行する。モーダルが必要なら `Deferred` を返す (`self.pending_deferred` に入れる)。

### 注意
- **テストは `vim/tests.rs` の `Harness` で書く**: `h.keys("ciwfoo<Esc>").check("期待するテキスト", キャレット位置)`。`<Esc>` `<CR>` `<BS>` `<Tab>` `<C-r>` などの特殊キーも書けます。
- `.` で繰り返したい変更は、`translate` / `translate_cmd` で `commit_change` を呼ぶ (挿入モードに入るものは `Esc` で確定される)。
- Ctrl キーのうち Vim が扱うのは `vim/mod.rs` の `key_token` に列挙したものだけです。それ以外は標準のキー割り当て (`Keymap::windows_default`) に渡ります。

## F. Markdown 用の表示デザインを追加する

見出しの文字を大きくする、コードブロックに背景を付ける、引用に縦線を付ける、といった「プレビューに近い」編集画面を目指します。2 段階で進めます。

### 段階 1: 行の装飾 (行の高さは変えない)

1. `syntax/mod.rs` に追加する。
   ```rust
   #[derive(Default, Clone, Copy)]
   pub struct LineDecoration {
       pub background: Option<TokenKind>,   // 行全体の背景 (例: コードブロック)
       pub left_border: Option<TokenKind>,  // 左の縦線 (例: 引用)
       pub scale: f32,                      // 文字の倍率 (段階 2 で使う。1.0 = 等倍)
   }
   trait Highlighter {
       fn line_decoration(&self, line: &str, state: LineState) -> LineDecoration { LineDecoration::default() }
   }
   ```
2. `Document` に `line_decoration(line, text)` を追加する (`highlight_line` と同じ方法で状態を求める)。
3. `Theme` に装飾用の色 (`code_block_bg`, `quote_border` など) を追加する。
4. `paint.rs` の本文の行ループで、背景と縦線を描く。

### 段階 2: 可変の行の高さ (見出しを大きくする)

現在は「y = text_top + (行 − scroll_line) × line_height」という固定の行の高さを前提にしています。これを抽象化します。

1. `lvt-app` に `LineMetrics` を新設する。
   - `height_of(line) -> f32`: 装飾の `scale` × 基準の行の高さ
   - `y_of(line)` / `line_at_y(y)`: 累積の高さによる変換。行の高さが均一な文書では現在の計算をそのまま使い、可変の文書だけ累積和の配列 (またはフェンウィック木) を持つ。編集のたびに、変わった行以降を更新する。
2. `app.rs` の `line_at_y`、`visible_lines`、`ensure_caret_visible`、`center_line`、スクロールバーの計算、`position_ime` を `LineMetrics` 経由に置き換える。
3. `paint.rs` で、行ごとに `scale` に応じたフォントサイズの書式を使う (`IDWriteTextLayout::SetFontSize` を範囲指定で使ってもよい)。
4. 縦スクロールの単位は「行」のままでよい (最初の表示行の位置を基準にする)。

数百MB の文書で全行の高さを求めることは避け、可変の行の高さはハイライタが装飾を返す文書 (Markdown など) に限定してください。

## G. 設定ファイルを導入する

1. `lvt-core/src/config.rs` を作る (`serde` + `toml` をワークスペースの依存に追加する)。
   ```toml
   # %APPDATA%\lvt\config.toml (exe と同じフォルダの lvt.toml を優先する)
   [editor]
   font = "Consolas"
   font_size = 15
   tab_width = 4
   insert_spaces = false
   theme = "light"
   input_mode = "standard"   # "vim" (現在は settings.toml に自動保存している。R1 で統合する)

   [keys]
   "Ctrl+Shift+K" = "edit.delete_line"
   "Ctrl+D" = "edit.duplicate_line"
   ```
2. コマンドを名前で指定できるよう、`Command::from_name(&str)` と `Command::name()` を用意する。対応表は 1 か所に定義し、テストで「すべてのコマンドが往復できること」を確認する。
3. `Keymap::windows_default()` で既定の割り当てを作ってから、設定の `[keys]` で上書きする。
4. `Renderer` のフォント名 (現在は定数 `EDITOR_FONT`) と `EditorConfig` を、設定から受け取るように変える。
5. 設定ファイルの構文エラーは起動を止めず、既定値で起動してステータスバーに警告を出す。

## H. 時間のかかる処理をバックグラウンドで行う

例: 大きなファイルの読み込み

1. `win/mod.rs` で `std::thread::spawn` を使い、`encoding::load_file` を実行する。
2. 完了したら、結果を `Box` にして `PostMessageW(hwnd, WM_APP + n, 0, Box::into_raw(..) as isize)` で UI スレッドへ送る。wndproc で `Box::from_raw` して受け取り、タブを追加する。
3. 進捗は `Arc<AtomicU64>` (読み込んだバイト数) で共有し、タイマーでステータスバーに表示する。キャンセルには `Arc<AtomicBool>` を使う。
4. **`Document` を複数のスレッドで共有しないこと。** ワーカーは完成した `Document` (または `DecodedText`) を渡すだけにします。

## I. GB 級ファイルに対応する

`TextBuffer` の内部を差し替えます。

- 元のファイルを mmap した読み取り専用領域と、追記専用の追加領域を用意し、その上にピースツリーを構築します。ノードにはバイト数と改行数を集計して持たせます。
- 行インデックスは、一定行ごとのチェックポイントを置く疎なものにし、バックグラウンドで構築します。
- 公開 API (文字インデックス・行 API・チャンクの走査) は維持します。現在 `rope()` を直接使っている `search.rs` と `syntax`、`encoding::encode_to` は、`TextBuffer` のチャンク走査 API に置き換えます。
- UTF-8 以外のファイルは、一時ファイルへ UTF-8 に変換してから mmap します。
- 外部の変更 (ファイルの切り詰め) によるクラッシュを防ぐため、共有モードと変更の監視を検討してください。
