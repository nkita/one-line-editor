# lvt-app (Windows フロントエンド) 仕様

Win32 API と Direct2D/DirectWrite で書かれた UI 層です。`windows` crate 0.62 を使います。
`src/main.rs` は `#[cfg(windows)] mod win;` だけを持ちます。Windows 以外でビルドすると何もしない exe になるので、ワークスペース全体の `cargo test` は Linux でも通ります。

## 1. モジュール

| ファイル | 責務 |
|---|---|
| `win/mod.rs` | 起動 (DPI 対応の宣言、COM 初期化、ウィンドウクラス、メニュー、コマンドライン引数)、メッセージループ、**wndproc**、モーダルを伴う処理 (`run_deferred`: 開く・保存・閉じる・終了・開き直し) |
| `win/app.rs` | **`App`**: タブ、入力モード、検索状態、入力欄、マウスのドラッグ、スクロール、IME の位置、コマンドの振り分け |
| `win/paint.rs` | `App::paint`: タブバー、本文 (行番号・現在行・検索一致・選択・ハイライト・キャレット)、入力欄、ステータスバー |
| `win/render.rs` | `Renderer`: D2D/DWrite のファクトリ、レンダーターゲット、ブラシのキャッシュ、テキスト書式とフォント寸法、描画プリミティブ、ヒットテスト |
| `win/menu.rs` | `Menus`: メニューバーの構築 (項目ごとに `Command` を対応付ける)、チェック状態・有効状態の更新 |
| `win/prompt.rs` | `Prompt`: 下部の 1 行入力欄の状態 (種類・テキスト・キャレット) |
| `win/dialogs.rs` | ファイルを開く/保存ダイアログ、メッセージボックス、クリップボード (CRLF ⇔ LF の変換) |
| `win/input.rs` | 仮想キーコード → `Key`、修飾キーの状態 |
| `win/util.rs` | UTF-16 変換 (文字・バイト ⇔ UTF-16 位置)、lParam/wParam の分解 |
| `build.rs` + `res/` | アプリケーション マニフェスト (`res/lvt.manifest`) を exe に埋め込む。コモンコントロール v6 を指定し、ダイアログ・メッセージボックスを現行の Windows の見た目にする。GNU では `x86_64-w64-mingw32-windres` でリソースを作ってリンクし、MSVC ではリンカーの `/MANIFESTINPUT` を使う |

## 2. 状態の所有と再入対策 (重要)

```rust
thread_local! { static APP: RefCell<Option<App>> }
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R>   // try_borrow_mut に失敗したら None
```

- wndproc では、メッセージごとに `with_app` で `App` を借用して処理します。
- **モーダルダイアログ (`GetOpenFileNameW`、`MessageBoxW` など) は、`App` を借用したまま呼んではいけません。** モーダルは内部でメッセージループを回すので、wndproc が再入して二重借用になります。
  → `App` のメソッドは、モーダルが必要な処理を **`Deferred`** として返します。wndproc は借用を解いてから `run_deferred` を実行します。`run_deferred` の中では、必要なときだけ短く `with_app` を呼びます。
- `SetWindowTextW` や `SetScrollInfo` などは、メッセージを同期的に送ることがあります。送られたメッセージの処理で `with_app` が失敗するのは仕様です (そのメッセージは既定の処理になり、`App` 側は次の `refresh` で整合します)。スクロールバーは `SIF_DISABLENOSCROLL` で常に表示しておき、表示・非表示の切り替えによる WM_SIZE を起こさないようにしています。

```rust
enum Deferred { Open, OpenPaths(Vec<PathBuf>), Save(usize), SaveAs(usize), CloseTab(usize), Exit, Reopen(TextEncoding),
                OpenUrl(String), Quit { idx, save, force } }   // Quit は Vim の :q / :q! / :wq (最後のタブなら終了)
```

## 3. メッセージ処理

