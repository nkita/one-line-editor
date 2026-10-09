//! 検索・置換。
//!
//! 文書全体を 1 つの文字列にせず、行境界で区切った約 1MB のブロック単位で
//! 正規表現を適用する (メモリ使用量を抑えるため)。そのため、ブロックをまたぐ
//! 複数行マッチは検出されない (通常の 1 行内の検索には影響しない)。

use std::ops::Range;

use regex::{Regex, RegexBuilder};

use crate::buffer::TextBuffer;

const BLOCK_BYTES: usize = 1 << 20;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
}

impl SearchQuery {
    pub fn compile(&self) -> Result<Regex, regex::Error> {
        let pat = if self.regex { self.pattern.clone() } else { regex::escape(&self.pattern) };
        RegexBuilder::new(&pat).case_insensitive(!self.case_sensitive).multi_line(true).build()
    }
}

/// 行 [start, end) の内容と、その先頭のバイト位置。
fn block_text(buf: &TextBuffer, start: usize, end: usize) -> (usize, String) {
    let rope = buf.rope();
    let sb = rope.line_to_byte(start);
    let eb = rope.line_to_byte(end);
    (sb, rope.byte_slice(sb..eb).to_string())
}

/// `start` 行から始まるブロックの終端行 (排他的)。
fn block_end_from(buf: &TextBuffer, start: usize) -> usize {
    let rope = buf.rope();
    let target = (rope.line_to_byte(start) + BLOCK_BYTES).min(rope.len_bytes());
    (rope.byte_to_line(target) + 1).clamp(start + 1, buf.len_lines())
}

/// `end` 行で終わるブロックの開始行。
fn block_start_before(buf: &TextBuffer, end: usize) -> usize {
    let rope = buf.rope();
    let target = rope.line_to_byte(end).saturating_sub(BLOCK_BYTES);
    rope.byte_to_line(target).min(end - 1)
}

fn next_boundary(text: &str, i: usize) -> usize {
    i + text[i..].chars().next().map_or(1, char::len_utf8)
}

/// `text` の `from` 以降で最初の空でない一致。
fn first_match(re: &Regex, text: &str, from: usize) -> Option<Range<usize>> {
    let mut at = from;
    while at <= text.len() {
        let m = re.find_at(text, at)?;
        if !m.is_empty() {
            return Some(m.range());
        }
        if m.end() >= text.len() {
            return None;
        }
        at = next_boundary(text, m.end());
    }
    None
}

/// 文字位置 `from` 以降で最初の一致 (文字範囲)。`wrap` なら先頭に戻って続ける。
pub fn find_next(buf: &TextBuffer, re: &Regex, from: usize, wrap: bool) -> Option<Range<usize>> {
    let from = from.min(buf.len_chars());
    let from_byte = buf.char_to_byte(from);
    let total = buf.len_lines();
    let to_chars = |sb: usize, r: Range<usize>| buf.byte_to_char(sb + r.start)..buf.byte_to_char(sb + r.end);

    let mut line = buf.char_to_line(from);
    while line < total {
        let end = block_end_from(buf, line);
        let (sb, text) = block_text(buf, line, end);
        if let Some(r) = first_match(re, &text, from_byte.saturating_sub(sb).min(text.len())) {
            return Some(to_chars(sb, r));
        }
        line = end;
    }
    if !wrap {
        return None;
    }
    let mut line = 0;
    let stop = buf.char_to_line(from);
    while line <= stop && line < total {
        let end = block_end_from(buf, line);
        let (sb, text) = block_text(buf, line, end);
        if let Some(r) = first_match(re, &text, 0).filter(|r| sb + r.start < from_byte) {
            return Some(to_chars(sb, r));
        }
        line = end;
    }
    None
}

