# lvt-core 仕様

OS に依存しないエディタの中核です。**Windows API を使ってはいけません**。すべて Linux 上の `cargo test -p lvt-core` で検証できる状態を保ちます。

## 0. 共通の約束

| 項目 | 約束 |
|---|---|
| 位置 | 特に断りがない限り **文字インデックス** (Unicode スカラー値単位)。`usize` |
| 行 | 0 始まり。表示 (ステータスバー・行番号) だけ 1 始まり |
| 改行 | 内部では常に `\n` (LF)。CR / CRLF は読み込み時に正規化する |
| 範囲 | `Range<usize>` (半開区間) |
| 行内のバイト位置 | ハイライタの `StyleSpan` と検索の内部処理だけで使う。その行の `&str` に対する文字境界 |
| UTF-16 位置 | `lvt-core` では使わない (DirectWrite との境界で `lvt-app/src/win/util.rs` が変換する) |

## 1. buffer.rs — `TextBuffer`

ropey の `Rope` の薄いラッパーです。**バッファの実装を差し替えるときの境界** になります。外部からは `rope()` 以外で ropey に依存しないことが望ましいです (現在 `search.rs` と `syntax` がバイト単位の API のために `rope()` を使っています)。

| メソッド | 意味 |
|---|---|
| `len_chars / len_bytes / len_lines` | 文字数・バイト数・行数 (空でも 1 行。末尾が改行なら最後に空行が 1 つある) |
| `line(i)` | **改行を含まない** 行の内容 (`RopeSlice`) |
| `line_len(i) / line_end(i)` | 行の文字数 (改行を除く) と行末の位置 |
| `line_to_char / char_to_line / line_col` | 行 ⇔ 位置の変換 (範囲外の値は丸める) |
| `insert / remove` | **`Document` だけが呼ぶ** (原則 3) |

## 2. selection.rs — `Selection`

`anchor` は選択の起点、`head` はキャレットです。`anchor == head` のときは選択なし (キャレットのみ) です。

- `moved_to(pos, extend)`: `extend` が真なら anchor を保ち、偽ならキャレットにする。移動コマンドはすべてこれを使う。
- `clamp(len)`: 文書の長さに収める。

マルチカーソルに拡張する場合は、`Document` が `Vec<Selection>` を持つように変えます。

## 3. history.rs — `History`

```
Transaction { id, edits: Vec<Edit>, before: Selection, after: Selection }
Edit        { pos, deleted: String, inserted: String }   // pos で deleted を消して inserted を入れた
```

- **記録**: `record(edits, before, after, merge)`。`merge` が `Typing` / `Deleting` で、かつ直前の記録と同じ種類なら、直前のトランザクションに追加します。
- **結合の解除**: `break_merge()`。キャレット移動 (`Document::set_selection` で選択が変わったとき)、保存、Undo/Redo のときに呼ばれます。
- **Undo**: 記録された edits を逆順にたどり、`inserted` を消して `deleted` を戻します。**Redo** は正順で再適用します。
- **まとめ (グループ)**: `begin_group()` から `end_group()` までの記録は、種類に関係なく 1 つのトランザクションにまとめます (Vim の挿入モード 1 回分や `cw` を 1 回の Undo で戻すため)。Undo/Redo をするとまとめは終わります。保存すると、以後の変更は新しいトランザクションに記録します (保存済みの状態と混ざらないように)。
- **変更判定**: 保存時点の「undo スタックの先頭の id」を `saved_id` として覚え、現在の先頭の id と比べます。保存時点が redo 側にある状態で新しい編集をすると、保存時点には戻れなくなるので、`saved_unreachable` を立てます。

不変条件: Undo してから Redo すると、テキストと選択範囲が完全に元に戻ること。

## 4. document.rs — `Document`

文書 1 つ (タブ 1 つ) の状態です。

