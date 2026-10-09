//! Vim キーバインド (`InputMode` の実装)。
//!
//! - `VimMode`: キー入力 → `Command` 列の変換 (状態機械)。文書には触れないので単体テストしやすい。
//! - `parse`: ノーマル・ビジュアルモードのキー列の解析。
//! - `exec`: Vim 固有の編集 (`EditCommand::Vim(VimEdit)`) の実行。
//! - `motion`: モーション・テキストオブジェクトの範囲計算。
//! - `ex`: `:` コマンドラインの解析と置換。
//!
//! ヤンク・削除した文字列は `editor::execute` の戻り値 (`Register`) として UI 層に返り、
//! UI 層が OS のクリップボードに置く (Vim の `clipboard=unnamed` 相当)。

pub mod ex;
mod exec;
pub mod motion;
mod parse;

pub use exec::{execute, leave_visual, normalize_caret, set_visual_cursor, visual_line_range};

use crate::command::{AppCommand, Command, EditCommand};
use crate::keymap::{CaretStyle, InputMode, Key, KeyChord, Keymap, ModeResponse};
use parse::{Action, Cmd, Kind, Parsed, Tok};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimMotion {
    Left,
    Right,
    Up,
    Down,
    WordStart { big: bool },
    WordEnd { big: bool },
    WordBack { big: bool },
    /// `0`
    LineStart,
    /// `^`
    FirstNonBlank,
    /// `$`
    LineEnd,
    /// `gg` (last = false) / `G` (last = true)。回数があればその行。
    GotoLine { last: bool },
    /// `f` `F` `t` `T`。`repeat` は `;` `,` による繰り返し。
    FindChar { c: char, forward: bool, till: bool, repeat: bool },
    /// `%`
    MatchPair,
    ParagraphForward,
    ParagraphBack,
    ScreenTop,
    ScreenMiddle,
    ScreenBottom,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    /// `+` / Enter
    NextLineStart,
    /// `-`
    PrevLineStart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextObject {
    Word { big: bool },
    Quote(char),
    Bracket(char, char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
    ToggleCase,
    Lower,
    Upper,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpTarget {
    Motion(VimMotion),
    Object { obj: TextObject, inner: bool },
    /// `dd` など、カーソル行から回数分の行。
    Lines,
    /// ビジュアルモードの選択範囲。
    Visual,
    /// ビジュアルモードの選択範囲を行単位にしたもの (`D`, `Y` など)。
    VisualLines,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertAt {
    /// `i`
    Before,
    /// `a`
    After,
    /// `I`
    LineStart,
    /// `A`
    LineEnd,
    /// `o`
    LineBelow,
    /// `O`
    LineAbove,
}

/// Vim 固有の編集操作。
#[derive(Clone, Debug, PartialEq)]
pub enum VimEdit {
    Move { motion: VimMotion, count: Option<usize> },
    Operate { op: Operator, target: OpTarget, count: Option<usize> },
    /// 挿入モードに入る (Undo のまとめを開始する)。
    Insert(InsertAt),
    /// 挿入モードを抜ける (Undo のまとめを終え、キャレットを 1 文字戻す)。
    ExitInsert,
    /// ビジュアルモードに入る・種類を変える (Some)、抜ける (None)。
    Visual { linewise: Option<bool> },
    VisualSwap,
    VisualObject { obj: TextObject, inner: bool },
    /// マウスで作った選択範囲をビジュアルモードの選択にする。
    VisualFromSelection,
    Paste { text: String, linewise: bool, before: bool, count: usize },
    ReplaceChar { c: char, count: usize },
    Join { count: usize },
    ToggleCaseChars { count: usize },
}

/// ヤンク・削除した文字列。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Register {
    pub text: String,
    /// 行単位 (`yy`, `dd` など) か。貼り付け方が変わる。
    pub linewise: bool,
}

/// ビジュアルモードの状態。両端の文字を含む。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visual {
    pub anchor: usize,
    pub cursor: usize,
    pub linewise: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Normal,
    Insert,
    Visual { linewise: bool },
}

/// `.` で繰り返すために記録する入力。
#[derive(Clone, Debug)]
enum Input {
    Key(KeyChord),
    Text(String),
}

pub struct VimMode {
    state: State,
    /// 入力途中のキー列 (`2d` など)。
    pending: Vec<Tok>,
    /// Vim が扱わないキー (Ctrl+S など) の割り当て。
    fallback: Keymap,
    last_find: Option<(char, bool, bool)>,
    /// 直前の `/` `?` `*` `#` の向き (`n` `N` の向きを決める)。
    search_forward: bool,
    /// 直前の変更 (`.` で繰り返す入力列)。
    last_change: Vec<Input>,
    /// 記録中の入力列。
    recording: Option<Vec<Input>>,
    replaying: bool,
}

impl VimMode {
    pub fn new(fallback: Keymap) -> Self {
        Self {
            state: State::Normal,
            pending: Vec::new(),
            fallback,
            last_find: None,
            search_forward: true,
            last_change: Vec::new(),
            recording: None,
            replaying: false,
        }
    }

    fn feed(&mut self, input: Input) -> ModeResponse {
        match self.state {
            State::Insert => self.insert_input(input),
            _ => self.normal_input(input),
        }
    }

    fn record(&mut self, input: &Input) {
        if let Some(r) = &mut self.recording {
            r.push(input.clone());
        }
    }

    /// 記録中の入力列を「直前の変更」として確定する。
    fn commit_change(&mut self) {
        let r = self.recording.take();
        if !self.replaying {
            if let Some(r) = r {
                self.last_change = r;
            }
        }
    }

    fn insert_input(&mut self, input: Input) -> ModeResponse {
        self.record(&input);
        match input {
            Input::Text(t) => ModeResponse::Commands(vec![EditCommand::InsertText(t).into()]),
            Input::Key(k) if is_escape(k) => {
                self.state = State::Normal;
                self.commit_change();
                ModeResponse::Commands(vec![vim(VimEdit::ExitInsert)])
            }
            Input::Key(k) => match self.fallback.get(&k) {
                Some(c) => ModeResponse::Commands(vec![c.clone()]),
                None => ModeResponse::Unhandled,
            },
        }
    }

    fn normal_input(&mut self, input: Input) -> ModeResponse {
        let tok = match &input {
            Input::Text(t) => match t.chars().next() {
                Some(c) => Tok::C(c),
                None => return ModeResponse::Consumed,
            },
            Input::Key(k) if is_escape(*k) => {
                let was_pending = !self.pending.is_empty();
                self.pending.clear();
                self.recording = None;
                if matches!(self.state, State::Visual { .. }) {
                    self.state = State::Normal;
                    return ModeResponse::Commands(vec![vim(VimEdit::Visual { linewise: None })]);
                }
                return if was_pending {
                    ModeResponse::Consumed
                } else {
                    ModeResponse::Commands(vec![AppCommand::Cancel.into()])
                };
            }
            Input::Key(k) => match key_token(*k) {
                Some(t) => t,
                None if self.pending.is_empty() => {
                    let visual = matches!(self.state, State::Visual { .. });
                    return match self.fallback.get(k) {
                        Some(c) if allowed_in_normal(c, visual) => ModeResponse::Commands(vec![c.clone()]),
                        // Tab や Shift+Enter などでテキストが変わらないようにする
                        Some(_) => ModeResponse::Consumed,
                        None => ModeResponse::Unhandled,
                    };
                }
                None => {
                    self.pending.clear();
                    self.recording = None;
                    return ModeResponse::Consumed;
                }
            },
        };
        if self.pending.is_empty() && !self.replaying && self.state == State::Normal {
            self.recording = Some(Vec::new());
        }
        self.record(&input);
        self.pending.push(tok);
        let visual = matches!(self.state, State::Visual { .. });
        match parse::parse(&self.pending, visual) {
            Parsed::Incomplete => ModeResponse::Consumed,
            Parsed::Invalid => {
                self.pending.clear();
                self.recording = None;
                ModeResponse::Consumed
            }
            Parsed::Done(action) => {
                self.pending.clear();
                let cmds = self.translate(action);
                ModeResponse::Commands(cmds)
            }
        }
    }

    /// `;` `,` を直前の f/t に置き換える。
    fn repeat_find(&self, reverse: bool) -> Option<VimMotion> {
        let (c, forward, till) = self.last_find?;
        Some(VimMotion::FindChar { c, forward: forward != reverse, till, repeat: true })
    }

    fn remember_find(&mut self, m: VimMotion) {
        if let VimMotion::FindChar { c, forward, till, repeat: false } = m {
            self.last_find = Some((c, forward, till));
        }
    }

    fn translate(&mut self, action: Action) -> Vec<Command> {
        let count = action.count;
        let n = count.unwrap_or(1).max(1);
        let in_visual = matches!(self.state, State::Visual { .. });
        let operate = |this: &mut Self, op: Operator, target: OpTarget| {
            if let OpTarget::Motion(m) = target {
                this.remember_find(m);
            }
            if in_visual {
                this.state = State::Normal;
            }
            if op == Operator::Change {
                this.state = State::Insert;
            } else if op == Operator::Yank || in_visual {
                this.recording = None;
            } else {
                this.commit_change();
            }
            vec![vim(VimEdit::Operate { op, target, count })]
        };
        match action.kind {
            Kind::Motion(m) => {
                self.recording = None;
                self.remember_find(m);
                vec![vim(VimEdit::Move { motion: m, count })]
            }
            Kind::Operator { op, target } => operate(self, op, target),
            Kind::OperatorRepeatFind { op, reverse } => match self.repeat_find(reverse) {
                Some(m) => operate(self, op, OpTarget::Motion(m)),
                None => {
                    self.recording = None;
                    Vec::new()
                }
            },
            Kind::Object { obj, inner } => vec![vim(VimEdit::VisualObject { obj, inner })],
            Kind::Cmd(c) => self.translate_cmd(c, count, n, in_visual),
        }
    }

    fn translate_cmd(&mut self, c: Cmd, count: Option<usize>, n: usize, in_visual: bool) -> Vec<Command> {
        let mut keep_recording = false;
        let mut is_change = false;
        let cmds = match c {
            Cmd::Paste { before } => {
                is_change = true;
                self.state = State::Normal;
                vec![AppCommand::VimPaste { before, count: n }.into()]
            }
            Cmd::Undo => vec![EditCommand::Undo.into(); n],
            Cmd::Redo => vec![EditCommand::Redo.into(); n],
            Cmd::Join => {
                is_change = true;
                self.state = State::Normal;
                vec![vim(VimEdit::Join { count: n })]
            }
            Cmd::Replace(c) => {
                is_change = true;
                vec![vim(VimEdit::ReplaceChar { c, count: n })]
            }
            Cmd::Tilde => {
                is_change = true;
                vec![vim(VimEdit::ToggleCaseChars { count: n })]
            }
            Cmd::Repeat => {
                self.recording = None;
                return self.repeat_last_change(n);
            }
            Cmd::Insert(at) => {
                self.state = State::Insert;
                keep_recording = true;
                vec![vim(VimEdit::Insert(at))]
            }
            Cmd::Visual { linewise } => match self.state {
                State::Visual { linewise: cur } if cur == linewise => {
                    self.state = State::Normal;
                    vec![vim(VimEdit::Visual { linewise: None })]
                }
                _ => {
                    self.state = State::Visual { linewise };
                    vec![vim(VimEdit::Visual { linewise: Some(linewise) })]
                }
            },
            Cmd::VisualSwap => vec![vim(VimEdit::VisualSwap)],
            Cmd::CommandLine => {
                self.state = State::Normal;
                vec![AppCommand::VimCommandLine { visual: in_visual }.into()]
            }
            Cmd::Search { forward } => {
                self.search_forward = forward;
                vec![AppCommand::VimSearch { forward }.into()]
            }
            Cmd::SearchNext { reverse } => {
                let cmd = if self.search_forward != reverse { AppCommand::FindNext } else { AppCommand::FindPrev };
                vec![cmd.into(); n]
            }
            Cmd::SearchWord { forward } => {
                self.search_forward = forward;
                vec![AppCommand::SearchWord { forward }.into()]
            }
            Cmd::Scroll(dir) => vec![AppCommand::ScrollLines(dir * n as i32).into()],
            Cmd::RepeatFind { reverse } => match self.repeat_find(reverse) {
                Some(m) => vec![vim(VimEdit::Move { motion: m, count })],
                None => Vec::new(),
            },
        };
        if is_change && !in_visual {
            self.commit_change();
        } else if !keep_recording {
            self.recording = None;
        }
        cmds
    }

    fn repeat_last_change(&mut self, n: usize) -> Vec<Command> {
        let inputs = self.last_change.clone();
        let mut out = Vec::new();
        self.replaying = true;
        for _ in 0..n {
            for input in inputs.iter().cloned() {
                if let ModeResponse::Commands(c) = self.feed(input) {
                    out.extend(c);
                }
            }
        }
        self.replaying = false;
        self.recording = None;
        out
    }
}

/// ノーマル・ビジュアルモードで標準のキー割り当てから受け付けるコマンド
/// (ファイル操作・クリップボード・Undo など。文字の挿入や削除は受け付けない)。
fn allowed_in_normal(cmd: &Command, visual: bool) -> bool {
    match cmd {
        Command::App(_) => true,
        Command::Edit(EditCommand::Undo | EditCommand::Redo) => !visual,
        Command::Edit(EditCommand::Move { .. }) => !visual,
        Command::Edit(_) => false,
    }
}

fn vim(e: VimEdit) -> Command {
    EditCommand::Vim(e).into()
}

fn is_escape(k: KeyChord) -> bool {
    let m = k.mods;
    (k.key == Key::Escape && !m.ctrl && !m.alt && !m.shift) || (k.key == Key::Char('[') && m.ctrl && !m.alt)
}

/// ノーマル・ビジュアルモードで Vim が解釈する特殊キー。
fn key_token(k: KeyChord) -> Option<Tok> {
    let m = k.mods;
    if m.alt {
        return None;
    }
    if m.ctrl {
        return match k.key {
            Key::Char(c @ ('r' | 'd' | 'u' | 'f' | 'b' | 'e' | 'y')) if !m.shift => Some(Tok::Ctrl(c)),
            _ => None,
        };
    }
    if m.shift {
        return None;
    }
    Some(match k.key {
        Key::Left | Key::Backspace => Tok::C('h'),
        Key::Right => Tok::C('l'),
        Key::Up => Tok::C('k'),
        Key::Down => Tok::C('j'),
        Key::Home => Tok::C('0'),
        Key::End => Tok::C('$'),
        Key::Enter => Tok::C('+'),
        Key::Delete => Tok::C('x'),
        Key::PageUp => Tok::Ctrl('b'),
        Key::PageDown => Tok::Ctrl('f'),
        _ => return None,
    })
}

impl InputMode for VimMode {
    fn name(&self) -> String {
        let mode = match self.state {
            State::Normal => "NORMAL",
            State::Insert => "INSERT",
            State::Visual { linewise: false } => "VISUAL",
            State::Visual { linewise: true } => "V-LINE",
        };
        if self.pending.is_empty() {
            return mode.to_string();
        }
        let keys: String = self
            .pending
            .iter()
            .map(|t| match t {
                Tok::C(c) => c.to_string(),
                Tok::Ctrl(c) => format!("^{}", c.to_ascii_uppercase()),
            })
            .collect();
        format!("{mode}  {keys}")
    }

    fn caret_style(&self) -> CaretStyle {
        match self.state {
            State::Insert => CaretStyle::Bar,
            _ if !self.pending.is_empty() => CaretStyle::Underline,
            _ => CaretStyle::Block,
        }
    }

    fn normal_caret(&self) -> bool {
        self.state == State::Normal
    }

    fn on_key(&mut self, chord: KeyChord) -> ModeResponse {
        self.feed(Input::Key(chord))
    }

    fn on_text(&mut self, text: &str) -> ModeResponse {
        if self.state == State::Insert {
            return self.feed(Input::Text(text.to_string()));
        }
        // ノーマルモードの途中で挿入モードに入ったら、残りは挿入する
        let mut out = Vec::new();
        let mut handled = false;
        for (i, c) in text.char_indices() {
            if self.state == State::Insert {
                if let ModeResponse::Commands(cmds) = self.feed(Input::Text(text[i..].to_string())) {
                    out.extend(cmds);
                }
                break;
            }
            match self.feed(Input::Text(c.to_string())) {
                ModeResponse::Commands(cmds) => out.extend(cmds),
                ModeResponse::Consumed => handled = true,
                ModeResponse::Unhandled => {}
            }
        }
        if out.is_empty() && handled {
            ModeResponse::Consumed
        } else {
            ModeResponse::Commands(out)
        }
    }

    fn on_mouse(&mut self, has_selection: bool) -> ModeResponse {
        if self.state == State::Insert {
            return ModeResponse::Unhandled;
        }
        self.pending.clear();
        self.recording = None;
        if has_selection {
            self.state = State::Visual { linewise: false };
            ModeResponse::Commands(vec![vim(VimEdit::VisualFromSelection)])
        } else if self.state != State::Normal {
            self.state = State::Normal;
            ModeResponse::Commands(vec![vim(VimEdit::Visual { linewise: None })])
        } else {
            ModeResponse::Unhandled
        }
    }

    fn reset(&mut self) {
        self.state = State::Normal;
        self.pending.clear();
        self.recording = None;
    }

    fn shortcut_for(&self, cmd: &Command) -> Option<KeyChord> {
        self.fallback.chord_for(cmd)
    }
}

#[cfg(test)]
mod tests;
