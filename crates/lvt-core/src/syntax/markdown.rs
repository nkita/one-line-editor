//! Markdown の簡易ハイライタ (見出し・強調・コード・リンク・引用・リスト)。

use super::{Highlighter, LineState, StyleSpan, TokenKind};

pub struct MarkdownHighlighter;

const NORMAL: LineState = LineState(0);
const IN_FENCE: LineState = LineState(1);

impl Highlighter for MarkdownHighlighter {
    fn name(&self) -> &'static str {
        "Markdown"
    }

    fn highlight_line(&self, line: &str, state: LineState, out: &mut Vec<StyleSpan>) -> LineState {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let is_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");

        if state == IN_FENCE {
            out.push(StyleSpan { range: 0..line.len(), kind: TokenKind::Code });
            return if is_fence { NORMAL } else { IN_FENCE };
        }
        if is_fence {
            out.push(StyleSpan { range: 0..line.len(), kind: TokenKind::Code });
            return IN_FENCE;
        }
        if indent == 0 && trimmed.starts_with('#') {
            let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
            if hashes <= 6 && (trimmed.len() == hashes || trimmed[hashes..].starts_with([' ', '\t'])) {
                out.push(StyleSpan { range: 0..line.len(), kind: TokenKind::Heading });
                return NORMAL;
            }
        }
        if trimmed.starts_with('>') {
            out.push(StyleSpan { range: 0..line.len(), kind: TokenKind::Quote });
            return NORMAL;
        }
        let mut start = indent;
        if let Some(len) = list_marker_len(trimmed) {
            out.push(StyleSpan { range: indent..indent + len, kind: TokenKind::ListMarker });
            start = indent + len;
        }
        inline(line, start, out);
        NORMAL
    }
}

fn list_marker_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'-' | b'*' | b'+') && b[1] == b' ' {
        return Some(1);
    }
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits + 1 < b.len() && matches!(b[digits], b'.' | b')') && b[digits + 1] == b' ' {
        return Some(digits + 1);
    }
    None
}

/// 行内要素: `code`, **strong**, *em* / _em_, [text](url)
fn inline(line: &str, from: usize, out: &mut Vec<StyleSpan>) {
    let b = line.as_bytes();
    let mut i = from;
    while i < b.len() {
        match b[i] {
            b'`' => {
                if let Some(end) = find_from(line, i + 1, "`") {
                    out.push(StyleSpan { range: i..end + 1, kind: TokenKind::Code });
                    i = end + 1;
                    continue;
                }
            }
            b'*' | b'_' if b.get(i + 1) == Some(&b[i]) => {
                let delim = if b[i] == b'*' { "**" } else { "__" };
                if let Some(end) = find_from(line, i + 2, delim) {
                    if end > i + 2 {
                        out.push(StyleSpan { range: i..end + 2, kind: TokenKind::Strong });
                        i = end + 2;
                        continue;
                    }
                }
            }
            b'*' | b'_' => {
                let delim = if b[i] == b'*' { "*" } else { "_" };
                let word_start = i == 0 || !b[i - 1].is_ascii_alphanumeric();
                if word_start && b.get(i + 1).is_some_and(|c| !c.is_ascii_whitespace()) {
                    if let Some(end) = find_from(line, i + 1, delim) {
                        out.push(StyleSpan { range: i..end + 1, kind: TokenKind::Emphasis });
                        i = end + 1;
                        continue;
                    }
                }
            }
            b'[' => {
                if let Some(close) = find_from(line, i + 1, "](") {
                    if let Some(end) = find_from(line, close + 2, ")") {
                        out.push(StyleSpan { range: i..end + 1, kind: TokenKind::Link });
                        i = end + 1;
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn find_from(s: &str, from: usize, pat: &str) -> Option<usize> {
    s.get(from..)?.find(pat).map(|p| p + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str, st: LineState) -> (Vec<(String, TokenKind)>, LineState) {
        let mut out = Vec::new();
        let next = MarkdownHighlighter.highlight_line(line, st, &mut out);
        (out.into_iter().map(|s| (line[s.range].to_string(), s.kind)).collect(), next)
    }

    #[test]
    fn headings_and_inline() {
        assert_eq!(kinds("# 見出し", NORMAL).0, vec![("# 見出し".into(), TokenKind::Heading)]);
        let (spans, _) = kinds("- a **b** `c` [d](e)", NORMAL);
        assert_eq!(
            spans,
            vec![
                ("-".into(), TokenKind::ListMarker),
                ("**b**".into(), TokenKind::Strong),
                ("`c`".into(), TokenKind::Code),
                ("[d](e)".into(), TokenKind::Link),
            ]
        );
    }

    #[test]
    fn fences_carry_state() {
        let (_, st) = kinds("```rust", NORMAL);
        assert_eq!(st, IN_FENCE);
        let (spans, st) = kinds("# not heading", st);
        assert_eq!(spans[0].1, TokenKind::Code);
        let (_, st) = kinds("```", st);
        assert_eq!(st, NORMAL);
    }
}