```
Document {
  buffer: TextBuffer, selection: Selection, preferred_col: Option<usize>, visual: Option<vim::Visual>,
  history: History, path: Option<PathBuf>, encoding, line_ending, meta_modified,
  highlighter: Box<dyn Highlighter>, highlight_cache: HighlightCache, revision: u64
}
```

| API | 仕様 |
|---|---|
| `open(path, forced_encoding)` | 読み込む。戻り値の bool は「不正なバイト列を U+FFFD に置き換えたか」 |
| `save()` / `save_as(path)` | 保存する。成功すると履歴に保存点を記録する。パスが変わったらハイライタを選び直す |
| `apply(edits, merge, after)` | **唯一の汎用変更 API**。互いに重ならない複数の置換を、1 回の操作として適用する。順序は問わない (内部で後ろから適用するので前方の位置がずれない)。`after(&buffer, &records)` で適用後の選択を決める |
| `replace_selection(text, merge)` | 選択範囲を置き換え、キャレットを挿入文字列の後ろに置く |
| `set_selection(sel)` | 選択を変える。変わった場合は Undo の結合を解除する |
| `undo()` / `redo()` | 戻り値は「実行したか」 |
| `begin_undo_group()` / `end_undo_group()` | 間の変更を 1 回の Undo で戻せるようにする (`History` のまとめ) |
| `set_detail_view(on)` / `detail_at_line(line)` | 装飾表示と詳細 (`*detail[…]`) の折りたたみ (§17, §18)。有効なとき、`set_selection` / `apply` / `undo` / `redo` は、キャレットを詳細の内側に置かず、詳細に一部だけかかる編集を詳細全体に広げる |
| `delete_detail_on_blank_line(line)` | 行が詳細と空白だけなら詳細を削除して空行を残す。BS / Del / Vim の `x` `X` がキャレット行に対して先に試す (行の結合より先に詳細を消す) |
| `delete_detail(line)` | 行の詳細を削除する (1 回の Undo)。行末の詳細なら前の空白も、行の途中なら重なる空白を 1 つ消す。残りが空白だけなら行ごと消す。範囲の調整 (`adjust_range`) は使わない |
| `adjust_range(r)` | 詳細に一部だけかかる範囲を詳細全体に広げる (Vim のヤンクする文字列を合わせるために使う) |
| `caret_pos()` | キャレットを描く位置。通常は `selection.head`、Vim のビジュアルモードでは `visual.cursor` |
| `is_modified()` | テキストの変更、または文字コード・改行コードの変更があるか |
| `is_pristine()` | パスなし・未変更・空。ファイルを開いたときに、このタブを置き換えてよいかの判定に使う |
| `highlight_line(line, text, out)` | 行のハイライト結果を `out` に追加する (状態キャッシュを使う) |
| `revision()` | テキストが変わるたびに増える (UI 側のキャッシュの無効化に使う) |

`preferred_col` は、上下移動で維持する表示桁です。上下移動以外のコマンドでは `None` に戻します。Vim の `$` のあとは `usize::MAX` (行末を保つ) にします。

`visual` は Vim のビジュアルモードの状態 (`anchor` / `cursor` / `linewise`、両端の文字を含む) です。`selection` は、これを描画・コピー用の通常の範囲に変換したものです (`vim` モジュールが同期させる)。

## 5. command.rs — コマンド

```rust
enum Command { Edit(EditCommand), App(AppCommand) }
```

- **`EditCommand`**: 文書に閉じた操作。`editor::execute` が実行する。
  `Move{motion, extend}`, `SelectAll`, `InsertText(String)`, `InsertNewline`, `InsertTab`, `Outdent`, `DeleteBackward/Forward`, `DeleteWordBackward/Forward`, `DeleteSelection`, `DuplicateLine`, `Undo`, `Redo`, `Vim(VimEdit)` (§14)
