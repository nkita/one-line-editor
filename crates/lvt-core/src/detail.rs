//! 詳細 (`*detail[内容]`) の検出とエスケープ。
//!
//! 行内の `*detail[ … ]` を「詳細」として扱い、本文では隠す (表示は `lineview`、
//! 詳細があることは UI 層が行番号の欄のバッジで示す)。内容の改行は `\n`、`]` は `\]`、`\` は `\\` で 1 行に保持する。
//! 閉じの `]` がなければ詳細ではない (通常の文字列)。1 行で最初の 1 つだけを詳細とみなす。
//! 新しく作るときは行末に置く (ファイルをそのまま読んだときに「本文 → 詳細」の順になる)。

use std::ops::Range;

use crate::buffer::TextBuffer;

/// 詳細の開始記号。
pub const PREFIX: &str = "*detail[";

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
        self.start + PREFIX.chars().count()..self.end - 1
    }

    /// 文書上の桁が詳細の内側 (両端を除く) にあるか。キャレットは置けない。
    pub fn contains_inner(&self, col: usize) -> bool {
        self.start < col && col < self.end
    }
}

/// 文字の列から詳細を探す (1 回の走査、文字列のコピーなし)。
/// 開始記号は先頭の `*` 以外に `*` を含まないので、不一致のときは `*` かどうかだけで照合をやり直せる。
pub fn find_iter(chars: impl IntoIterator<Item = char>) -> Option<Token> {
    let prefix: Vec<char> = PREFIX.chars().collect();
    let mut matched = 0;
    let mut start = None;
    let mut escaped = false;
    for (i, c) in chars.into_iter().enumerate() {
        if let Some(s) = start {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                ']' => return Some(Token { start: s, end: i + 1 }),
                _ => {}
            }
            continue;
        }
        matched = if c == prefix[matched] { matched + 1 } else { usize::from(c == '*') };
        if matched == prefix.len() {
            start = Some(i + 1 - prefix.len());
        }
    }
    None
}

/// 行の文字列から詳細を探す。
pub fn find(line: &str) -> Option<Token> {
    find_iter(line.chars())
}

/// バッファの行の詳細。
pub fn find_in_line(buf: &TextBuffer, line: usize) -> Option<Token> {
    find_iter(buf.line(line).chars())
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
        assert_eq!(find("*detail[a]rest"), Some(Token { start: 0, end: 10 }));
        assert_eq!(find("買い物 *detail[x\\]y]"), Some(Token { start: 4, end: 17 }));
        assert_eq!(find("*detail[]"), Some(Token { start: 0, end: 9 }));
        assert_eq!(find("*detail[unterminated"), None);
        assert_eq!(find("*details[x]"), None);
        assert_eq!(find("*detail[a\\\\]b]"), Some(Token { start: 0, end: 12 })); // \\ の後の ] で閉じる
        // 閉じの ] までが 1 つの詳細 (内容に *detail[ を含んでもよい)
        assert_eq!(find("*detail[ *detail[x]").map(|t| t.start), Some(0));
        assert_eq!(find("**detail[x]").map(|t| t.start), Some(1));
        let t = find("ab *detail[xyz]").unwrap();
        assert_eq!(t.content(), 11..14);
        assert!(t.contains_inner(4) && !t.contains_inner(3) && !t.contains_inner(15));
        let buf = TextBuffer::from_str("plain\n*detail[q]");
        assert_eq!(find_in_line(&buf, 0), None);
        assert_eq!(find_in_line(&buf, 1), Some(Token { start: 0, end: 10 }));
    }

    #[test]
    fn escaping() {
        let raw = "買い物\nmilk]\\done";
        let e = escape(raw);
        assert_eq!(e, "買い物\\nmilk\\]\\\\done");
        assert!(!e.contains('\n'));
        assert_eq!(unescape(&e), raw);
        assert_eq!(unescape("a\\tb"), "a\\tb"); // 未知のエスケープはそのまま
        let line = format!("title *detail[{e}]");
        let t = find(&line).unwrap();
        let content: String = line.chars().skip(t.content().start).take(t.content().len()).collect();
        assert_eq!(unescape(&content), raw);
    }
}
