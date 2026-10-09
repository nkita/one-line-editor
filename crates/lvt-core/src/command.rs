//! コマンド。キー入力・メニュー・(将来の) コマンドパレットやマクロは、
//! すべてこの `Command` に変換されてから実行される。
//!
//! - `EditCommand`: 文書に対する操作。OS 非依存で `editor::execute` が実行する。
//! - `AppCommand` : ファイル・タブ・クリップボード・ダイアログなど UI 層が実行する操作。
//!
//! Vim キーバインド等を追加する場合も、新しい入力モードがこれらのコマンドを
//! 発行するだけでよい。足りない操作はここに追加する。

use crate::encoding::{LineEnding, TextEncoding};
use crate::vim::VimEdit;

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Edit(EditCommand),
    App(AppCommand),
}

/// カーソル移動の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    /// 行頭 (インデントの後ろと桁 0 を交互に)。
    LineStart,
    LineEnd,
    PageUp,
    PageDown,
    DocStart,
    DocEnd,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditCommand {
    Move { motion: Motion, extend: bool },
    SelectAll,
    InsertText(String),
    InsertNewline,
    InsertTab,
    /// 選択行のインデントを 1 段戻す。
    Outdent,
    DeleteBackward,
    DeleteForward,
    DeleteWordBackward,
    DeleteWordForward,
    DeleteSelection,
    DuplicateLine,
    Undo,
    Redo,
    /// Vim キーバインドの編集操作。
    Vim(VimEdit),
}

#[derive(Clone, Debug, PartialEq)]
pub enum AppCommand {
    NewTab,
    Open,
    Save,
    SaveAs,
    CloseTab,
    NextTab,
    PrevTab,
    /// アクティブなタブを左 (負) / 右 (正) へ移動する。
    MoveTab(i32),
    Exit,
    Cut,
    Copy,
    Paste,
    Find,
    FindNext,
    FindPrev,
    Replace,
    GoToLine,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleTheme,
    ReopenWithEncoding(TextEncoding),
    SetSaveEncoding(TextEncoding),
    SetLineEnding(LineEnding),
    /// 表示を行単位でスクロールする (カーソルは動かさない)。
    ScrollLines(i32),
    /// プロンプトバー (検索欄など) を閉じる。
    Cancel,
    /// Vim キーバインドの有効・無効を切り替える。
    ToggleVim,
    /// Vim の p / P: クリップボードの内容を貼り付ける (行単位かどうかは UI 層が判断する)。
    VimPaste { before: bool, count: usize },
    /// Vim の `:` コマンドライン。`visual` ならビジュアルモードの行範囲を `'<,'>` として使う。
    VimCommandLine { visual: bool },
    /// Vim の `/` (forward) と `?`。
    VimSearch { forward: bool },
    /// Vim の `*` / `#`: キャレット位置の単語を検索する。
    SearchWord { forward: bool },
}

impl From<EditCommand> for Command {
    fn from(c: EditCommand) -> Self {
        Command::Edit(c)
    }
}

impl From<AppCommand> for Command {
    fn from(c: AppCommand) -> Self {
        Command::App(c)
    }
}