- **`Motion`**: `Left/Right/Up/Down/WordLeft/WordRight/LineStart/LineEnd/PageUp/PageDown/DocStart/DocEnd`
- **`AppCommand`**: UI 層が実行する操作 (ファイル・タブ・クリップボード・検索・表示)。`ReopenWithEncoding(TextEncoding)`、`SetSaveEncoding`、`SetLineEnding`、`ScrollLines(i32)`、`MoveTab(i32)`、`Cancel` などの引数付きのものを含む。Vim 用に `ToggleVim`、`VimPaste{before, count}`、`VimCommandLine{visual}`、`VimSearch{forward}`、`SearchWord{forward}` がある。

**判断基準**: 「Document と設定だけで完結し、OS の機能を必要としない」なら `EditCommand` に、それ以外は `AppCommand` に入れます。

## 6. editor.rs — `execute(doc, cmd, cfg, view) -> Option<Register>`

戻り値は、Vim の操作でヤンク・削除した文字列です (それ以外は常に `None`)。UI 層はこれをクリップボードに置きます。

| 引数 | 内容 |
|---|---|
| `EditorConfig` | `tab_width` (4)、`insert_spaces` (false)、`auto_indent` (true) |
| `ViewInfo` | `page_lines` (PageUp/PageDown の移動量。UI 層が画面の行数から渡す)、`top_line` (画面の先頭行。Vim の `H` `M` `L` で使う) |

主な仕様:

- **左右移動**: 選択中に Shift なしで押すと、選択の端に縮める。
- **上下移動**: `width::display_col` で表示桁を求め、`preferred_col` に保持する。移動先の行では `char_col_at_display` で最も近い文字位置へ移る (全角 = 2 桁、タブはタブ位置まで)。先頭行で ↑ を押すと文書の先頭へ、最終行で ↓ を押すと文書の末尾へ移る。
- **Home**: インデントの直後と行頭を交互に移る (スマートホーム)。
- **単語移動**: `width::char_class` で文字を 空白 / 改行 / 英数_ / ひらがな / カタカナ / 漢字 / 記号 に分類し、同じ分類が続く範囲を 1 単語とする。右方向は単語の後ろの空白も飛ばす。
- **改行**: 自動インデントが有効なら、キャレットより前にある行頭の空白・タブを引き継ぐ。
- **Tab**: 選択が複数行にまたがれば各行の先頭にインデントを入れる (空行を除く)。そうでなければインデント 1 段分を挿入する (`insert_spaces` なら次のタブ位置まで空白で埋める)。
- **Shift+Tab**: 各行の先頭から、タブ 1 個または最大 `tab_width` 個の空白を取り除く。
- **結合単位**: 1 文字の入力は `Typing`、1 文字の削除は `Deleting`、それ以外は `None`。
- `word_at(buf, pos)`: ダブルクリックでの単語選択に使う。

## 7. keymap.rs — キー入力と入力モード

```rust
struct KeyChord { key: Key, mods: Modifiers{ctrl, shift, alt} }   // 英字は小文字で表す
KeyChord::parse("Ctrl+Shift+Z")   // 設定ファイルで使う表記。"Ctrl++" や "F3" も解析できる
KeyChord::label()                 // メニュー表示用 ("Ctrl+Shift+Z")

trait InputMode {
    fn name(&self) -> String { String::new() }         // ステータスバーに出すモード名 (入力途中のキーを含めてよい)
    fn caret_style(&self) -> CaretStyle { Bar }        // Bar / Block / Underline
    fn normal_caret(&self) -> bool { false }           // Vim のノーマルモードか (UI 層が入力後にキャレットを整える)
    fn on_key(&mut self, chord: KeyChord) -> ModeResponse;
    fn on_text(&mut self, text: &str) -> ModeResponse; // 文字入力 (IME の確定文字列を含む)
    fn on_mouse(&mut self, has_selection: bool) -> ModeResponse { Unhandled }  // マウス操作の終了
    fn reset(&mut self) {}                             // タブ切り替えなどで入力途中の状態を破棄する
    fn shortcut_for(&self, cmd: &Command) -> Option<KeyChord> { None }  // メニュー表示用
}
enum ModeResponse { Commands(Vec<Command>), Consumed, Unhandled }
```

