//! キー入力の抽象化と入力モード。
//!
//! UI 層は OS のキーイベントを `KeyChord` (キー＋修飾キー) と文字入力に変換し、
//! 現在の `InputMode` に渡す。入力モードは状態を持てるので、Vim のような
//! モーダル編集は `InputMode` を実装した新しい型 (例: `VimMode`) として追加できる。
//! 既定は Windows 標準的な操作体系の `StandardMode`。

use std::collections::HashMap;

use crate::command::{AppCommand, Command, EditCommand, Motion};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// 文字キー。英字は小文字で表す (Shift は `Modifiers` 側で表現)。
    Char(char),
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Backspace,
    Enter,
    Tab,
    Escape,
    Space,
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub key: Key,
    pub mods: Modifiers,
}

impl KeyChord {
    pub fn new(key: Key, mods: Modifiers) -> Self {
        Self { key, mods }
    }

    /// "Ctrl+Shift+Z" のような表記を解析する (設定ファイルでの利用を想定)。
    pub fn parse(s: &str) -> Option<Self> {
        let mut mods = Modifiers::default();
        let mut key = None;
        for part in s.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods.ctrl = true,
                "shift" => mods.shift = true,
                "alt" => mods.alt = true,
                "left" => key = Some(Key::Left),
                "right" => key = Some(Key::Right),
                "up" => key = Some(Key::Up),
                "down" => key = Some(Key::Down),
                "home" => key = Some(Key::Home),
                "end" => key = Some(Key::End),
                "pageup" | "pgup" => key = Some(Key::PageUp),
                "pagedown" | "pgdn" => key = Some(Key::PageDown),
                "insert" | "ins" => key = Some(Key::Insert),
                "delete" | "del" => key = Some(Key::Delete),
                "backspace" | "bs" => key = Some(Key::Backspace),
                "enter" | "return" => key = Some(Key::Enter),
                "tab" => key = Some(Key::Tab),
                "escape" | "esc" => key = Some(Key::Escape),
                "space" => key = Some(Key::Space),
                p if p.len() >= 2 && p.starts_with('f') && p[1..].parse::<u8>().is_ok() => {
                    key = Some(Key::F(p[1..].parse().ok()?))
                }
                p if p.chars().count() == 1 => key = Some(Key::Char(p.chars().next()?)),
                "" if s.ends_with('+') => key = Some(Key::Char('+')),
                _ => return None,
            }
        }
        Some(Self { key: key?, mods })
    }

    /// メニュー表示用の表記 ("Ctrl+S" など)。
    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.mods.ctrl {
            s.push_str("Ctrl+");
        }
        if self.mods.shift {
            s.push_str("Shift+");
        }
        if self.mods.alt {
            s.push_str("Alt+");
        }
        match self.key {
            Key::Char(c) => s.extend(c.to_uppercase()),
            Key::F(n) => s.push_str(&format!("F{n}")),
            Key::PageUp => s.push_str("PgUp"),
            Key::PageDown => s.push_str("PgDn"),
            Key::Escape => s.push_str("Esc"),
            Key::Delete => s.push_str("Del"),
            other => s.push_str(&format!("{other:?}")),
        }
        s
    }
}

/// キー → コマンドの対応表。
#[derive(Clone, Default)]
pub struct Keymap {
    bindings: HashMap<KeyChord, Command>,
}

impl Keymap {
    pub fn bind(&mut self, chord: &str, cmd: impl Into<Command>) {
        let chord = KeyChord::parse(chord).unwrap_or_else(|| panic!("invalid key chord: {chord}"));
        self.bindings.insert(chord, cmd.into());
    }

    pub fn get(&self, chord: &KeyChord) -> Option<&Command> {
        self.bindings.get(chord)
    }

    /// コマンドに割り当てられたキー (メニューのショートカット表示用)。
    pub fn chord_for(&self, cmd: &Command) -> Option<KeyChord> {
        let mut found: Vec<_> = self.bindings.iter().filter(|(_, c)| *c == cmd).map(|(k, _)| *k).collect();
        // 修飾キーが少ないものを優先して決定的に選ぶ
        found.sort_by_key(|k| (k.mods.shift as u8 + k.mods.alt as u8, format!("{k:?}")));
        found.into_iter().next()
    }