| メッセージ | 処理 |
|---|---|
| `WM_PAINT` | `BeginPaint` → `App::paint` → `EndPaint` |
| `WM_ERASEBKGND` | 1 を返す (ちらつき防止) |
| `WM_SIZE` | `refresh` (スクロール範囲を更新して再描画)。レンダーターゲットのサイズは描画時に合わせる |
| `WM_KEYDOWN` / `WM_SYSKEYDOWN` | `App::on_key_down` (§4)。処理しなければ `DefWindowProc` へ渡す (Alt によるメニュー操作や Alt+F4 を生かすため) |
| `WM_SYSCHAR` | 直前の Alt+キーを自分で処理した場合は握りつぶす (警告音を防ぐ) |
| `WM_CHAR` | `App::on_char`: サロゲートペアを結合し、制御文字は捨てる。入力欄が開いていれば入力欄へ、そうでなければ `InputMode::on_text` へ |
| `WM_LBUTTONDOWN` / `WM_LBUTTONDBLCLK` / `WM_MOUSEMOVE` / `WM_LBUTTONUP` | クリック、ドラッグ選択、ダブルクリックでの単語選択、行番号での行選択。ドラッグ中に領域の外へ出たらタイマー (30ms) で自動スクロール。Ctrl+クリックで URL を開く (`Deferred::OpenUrl`)。タブのドラッグで並べ替え (§12)。操作が終わったら `InputMode::on_mouse` を呼ぶ (Vim は選択があればビジュアルモードにする) |
| `WM_KEYDOWN` / `WM_KEYUP` (VK_CONTROL) | マウスが URL の上にあれば、カーソルを手の形に切り替える (`refresh_cursor`) |
| `WM_MBUTTONUP` | タブ上なら、そのタブを閉じる |
| `WM_MOUSEWHEEL` / `WM_MOUSEHWHEEL` | 120 = 3 行。端数は積算する (高精度タッチパッド対応)。Shift なら横、Ctrl なら文字サイズ |
| `WM_VSCROLL` / `WM_HSCROLL` | スクロールバーの操作。つまみのドラッグは `SIF_TRACKPOS` で 32 ビットの位置を取る |
| `WM_SETCURSOR` | 本文の上では I ビーム (Ctrl を押して URL の上にあれば手の形)、それ以外では矢印 |
| `WM_COMMAND` | メニュー ID → `Menus::command` → `App::execute_commands` |
| `WM_INITMENUPOPUP` | `Menus::update` (文字コード・改行・テーマのチェック、Undo/Redo・切り取り・コピーの有効/無効) |
| `WM_IME_STARTCOMPOSITION` / `WM_IME_COMPOSITION` / `WM_IME_ENDCOMPOSITION` | 本文への入力では変換中の文字列を `App::ime` に保持して自前で描き、確定文字列を `InputMode::on_text` へ渡す (既定の処理には渡さない)。入力欄では既定の処理に任せる (9 章) |
| `WM_DROPFILES` | ドロップされたファイルを開く (`Deferred::OpenPaths`) |
| `WM_DPICHANGED` | `Renderer::set_dpi` のあと、推奨された矩形にウィンドウを移す |
| `WM_CLOSE` | `Deferred::Exit` (未保存のタブを順に確認し、すべて了承されれば `DestroyWindow`) |
| `WM_DESTROY` | `App` を破棄して `PostQuitMessage` |

## 4. キー入力の振り分け (`App::on_key_down`)

```
仮想キー → Key (変換できないキー、例えば Alt 単体は Unhandled)
  ├─ 入力欄が開いている → prompt_key: Enter/Esc/BS/Del/←→/Home/End/Alt+C/Alt+R/Ctrl+V/Ctrl+A
  │                         を処理し、それ以外 (F3, Ctrl+S など) は下へ流す
  ├─ 修飾なしの文字キー → Unhandled (WM_CHAR 側で処理する)
  └─ InputMode::on_key(chord) → Commands なら順に App::execute。Deferred が返ったらそこで止める
                                Consumed (入力途中) なら再描画だけする (ステータスバーに入力途中のキーを出す)

コマンド列を実行し終えたら App::after_input:
  - InputMode::normal_caret() (Vim のノーマルモード) なら vim::normalize_caret でキャレットを整える
  - ノーマルモードに戻ったときは IME を閉じる (ImmSetOpenStatus)。日本語入力のまま j などを押さないように
```

コマンド列の途中では `after_input` を呼びません。`.` の繰り返しなどで「挿入 → Esc」を含むコマンド列が一度に届くため、途中でキャレットを整えると位置がずれます。