- **キーと文字の分担**: 修飾なし (または Shift のみ) の文字キーは `on_key` に**渡されず**、WM_CHAR 経由で `on_text` に届きます。Ctrl/Alt 付きのキーと、文字を生まないキー (矢印・Enter・Esc など) は `on_key` に届きます。
- **`StandardMode`**: `Keymap` (KeyChord → Command の HashMap) を引くだけの、状態を持たないモードです。既定の割り当ては `Keymap::windows_default()` にあります。
- `Keymap::chord_for(cmd)`: コマンドに割り当てられたキーのうち、修飾が少ないものを返します。
- **`VimMode`**: §14。

## 8. search.rs — 検索・置換

```rust
SearchQuery { pattern, regex: bool, case_sensitive: bool }.compile() -> Regex   // 常に multi_line
find_next(buf, re, from, wrap) -> Option<Range<usize>>   // from 以降で最初の空でない一致
find_prev(buf, re, before, wrap)                         // before より前で始まる最後の一致
replace_all(buf, query, re, replacement) -> Vec<(Range, String)>   // Document::apply に渡す
```

- **ブロック単位の走査**: 行境界で区切った約 1MB (`BLOCK_BYTES`) ごとに、`String` に取り出して regex を適用します。ブロックの先頭は必ず行頭なので、`^` は正しく動きます。
- **折り返し**: 末尾 (または先頭) まで見つからなければ反対側から探し、開始位置の手前で止めます。
- **リテラル検索**: `regex::escape` で正規表現に変換します。置換文字列は展開しません (`$` もそのまま挿入)。
- **空一致**: 検索では無視します。置換では正規表現モードのときだけ有効にします (例: `^` で行頭に挿入)。

## 9. encoding.rs — 文字コード・読み込み・保存

### 読み込み (`load_file` / `decode`)

1. 先頭の 1MB (`HEAD_BYTES`) を読む。
2. 文字コードを決める。順序は次のとおり。
   1. 指定があればそれを使う (BOM が同系統なら BOM を読み飛ばす)。
   2. BOM があれば、それに従う。
   3. 先頭部分が有効な UTF-8 なら UTF-8 とする (末尾で文字が途切れているだけなら有効とみなす)。
   4. それ以外は chardetng (TLD を `jp` として日本語系を優先) で推定する。
3. 1MB ずつ `encoding_rs` の Decoder でデコードし、`Normalizer` に渡す。Normalizer は CRLF / CR を LF に変えながら 64KB ごとに `RopeBuilder` に追加し、同時に改行コードごとの出現数を数える。CR が塊の末尾に来た場合は、次の塊の先頭を見るまで保留する。
4. 最も多かった改行コードを、その文書の改行コードとする (改行がなければ CRLF)。
5. UTF-8 と推定したのに後半で不正なバイト列があった場合は、ファイル全体で推定し直して読み直す。

### 保存 (`save_file` / `encode_to`)

- rope のチャンクごとに、LF を文書の改行コードに置き換えてエンコードする。
  - UTF-16 は自前で変換する。
  - Shift_JIS などは `encode_from_utf8_without_replacement` を使い、**表現できない文字があれば `SaveError::Unmappable(char)`** を返す (勝手に置換しない)。
- 書き込み先は同じフォルダの `.<名前>.lvt-tmp`。fsync してから rename で置き換える。失敗したら一時ファイルを消す。

### `TextEncoding`

`Utf8`, `Utf8Bom`, `Utf16Le`, `Utf16Be`, `ShiftJis`, `EucJp`, `Other(&'static Encoding)`。`COMMON` はメニューに並べる 6 種類。UTF-16 は常に BOM 付きで保存します。

## 10. syntax/ — ハイライト

```rust
trait Highlighter: Send {
    fn name(&self) -> &'static str;
    fn highlight_line(&self, line: &str, state: LineState, out: &mut Vec<StyleSpan>) -> LineState;
    fn is_stateful(&self) -> bool { true }
}
StyleSpan { range: Range<usize> /* 行内のバイト範囲 */, kind: TokenKind }
LineState(u32)   // 行をまたぐ状態 (コードブロック内、コメント内など)。意味はハイライタごとに決める
```

