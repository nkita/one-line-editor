//! 画面下部の 1 行入力欄 (検索・置換・行移動)。
//!
//! 将来 Vim の `:` コマンドラインもこの仕組みを流用する想定。

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Find,
    /// 置換: 検索文字列の入力
    ReplaceFind,
    /// 置換: 置換後文字列の入力
    ReplaceWith { find: String },
    GoToLine,
}

pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
    /// 入力欄内のキャレット (バイト位置)
    pub cursor: usize,
    /// 入力欄を開いた時点のキャレット (インクリメンタル検索の起点)
    pub origin: usize,
}

impl Prompt {
    pub fn new(kind: PromptKind, initial: String, origin: usize) -> Self {
        let cursor = initial.len();
        Self { kind, text: initial, cursor, origin }
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            PromptKind::Find => "検索",
            PromptKind::ReplaceFind => "置換する文字列",
            PromptKind::ReplaceWith { .. } => "置換後の文字列",
            PromptKind::GoToLine => "行番号 (行:桁)",
        }
    }

    pub fn is_search(&self) -> bool {
        matches!(self.kind, PromptKind::Find | PromptKind::ReplaceFind)
    }

    pub fn insert(&mut self, s: &str) {
        let s = s.replace(['\r', '\n'], "");
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    pub fn backspace(&mut self) -> bool {
        let Some(c) = self.text[..self.cursor].chars().next_back() else { return false };
        self.cursor -= c.len_utf8();
        self.text.remove(self.cursor);
        true
    }

    pub fn delete(&mut self) -> bool {
        if self.cursor >= self.text.len() {
            return false;
        }
        self.text.remove(self.cursor);
        true
    }

    pub fn left(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
        }
    }

    pub fn right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.len();
    }
}