/// 文字位置 `before` より前で始まる最後の一致。`wrap` なら末尾に戻って続ける。
pub fn find_prev(buf: &TextBuffer, re: &Regex, before: usize, wrap: bool) -> Option<Range<usize>> {
    let before_byte = buf.char_to_byte(before.min(buf.len_chars()));
    let to_chars = |sb: usize, r: Range<usize>| buf.byte_to_char(sb + r.start)..buf.byte_to_char(sb + r.end);
    let search = |limit: usize, mut end: usize| {
        while end > 0 {
            let start = block_start_before(buf, end);
            let (sb, text) = block_text(buf, start, end);
            let mut last = None;
            let mut at = 0;
            while let Some(r) = first_match(re, &text, at) {
                if sb + r.start >= limit {
                    break;
                }
                at = next_boundary(&text, r.start);
                last = Some(r);
                if at > text.len() {
                    break;
                }
            }
            if let Some(r) = last {
                return Some(to_chars(sb, r));
            }
            end = start;
        }
        None
    };
    let from_line = buf.char_to_line(before.min(buf.len_chars()));
    search(before_byte, from_line + 1).or_else(|| if wrap { search(usize::MAX, buf.len_lines()) } else { None })
}

/// すべての一致を置換する編集の一覧 (文字範囲, 置換後文字列) を返す。
/// 正規表現モードでは `$1` や `${name}` でキャプチャを参照できる。
pub fn replace_all(buf: &TextBuffer, query: &SearchQuery, re: &Regex, replacement: &str) -> Vec<(Range<usize>, String)> {
    let mut edits = Vec::new();
    let mut line = 0;
    let total = buf.len_lines();
    while line < total {
        let end = block_end_from(buf, line);
        let (sb, text) = block_text(buf, line, end);
        for caps in re.captures_iter(&text) {
            let m = caps.get(0).expect("group 0 always exists");
            if m.is_empty() && !query.regex {
                continue;
            }
            let rep = if query.regex {
                let mut s = String::new();
                caps.expand(replacement, &mut s);
                s
            } else {
                replacement.to_string()
            };
            let range = buf.byte_to_char(sb + m.start())..buf.byte_to_char(sb + m.end());
            edits.push((range, rep));
        }
        line = end;
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(p: &str, regex: bool, case: bool) -> Regex {
        SearchQuery { pattern: p.into(), regex, case_sensitive: case }.compile().unwrap()
    }

    #[test]
    fn next_and_prev_with_wrap() {
        let b = TextBuffer::from_str("foo bar\nあいfoo\nbaz Foo");
        let re = q("foo", false, true);
        assert_eq!(find_next(&b, &re, 0, true), Some(0..3));
        assert_eq!(find_next(&b, &re, 1, true), Some(10..13));
        assert_eq!(find_next(&b, &re, 11, false), None);
        assert_eq!(find_next(&b, &re, 11, true), Some(0..3));
        assert_eq!(find_prev(&b, &re, 10, true), Some(0..3));
        assert_eq!(find_prev(&b, &re, 0, true), Some(10..13));
        let ci = q("foo", false, false);
        assert_eq!(find_next(&b, &ci, 11, false), Some(18..21));
    }

    #[test]
    fn regex_replace_all() {
        let b = TextBuffer::from_str("a1 b22\nc333");
        let query = SearchQuery { pattern: r"([a-z])(\d+)".into(), regex: true, case_sensitive: true };
        let edits = replace_all(&b, &query, &query.compile().unwrap(), "$2$1");
        let mut d = crate::Document::from_text("a1 b22\nc333");
        d.apply(edits, crate::history::MergeKind::None, |_, _| crate::Selection::caret(0));
        assert_eq!(d.buffer().to_string(), "1a 22b\n333c");
    }

    #[test]
    fn literal_special_chars() {
        let b = TextBuffer::from_str("a.b axb");
        assert_eq!(find_next(&b, &q("a.b", false, true), 1, true), Some(0..3));
        assert_eq!(find_next(&b, &q("a.b", true, true), 1, true), Some(4..7));
    }

    #[test]
    fn large_buffer_crosses_blocks() {
        let line = "x".repeat(1000) + "\n";
        let mut text = line.repeat(3000);
        text.push_str("needle\n");
        text.push_str(&line.repeat(10));
        let b = TextBuffer::from_str(&text);
        let re = q("needle", false, true);
        let pos = 3000 * 1001;
        assert_eq!(find_next(&b, &re, 0, false), Some(pos..pos + 6));
        assert_eq!(find_prev(&b, &re, b.len_chars(), false), Some(pos..pos + 6));
    }
}