- **`HighlightCache`**: 256 行ごと (`INTERVAL`) に行頭の状態を記録します。行 N の状態は、直前のチェックポイントから N まで解析して求めます。編集時には `Document` が、編集した行以降のチェックポイントを捨てます (`invalidate_from`)。
- **`highlighter_for_path`**: 拡張子からハイライタを選ぶ登録所です。現在の対応は `md` / `markdown` / `mkd` → `MarkdownHighlighter`、それ以外 → `PlainText`。
- **`MarkdownHighlighter`**: 状態は `0` (通常) と `1` (コードフェンス内) の 2 つです。対応しているのは、見出し、引用、リストの記号、`` `code` ``、`**strong**`、`*em*` / `_em_`、`[text](url)` です。

## 11. theme.rs — 配色

`Theme` は、画面要素の色 (背景・選択・行番号など) と、`TokenKind` ごとの `TextStyle {fg, bold, italic, underline}` を持ちます。`Theme::light()` と `Theme::dark()` の 2 種類があります。`TokenKind` を追加した場合は、`TokenKind::COUNT` と両テーマの `tokens` 配列 (宣言順に並べる) を必ず更新してください。

## 12. width.rs — 表示幅と文字種

- `char_width(c, col, tab_width)`: タブは次のタブ位置まで。それ以外は `unicode-width` の値 (全角 = 2)。
- `display_col` / `char_col_at_display`: 文字位置 ⇔ 表示桁の変換。
- `char_class`: 単語境界の判定に使う文字種。

## 13. テスト

※ 13 章のあとに、追加したモジュールの仕様 (14〜18 章) があります。

各モジュールの末尾にある `#[cfg(test)] mod tests` で単体テストをしています。追加・変更したときは、少なくとも次を守ってください。

- 編集コマンド: 実行結果のテキストと選択範囲、および Undo で元に戻ること。
- 文字コード: 保存してから読み込むと元に戻ること (`roundtrip`)。1MB を超える入力でも境界で壊れないこと。
- 検索: 1MB のブロック境界をまたぐ文書で、次・前の検索が正しく動くこと。

性能は `cargo run -p lvt-core --release --example load_bench -- <file> [pattern]` で計測できます。

## 14. vim/ — Vim キーバインド

| ファイル | 責務 |
|---|---|
| `mod.rs` | 型 (`VimMotion` `Operator` `OpTarget` `TextObject` `VimEdit` `Register` `Visual`) と **`VimMode`** (`InputMode` の実装) |
| `parse.rs` | ノーマル・ビジュアルモードのキー列の解析。入力途中のキー列全体を毎回先頭から解析し、`Incomplete` / `Invalid` / `Done(Action)` を返す (`2d3w` → Delete, WordStart, 回数 6) |
| `exec.rs` | `VimEdit` の実行 (`editor::execute` から呼ばれる)。テキストは `Document::apply` で変える |
| `motion.rs` | モーション・テキストオブジェクトの範囲計算 (純粋な関数) |
| `ex.rs` | `:` コマンドの解析、Vim のパターン → Rust の regex の変換、`:s` の置換 |
| `tests.rs` | キー入力から文書の変化までを通しで確かめるテスト (UI 層の代わりの `Harness` を使う) |

### 状態とコマンドの流れ

```
VimMode { state: Normal | Insert | Visual{linewise}, pending: Vec<Tok>, fallback: Keymap,
          last_find, search_forward, last_change: Vec<Input>, recording, replaying }

キー → VimMode::on_key / on_text
  Insert : 文字 → InsertText、Esc / Ctrl+[ → VimEdit::ExitInsert、その他 → fallback (標準のキー割り当て)
  Normal / Visual : pending に積んで parse → Action → translate → Vec<Command>
                    Vim が扱わないキー (Ctrl+S など) は fallback。ただし文字の挿入・削除になるもの (Tab など) は無視する
```

