//! 行頭の詳細 (`*[内容]`) の検出と、折りたたみ表示の位置変換。
//!
//! 行頭 (先頭の空白・タブは許可) の `*[ … ]` を「詳細」として扱い、本文では
//! アイコン 1 文字に折りたたんで表示する。内容の改行は `\n`、`]` は `\]`、`\` は `\\` で
//! 1 行に保持する。閉じの `]` がなければ詳細ではない (通常の文字列)。
//!
//! 1 行に詳細は高々 1 つ (行頭) なので、文書上の桁 ⇔ 表示上の桁は「詳細より後ろを
//! ずらす」だけの単純な変換になる。

use std::ops::Range;

use crate::buffer::TextBuffer;

/// 折りたたんだ詳細の代わりに表示する文字。
pub const ICON: char = '📝';

/// 行内の詳細の位置 (いずれも行内の文字位置)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    /// `*` の位置。
    pub start: usize,
    /// `]` の直後。
    pub end: usize,
}

impl Token {
    /// `[` と `]` の間 (エスケープされたままの内容)。
    pub fn content(&self) -> Range<usize> {
        self.start + 2..self.end - 1
    }

    /// 文書上の桁 → 表示上の桁。詳細の内側は、`round_up` なら アイコンの後ろ、そうでなければ前。
    pub fn to_view(&self, col: usize, round_up: bool) -> usize {
        if col <= self.start {
            col
        } else if col >= self.end {
            col - (self.end - self.start) + 1
        } else if round_up {
            self.start + 1
        } else {
            self.start
        }
    }

    /// 表示上の桁 → 文書上の桁 (アイコンの後ろは詳細の直後)。
    pub fn to_doc(&self, view_col: usize) -> usize {
        if view_col <= self.start {
            view_col
        } else {
            view_col - 1 + self.end - self.start
        }
    }

    /// 文書上の桁が詳細の内側 (両端を除く) にあるか。キャレットは置けない。
    pub fn contains_inner(&self, col: usize) -> bool {
        self.start < col && col < self.end
    }
}

/// 行の文字列から詳細を探す。
pub fn find(chars: impl IntoIterator<Item = char>) -> Option<Token> {
    let mut it = chars.into_iter().enumerate().skip_while(|(_, c)| *c == ' ' || *c == '\t');
    let (start, c) = it.next()?;
    if c != '*' || it.next()?.1 != '[' {
        return None;
    }
    let mut escaped = false;
    for (i, c) in it {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            ']' => return Some(Token { start, end: i + 1 }),
            _ => {}
        }
    }
    None
}

/// バッファの行の詳細。
pub fn find_in_line(buf: &TextBuffer, line: usize) -> Option<Token> {
    find(buf.line(line).chars())
}

/// 表示用の行の文字列 (詳細をアイコンに置き換える)。
pub fn view_text(line: &str, token: Option<Token>) -> String {
    match token {
        None => line.to_string(),
        Some(t) => {
            let mut s: String = line.chars().take(t.start).collect();
            s.push(ICON);
            s.extend(line.chars().skip(t.end));
            s
        }
    }
}

/// エスケープされた内容 → 表示・編集用の文字列 (`\n` を改行に戻す)。
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some(c @ ('\\' | ']')) => out.push(c),
            Some(c) => {
                out.push('\\');
                out.push(c);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// 編集した文字列 → 1 行に保持できる内容。
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            ']' => out.push_str("\\]"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect() {
        assert_eq!(find("*[a]rest".chars()), Some(Token { start: 0, end: 4 }));
        assert_eq!(find("\t  *[x\\]y] t".chars()), Some(Token { start: 3, end: 10 }));
        assert_eq!(find("*[]".chars()), Some(Token { start: 0, end: 3 }));
        assert_eq!(find("*[unterminated".chars()), None);
        assert_eq!(find("a *[x]".chars()), None);
        assert_eq!(find("* [x]".chars()), None);
        assert_eq!(find("*[a\\\\]b]".chars()), Some(Token { start: 0, end: 6 })); // \\ の後の ] で閉じる
    }

    #[test]
    fn mapping() {
        let t = Token { start: 2, end: 8 }; // "  *[abc] x"
        assert_eq!(t.content(), 4..7);
        assert_eq!(t.to_view(0, false), 0);
        assert_eq!(t.to_view(2, false), 2);
        assert_eq!(t.to_view(5, false), 2);
        assert_eq!(t.to_view(5, true), 3);
        assert_eq!(t.to_view(8, false), 3);
        assert_eq!(t.to_view(10, false), 5);
        assert_eq!(t.to_doc(2), 2);
        assert_eq!(t.to_doc(3), 8);
        assert_eq!(t.to_doc(5), 10);
        assert_eq!(view_text("  *[abc] x", Some(t)), format!("  {ICON} x"));
        assert!(t.contains_inner(3) && !t.contains_inner(2) && !t.contains_inner(8));
    }

    #[test]
    fn escaping() {
        let raw = "買い物\nmilk]\\done";
        let e = escape(raw);
        assert_eq!(e, "買い物\\nmilk\\]\\\\done");
        assert!(!e.contains('\n'));
        assert_eq!(unescape(&e), raw);
        assert_eq!(unescape("a\\tb"), "a\\tb"); // 未知のエスケープはそのまま
        let line = format!("*[{e}] title");
        let t = find(line.chars()).unwrap();
        let content: String = line.chars().skip(t.content().start).take(t.content().len()).collect();
        assert_eq!(unescape(&content), raw);
    }
}