    /// Windows の一般的なテキストエディタに準じた既定のキー割り当て。
    pub fn windows_default() -> Self {
        use AppCommand as A;
        use EditCommand as E;
        let mut m = Keymap::default();
        let motions = [
            ("Left", Motion::Left),
            ("Right", Motion::Right),
            ("Up", Motion::Up),
            ("Down", Motion::Down),
            ("Ctrl+Left", Motion::WordLeft),
            ("Ctrl+Right", Motion::WordRight),
            ("Home", Motion::LineStart),
            ("End", Motion::LineEnd),
            ("PageUp", Motion::PageUp),
            ("PageDown", Motion::PageDown),
            ("Ctrl+Home", Motion::DocStart),
            ("Ctrl+End", Motion::DocEnd),
        ];
        for (chord, motion) in motions {
            m.bind(chord, E::Move { motion, extend: false });
            m.bind(&format!("Shift+{chord}"), E::Move { motion, extend: true });
        }
        m.bind("Ctrl+Up", A::ScrollLines(-1));
        m.bind("Ctrl+Down", A::ScrollLines(1));
        m.bind("Enter", E::InsertNewline);
        m.bind("Shift+Enter", E::InsertNewline);
        m.bind("Tab", E::InsertTab);
        m.bind("Shift+Tab", E::Outdent);
        m.bind("Backspace", E::DeleteBackward);
        m.bind("Shift+Backspace", E::DeleteBackward);
        m.bind("Delete", E::DeleteForward);
        m.bind("Ctrl+Backspace", E::DeleteWordBackward);
        m.bind("Ctrl+Delete", E::DeleteWordForward);
        m.bind("Ctrl+A", E::SelectAll);
        m.bind("Ctrl+Z", E::Undo);
        m.bind("Ctrl+Y", E::Redo);
        m.bind("Ctrl+Shift+Z", E::Redo);
        m.bind("Ctrl+D", E::DuplicateLine);

        m.bind("Ctrl+N", A::NewTab);
        m.bind("Ctrl+O", A::Open);
        m.bind("Ctrl+S", A::Save);
        m.bind("Ctrl+Shift+S", A::SaveAs);
        m.bind("Ctrl+W", A::CloseTab);
        m.bind("Ctrl+F4", A::CloseTab);
        m.bind("Ctrl+Tab", A::NextTab);
        m.bind("Ctrl+PageDown", A::NextTab);
        m.bind("Ctrl+Shift+Tab", A::PrevTab);
        m.bind("Ctrl+PageUp", A::PrevTab);
        m.bind("Ctrl+X", A::Cut);
        m.bind("Shift+Delete", A::Cut);
        m.bind("Ctrl+C", A::Copy);
        m.bind("Ctrl+Insert", A::Copy);
        m.bind("Ctrl+V", A::Paste);
        m.bind("Shift+Insert", A::Paste);
        m.bind("Ctrl+F", A::Find);
        m.bind("F3", A::FindNext);
        m.bind("Shift+F3", A::FindPrev);
        m.bind("Ctrl+H", A::Replace);
        m.bind("Ctrl+G", A::GoToLine);
        m.bind("Ctrl+=", A::ZoomIn);
        m.bind("Ctrl++", A::ZoomIn);
        m.bind("Ctrl+;", A::ZoomIn); // JIS 配列の「+」キー
        m.bind("Ctrl+-", A::ZoomOut);
        m.bind("Ctrl+0", A::ZoomReset);
        m.bind("Escape", A::Cancel);
        m
    }
}

/// キャレットの形 (Vim のノーマルモードではブロックにする等)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretStyle {
    Bar,
    Block,
    Underline,
}

/// 入力モードの応答。
#[derive(Debug)]
pub enum ModeResponse {
    /// これらのコマンドを順に実行する。
    Commands(Vec<Command>),
    /// 入力を消費したが実行するものはない (多打鍵コマンドの途中など)。
    Consumed,
    /// このモードでは扱わない (UI 層の既定処理へ)。
    Unhandled,
}

/// 入力モード。キー入力を解釈してコマンド列を返す。
pub trait InputMode {
    /// ステータスバーに表示する名前 (空ならば表示しない)。
    fn name(&self) -> &str {
        ""
    }

    fn caret_style(&self) -> CaretStyle {
        CaretStyle::Bar
    }

    /// 文字を生まないキー、または修飾キー付きのキーが押された。
    fn on_key(&mut self, chord: KeyChord) -> ModeResponse;

    /// 文字が入力された (IME の確定文字列も含む)。
    fn on_text(&mut self, text: &str) -> ModeResponse;

    /// メニュー表示用のショートカット。
    fn shortcut_for(&self, _cmd: &Command) -> Option<KeyChord> {
        None
    }
}

/// 標準 (モードレス) の入力モード。
pub struct StandardMode {
    keymap: Keymap,
}

impl StandardMode {
    pub fn new(keymap: Keymap) -> Self {
        Self { keymap }
    }
}

impl Default for StandardMode {
    fn default() -> Self {
        Self::new(Keymap::windows_default())
    }
}

impl InputMode for StandardMode {
    fn on_key(&mut self, chord: KeyChord) -> ModeResponse {
        match self.keymap.get(&chord) {
            Some(cmd) => ModeResponse::Commands(vec![cmd.clone()]),
            None => ModeResponse::Unhandled,
        }
    }

    fn on_text(&mut self, text: &str) -> ModeResponse {
        ModeResponse::Commands(vec![EditCommand::InsertText(text.to_string()).into()])
    }

    fn shortcut_for(&self, cmd: &Command) -> Option<KeyChord> {
        self.keymap.chord_for(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_chords() {
        let c = KeyChord::parse("Ctrl+Shift+Z").unwrap();
        assert_eq!(c.key, Key::Char('z'));
        assert!(c.mods.ctrl && c.mods.shift && !c.mods.alt);
        assert_eq!(KeyChord::parse("Ctrl++").unwrap().key, Key::Char('+'));
        assert_eq!(KeyChord::parse("Shift+F3").unwrap().key, Key::F(3));
        assert_eq!(c.label(), "Ctrl+Shift+Z");
    }

    #[test]
    fn default_keymap_resolves() {
        let mut mode = StandardMode::default();
        let chord = KeyChord::parse("Ctrl+S").unwrap();
        assert!(matches!(mode.on_key(chord), ModeResponse::Commands(c) if c == vec![Command::App(AppCommand::Save)]));
        assert_eq!(mode.shortcut_for(&AppCommand::Copy.into()).unwrap().label(), "Ctrl+C");
    }
}