## 5. 画面の配置 (`Geometry`, 単位は DIP)

```
┌──────────────────────────────────────────── width ─┐
│ タブバー                         (tabbar_h)          │
├──────────┬─────────────────────────────────────────┤ text_top
│ 行番号    │ 本文 (text_left から。横方向は scroll_x)    │
│ gutter_w │                                         │
├──────────┴─────────────────────────────────────────┤ text_bottom = prompt_top
│ 入力欄 (開いているときだけ)                             │
├────────────────────────────────────────────────────┤ status_top
│ ステータスバー: 行・桁 / メッセージ   モード  言語  文字コード  改行 │
└────────────────────────────────────────────────────┘ height
```

- DIP = 物理ピクセル ÷ (DPI / 96)。マウス座標は `to_dip` で変換します。
- 行の高さ (`Renderer::line_height`) は、"M" の高さ × 1.15 をデバイスピクセルに切り上げた値です。
- 行番号の欄の幅は、(行数の桁数 (最低 3) + 2) × 半角幅です。

## 6. 描画 (`paint.rs`)

本文の 1 行ごとに、次の順で処理します。

1. 行の文字列を取得する (先頭 `MAX_RENDER_CHARS` = 20,000 文字まで) → UTF-16 → `IDWriteTextLayout`。行の高さで縦中央に置く。
2. ハイライトの結果を書式として適用する (`SetDrawingEffect` で色、`SetFontWeight`、`SetFontStyle`、`SetUnderline`)。**位置の計算より先に行う** (太字で文字幅が変わるため)。
3. 背景を描く: 現在行 → 検索一致 → 選択範囲 (改行を含む場合は 0.6 文字分延ばす) → ブロックキャレット。
4. 文字を描く (`DrawTextLayout`。カラー絵文字を有効にする)。
5. バー型・下線型のキャレットを描く (ウィンドウにフォーカスがあるときだけ)。

本文は行番号の欄の右側でクリップし、行番号は最後に右寄せで描きます。
ブラシは色ごとにキャッシュし、`D2DERR_RECREATE_TARGET` が返ったらレンダーターゲットとブラシを作り直します。

## 7. スクロール

- 縦: `Tab::scroll_line` (先頭に表示する行)。最大値は `行数 − 表示行数` です。
- 横: `Tab::scroll_x` (DIP)。最大値は、これまでに描いた行の最大幅 (`Tab::max_width`) です。
- コマンドを実行するたびに `ensure_caret_visible` を呼び、キャレットが見える位置までスクロールします。横方向は、見えない場合に 1/4 から 3/4 の位置へ寄せます。検索と行移動では、移動先の行を中央付近に表示します (`center_line`)。

## 8. 入力欄 (`prompt.rs`)

| 種類 | Enter | Shift+Enter | 入力のたびに |
|---|---|---|---|
| `Find` | 次を検索 (欄は開いたまま) | 前を検索 | インクリメンタル検索 (文書が 32MB 以下の場合) |
| `ReplaceFind` | 置換後の文字列の入力へ進む | — | 検索条件を更新して強調表示 |
| `ReplaceWith` | すべて置換 (1 回の Undo で戻る) | — | — |
| `GoToLine` | `行` または `行:桁` へ移動 | — | — |
| `VimSearch{forward}` (`/` `?`) | 検索して閉じる (カーソル位置の一致は飛ばす) | — | インクリメンタル検索。Esc で元の位置に戻る |
| `Command{visual}` (`:`) ※ | `vim::ex::parse` → `App::run_ex`。保存・終了は `Deferred` を返す | — | — |

開いたときの初期値は、選択中の文字列 (1 行かつ 1000 バイト未満の場合)、なければ前回の検索文字列です。

※ `VimSearch` と `Command` (`Prompt::in_status_bar`) は入力欄の行を追加せず、ステータスバーの左側 (行・桁の位置) に `:` などのラベルと入力文字列を UI フォントで描く。IME の変換ウィンドウもそこに合わせる。

## 9. IME

IMM32 を使います。