- **モードの遷移は `VimMode` が行い**、文書側の状態 (ビジュアルの範囲・Undo のまとめ) は `VimEdit` の実行で変わる。例: `cw` → `VimMode` は Insert になり、`Operate{Change}` の実行が Undo のまとめを開始する。`Esc` → `ExitInsert` がまとめを終える。
- **モーションの種類** (`motion::MotionKind`): Exclusive / Inclusive / Linewise。演算子の範囲はこれで決まる。Vim の例外規則のうち、次の 2 つを実装している。
  - `cw` は `ce` と同じ (カーソルが空白の上にない場合)。
  - 演算子 + `w` で最後に越えた単語が行末にあるときは、次の行へ進まない。それ以外の Exclusive なモーションで終端が後ろの行の行頭になる場合は、前の行の行末までにする (始点がインデント以前なら行単位にする)。
- **単語**: `width::char_class` を使うので、日本語は ひらがな / カタカナ / 漢字 の境界でも区切る。空行は 1 単語として扱う。
- **キャレット**: ノーマルモードでは改行の上に置かない。UI 層が入力のあとに `vim::normalize_caret` を呼んで整える (選択があれば解除し、行末なら最後の文字へ戻す)。
- **ヤンク・貼り付け**: ヤンク・削除の結果は `Register {text, linewise}` として `editor::execute` から返り、UI 層が OS のクリップボードに置く。`p` は `AppCommand::VimPaste` になり、UI 層がクリップボードを読んで `VimEdit::Paste` を実行する。自分がヤンクした内容と同じなら記録した `linewise` を、違えば「改行で終わるか」を使う。
- **`.` (繰り返し)**: 変更を始めたキーから、ノーマルモードに戻るまでの入力 (挿入した文字列を含む) を `last_change` に記録し、`.` で `VimMode` 自身に入力し直す。ビジュアルモードでの変更は記録しない。
- **ex (`:`)**: `ex::parse` が `ExCommand` を返し、UI 層が実行する (保存・終了は `Deferred`)。範囲は `%`、`N,M`、`.`、`$`、`'<,'>` に対応。

対応しているキーの一覧は [BUILD_AND_VERIFY.md](../BUILD_AND_VERIFY.md) の「Vim キーバインド」を参照。

## 15. link.rs — URL の検出

- `find_urls(line)`: 行内の `http://` `https://` `www.` で始まる URL のバイト範囲。正規表現を使わず 1 回の走査で求める (表示行ごとに呼ばれるため)。
- URL は ASCII の空白以外の文字が続く範囲で、非 ASCII 文字で終わる (日本語の文章では URL の直後に「。」などが空白なしで続くため)。末尾の `.,;:!?'*_` と、対になる開き括弧のない `)` `]` は含めない。
- `normalize(url)`: `www.` には `https://` を補う。開くのは UI 層 (`http` / `https` のみ)。

## 16. settings.rs — 記憶する設定値

`Settings` は `キー = 値` の対応表で、TOML のサブセット (`キー = "文字列"` / 真偽値 / 整数、`#` コメント) で読み書きします。将来の設定ファイル (ロードマップ R1) で本物の TOML パーサに置き換えても、そのまま読める形式です。知らないキーも保存時に残します。読めないファイルは空の設定として扱い、起動を止めません。

| キー | 値 |
|---|---|
| `input_mode` | `"vim"` (既定。キーがないときも Vim) / `"standard"` |
| `txt_detail_view` | `true` (既定) / `false`。.txt で装飾表示を使うか |
| `detail_panel_pos` | `"0.9500,0.0000"` のような、詳細パネルの位置 (本文の領域に対する割合)。ドラッグで動かすと保存する |

## 17. detail.rs — 詳細 (`*detail[…]`)

