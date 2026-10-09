//! コマンド。キー入力・メニュー・(将来の) コマンドパレットやマクロは、
//! すべてこの `Command` に変換されてから実行される。
//!
//! - `EditCommand`: 文書に対する操作。OS 非依存で `editor::execute` が実行する。
//! - `AppCommand` : ファイル・タブ・クリップボード・ダイアログなど UI 層が実行する操作。
//!
//! Vim キーバインド等を追加する場合も、新しい入力モードがこれらのコマンドを
//! 発行するだけでよい。足りない操作はここに追加する。

use crate::encoding::{LineEnding, TextEncoding};

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