- **本文への入力**: 変換中の文字列は本文に埋め込んで自前で描きます (OS の変換ウィンドウは使いません。別ウィンドウ・別フォントで重ねて描かれ、後ろの文字と重なるため)。
  - `WM_IME_COMPOSITION` で `GCS_COMPSTR` (変換中の文字列)・`GCS_CURSORPOS`・`GCS_COMPATTR` (変換対象の文節) を読み、`App::ime` (`ImeComposition`) に保持します。`GCS_RESULTSTR` (確定した文字列) は `InputMode::on_text` へ渡します。これらのメッセージは既定の処理に渡しません (渡すと OS の変換ウィンドウが出て、確定文字列が `WM_CHAR` でも届いてしまう)。
  - 描画 (`paint_text`) では、キャレットのある行のキャレット位置に変換中の文字列を挿入してレイアウトし、下線を引き、変換対象の文節に背景色を付けます。文書上の位置から表示上の位置への変換は `to_view` で行います。
  - 候補ウィンドウは `ImmSetCandidateWindow` (`CFS_EXCLUDE`) で、入力中の行を隠さない位置に出します。
- **入力欄への入力**: 既定の処理に任せます。`position_ime` で変換ウィンドウの位置 (`CFS_POINT`) とフォント (`ImmSetCompositionFontW`) をキャレットに合わせ、確定した文字列は `WM_IME_CHAR` → `DefWindowProc` → `WM_CHAR` の経路で届きます。

## 10. ファイル操作の流れ (`win/mod.rs`)

- **開く** (`open_paths`): 絶対パスにする → すでに開いていればそのタブへ切り替え → `Document::open` → 現在のタブが `is_pristine` なら置き換え、そうでなければ新しいタブにする → ステータスバーに所要時間を表示する。
- **保存** (`save`): パスがない、または「名前を付けて保存」ならダイアログを出す → `App::save_tab` → `Unmappable` なら「UTF-8 で保存するか」を確認して再試行する。
- **閉じる** (`confirm_discard`): 変更があれば、そのタブを表示してから「はい / いいえ / キャンセル」で確認する。
- **終了** (`confirm_close_all`): すべてのタブで `confirm_discard` を行い、1 つでもキャンセルされれば中止する。

## 11. Windows API を使うときの注意

- `windows` crate の機能 (feature) は `crates/lvt-app/Cargo.toml` で列挙しています。API が見つからないときは、まず必要な feature を確認してください (例: `SetScrollInfo` は `Win32_UI_Controls` にあります)。
- 文字列は `util::wide(&str)` で NUL 終端の UTF-16 にし、`PCWSTR(v.as_ptr())` で渡します。`v` が呼び出し中に生きていることを確認してください。
- 戻り値の `Result` / `BOOL` は、無視してよい場合でも `let _ =` で明示します。
- 新しい `unsafe` ブロックは小さく保ち、その API 呼び出しだけを囲みます。

## 12. タブのドラッグ

- タブを押すと `Drag::Tab {idx, start_x, moved}` を開始し、マウスをキャプチャする。4 DIP 以上動かしたらドラッグとみなす。
- マウスが隣のタブの中央を越えるたびに `App::move_tab(from, to)` で入れ替える。アクティブなタブは同じ文書を指し続ける。
- 次の描画までに複数回マウスが動いても正しく判定できるよう、`move_tab` は当たり判定 (`tab_hits`) も並べ替えて位置を計算し直す。
- `Ctrl+Shift+PageUp/PageDown` (`AppCommand::MoveTab`) でも移動できる。

## 13. 入力モードの切り替えと設定の保存

- `AppCommand::ToggleVim` (表示メニュー / `Ctrl+Alt+V`) で `App.mode` を `StandardMode` と `VimMode` で差し替え、メニューを作り直す (ショートカットの表示が変わるため。`SetMenu` → 古いメニューを `DestroyMenu`)。
- 状態は `%APPDATA%\lvt\settings.toml` の `input_mode` に保存し、起動時に読む (`lvt_core::settings`)。設定がなければ Vim を既定にする (`app::vim_by_default`)。保存に失敗したらステータスバーに表示する。
- タブを切り替えたら、元のタブの Undo のまとめとビジュアルモードを終え、`InputMode::reset` で入力途中の状態を破棄する。
- ヤンク・削除した文字列 (`editor::execute` の戻り値) はクリップボードに置き、`App.register` に覚える (`p` で行単位かどうかを判断するため)。