- **記法**: 行内の `*detail[` から、エスケープされていない最初の `]` まで (1 行で最初の 1 つ)。内容の中では `\n` が改行、`\]` が `]`、`\\` が `\` (`escape` / `unescape`)。閉じがなければ詳細ではない。新しく作るときは行末に置く (ファイルをそのまま読むと「本文 → 詳細」の順になる)。
- **検出**: `find_iter` が文字列をコピーせずに 1 回の走査で探す (開始記号は先頭以外に `*` を含まないので、不一致のときの照合のやり直しが単純)。
- **`Token {start, end}`**: 行内の文字位置 (`*` の位置と `]` の直後)。`content()` は `[` と `]` の間。
- **キャレットと編集の整合** (`Document` が行う)。詳細は本文で見えない (幅 0) ので、次の規則で「見えている文字」に対する操作として振る舞わせる:
  - キャレットは詳細の直前 (`start`) にだけ置く。内側や直後 (`end`) に来たら直前へ寄せる。ただし直前から 1 文字前へ進んだ場合 (→ や Vim の `l`) は、詳細の直後の文字を越えた位置 (`end + 1`) へ進める。
  - 削除範囲の調整 (`adjust_range`): 詳細の直前から始まり詳細の中で終わる範囲 (Del・`x`) は、詳細の後ろの同じ文字数を消す。詳細の手前から詳細の中・直後までの範囲は詳細の手前までにする。詳細の中から始まる範囲は詳細の直後からにする。詳細全体をまたいでその先まで消す範囲 (`dd`・複数行の選択) だけ、詳細ごと消える。内容の中に収まる範囲 (置換・パネルからの書き戻し) はそのまま。
  - 挿入位置の調整: 詳細の内側への挿入は詳細の後ろへ。行末にある詳細の直前への改行の挿入 (行末で Enter) は、詳細を元の行に残すため詳細の後ろへ。
  - Vim の `x` の範囲は、詳細を飛び越えても次の行へはみ出さない (行を結合しない)。
  - Vim の `$` とノーマルモードのキャレットは、行の最後の「見える」文字までにする (`motion::last_visible_char`)。
- 有効・無効は UI 層が拡張子 (`.lvt` は常に、`.txt` は設定) で決める (`Document::set_detail_view`)。

## 18. lineview.rs — 1 行の表示モデル (装飾表示)

`LineView::build(line, caret)` が、文書上の 1 行から次のものを作ります。状態は持たず、描画・クリック判定・上下移動のたびに作ります。

| フィールド / API | 内容 |
|---|---|
| `text` | 表示用の文字列。詳細は隠し (幅 0)、`[label](url)` は label に、`` `code` `` と `**強調**` は記号を除いた文字列になる |
| `styles` | 表示用の文字列に対する書式 (`Link` / `Code` / `Strong`、展開中の記号は `Punctuation`) |
| `links` | label の表示上のバイト範囲と url (Ctrl+クリックで開く) |
| `detail` | 行の詳細 (UI 層が行番号の欄にバッジを描く) |
| `heading` | 見出しのレベル (行頭の `#`〜`######` + 空白、または `#` だけ)。記号を隠し、本文を `Strong` にする。キャレットがこの行にあれば記号を見せる。背景の帯は UI 層が描く |
| `to_view(col, round_up)` / `to_doc(vcol)` | 文書上の桁 ⇔ 表示上の桁。隠した記号の位置では、開き記号なら記号の前、閉じ記号なら記号の後ろ (要素の外側) に対応させる |

- **展開**: `caret` (キャレットがこの行にあるときの桁) が要素の上 (両端を含む) にあれば、その要素だけ記法のまま表示する (Typora と同じ方式)。キャレットは詳細以外の要素の中に自由に入れる。
- **解析**: 左から 1 回走査する。コード → 強調 → リンクの順に判定し、入れ子にはしない。詳細の範囲は除く。
- **上下移動**: `editor::current_display_col` (キャレット行。展開した状態) と `pos_at_display_col` (移動先の行。展開しない状態) で表示桁を数える。そのため、移動先の要素が展開されると、表示上の桁が少しずれることがある。
