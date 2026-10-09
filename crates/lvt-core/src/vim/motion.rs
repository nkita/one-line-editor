//! Vim のモーションとテキストオブジェクトの範囲計算 (文書を変更しない純粋な関数)。

use std::ops::Range;

use crate::buffer::TextBuffer;
use crate::editor::{current_display_col, pos_at_display_col, EditorConfig, ViewInfo};
use crate::width::{char_class, CharClass};

use super::{TextObject, VimMotion};

/// モーションの種類 (演算子を適用する範囲の決め方)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionKind {
    /// 移動先の文字を含まない。
    Exclusive,
    /// 移動先の文字を含む。
    Inclusive,
    /// 行全体。
    Linewise,
}

impl VimMotion {
    pub fn kind(&self) -> MotionKind {
        use VimMotion as M;
        match self {
            M::Left
            | M::Right
            | M::WordStart { .. }
            | M::WordBack { .. }
            | M::LineStart
            | M::FirstNonBlank
            | M::ParagraphForward
            | M::ParagraphBack => MotionKind::Exclusive,
            M::WordEnd { .. } | M::LineEnd | M::FindChar { .. } | M::MatchPair => MotionKind::Inclusive,
            M::Up
            | M::Down
            | M::GotoLine { .. }
            | M::ScreenTop
            | M::ScreenMiddle
            | M::ScreenBottom
            | M::HalfPageDown
            | M::HalfPageUp
            | M::PageDown
            | M::PageUp
            | M::NextLineStart
            | M::PrevLineStart => MotionKind::Linewise,
        }
    }

    /// 上下移動 (表示桁を維持する)。
    fn keeps_column(&self) -> bool {
        use VimMotion as M;
        matches!(self, M::Up | M::Down | M::HalfPageDown | M::HalfPageUp | M::PageDown | M::PageUp)
    }
}

/// 移動先と、以後の上下移動で維持する表示桁。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Jump {
    pub pos: usize,
    pub pref: Option<usize>,
}

pub struct Ctx<'a> {
    pub buf: &'a TextBuffer,
    pub cfg: &'a EditorConfig,
    pub view: &'a ViewInfo,
    /// 現在維持している表示桁。
    pub pref: Option<usize>,
    /// 装飾表示か (詳細の折りたたみや記号の非表示で、表示桁の数え方が変わる)。
    pub detail_view: bool,
}

/// `pos` から `motion` を `count` 回 (G などでは行番号) 適用した移動先。移動できなければ None。
pub fn target(ctx: &Ctx, pos: usize, motion: VimMotion, count: Option<usize>) -> Option<Jump> {
    use VimMotion as M;
    let buf = ctx.buf;
    let n = count.unwrap_or(1).max(1);
    let (line, _) = buf.line_col(pos);
    let last_line = buf.len_lines() - 1;
    let tab = ctx.cfg.tab_width;
    let dv = ctx.detail_view;
    let pref = ctx.pref.unwrap_or_else(|| current_display_col(buf, pos, tab, dv));
    let to_line = |l: usize| first_nonblank(buf, l.min(last_line));
    let pos = match motion {
        M::Left => {
            let start = buf.line_to_char(line);
            if pos == start {
                return None;
            }
            pos.saturating_sub(n).max(start)
        }
        M::Right => {
            let end = buf.line_end(line);
            if pos >= end {
                return None;
            }
            (pos + n).min(end)
        }
        M::Up | M::HalfPageUp | M::PageUp => {
            let step = match motion {
                M::Up => n,
                M::HalfPageUp => (ctx.view.page_lines / 2).max(1) * n,
                _ => ctx.view.page_lines.max(1) * n,
            };
            if line == 0 {
                return None;
            }
            pos_at_display_col(buf, line.saturating_sub(step), pref, tab, dv)
        }
        M::Down | M::HalfPageDown | M::PageDown => {
            let step = match motion {
                M::Down => n,
                M::HalfPageDown => (ctx.view.page_lines / 2).max(1) * n,
                _ => ctx.view.page_lines.max(1) * n,
            };
            if line >= last_line {
                return None;
            }
            pos_at_display_col(buf, (line + step).min(last_line), pref, tab, dv)
        }
        M::WordStart { big } => repeat(pos, n, |p| next_word_start(buf, p, big)),
        M::WordEnd { big } => repeat(pos, n, |p| next_word_end(buf, p, big)),
        M::WordBack { big } => repeat(pos, n, |p| prev_word_start(buf, p, big)),
        M::LineStart => buf.line_to_char(line),
        M::FirstNonBlank => first_nonblank(buf, line),
        M::LineEnd => {
            let l = (line + n - 1).min(last_line);
            return Some(Jump { pos: last_visible_char(buf, l, ctx.detail_view), pref: Some(usize::MAX) });
        }
        M::GotoLine { last } => match count {
            Some(c) => to_line(c.max(1) - 1),
            None if last => to_line(last_line),
            None => to_line(0),
        },
        M::FindChar { c, forward, till, repeat: rep } => find_char(buf, pos, c, forward, till, rep, n)?,
        M::MatchPair => match_pair(buf, pos)?,
        M::ParagraphForward => repeat(pos, n, |p| paragraph(buf, p, true)),
        M::ParagraphBack => repeat(pos, n, |p| paragraph(buf, p, false)),
        M::ScreenTop => to_line(ctx.view.top_line + n - 1),
        M::ScreenBottom => {
            let bottom = (ctx.view.top_line + ctx.view.page_lines.max(1) - 1).min(last_line);
            to_line(bottom.saturating_sub(n - 1).max(ctx.view.top_line))
        }
        M::ScreenMiddle => {
            let bottom = (ctx.view.top_line + ctx.view.page_lines.max(1) - 1).min(last_line);
            to_line((ctx.view.top_line + bottom) / 2)
        }
        M::NextLineStart => {
            if line >= last_line {
                return None;
            }
            to_line(line + n)
        }
        M::PrevLineStart => {
            if line == 0 {
                return None;
            }
            to_line(line.saturating_sub(n))
        }
    };
    let pref = motion.keeps_column().then_some(pref);
    Some(Jump { pos, pref })
}

fn repeat(pos: usize, n: usize, f: impl Fn(usize) -> usize) -> usize {
    (0..n).fold(pos, |p, _| f(p))
}

fn ch(buf: &TextBuffer, p: usize) -> char {
    buf.rope().char(p)
}

/// 行の最初の空白でない文字 (空白だけの行なら行末)。
pub fn first_nonblank(buf: &TextBuffer, line: usize) -> usize {
    let start = buf.line_to_char(line);
    start + buf.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// 行の最後の文字 (空行なら行頭)。ノーマルモードのキャレットが置ける最も右。
pub fn last_char(buf: &TextBuffer, line: usize) -> usize {
    let end = buf.line_end(line);
    if buf.line_len(line) > 0 {
        end - 1
    } else {
        end
    }
}

/// 行の最後の見える文字。装飾表示で行末に詳細 (見えない) があるときは、その手前の文字。
pub fn last_visible_char(buf: &TextBuffer, line: usize, detail_view: bool) -> usize {
    let ls = buf.line_to_char(line);
    match detail_view.then(|| crate::detail::find_in_line(buf, line)).flatten() {
        Some(t) if ls + t.end == buf.line_end(line) => ls + t.start.saturating_sub(1),
        _ => last_char(buf, line),
    }
}

/// 単語の種類。0 は空白 (改行を含む)。`big` (W/B/E) では空白以外をすべて同じ種類とみなす。
fn cls(c: char, big: bool) -> u8 {
    match char_class(c) {
        CharClass::Space | CharClass::Newline => 0,
        _ if big => 1,
        CharClass::Punct => 1,
        CharClass::Word => 2,
        CharClass::Hiragana => 3,
        CharClass::Katakana => 4,
        CharClass::Kanji => 5,
    }
}

/// `p` が空行の上にあるか (Vim では空行も 1 単語として扱う)。
fn is_empty_line_at(buf: &TextBuffer, p: usize) -> bool {
    ch(buf, p) == '\n' && (p == 0 || ch(buf, p - 1) == '\n')
}

/// `w`: 次の単語の先頭。
pub fn next_word_start(buf: &TextBuffer, mut p: usize, big: bool) -> usize {
    let len = buf.len_chars();
    if p >= len {
        return len;
    }
    let start = cls(ch(buf, p), big);
    p += 1;
    if start != 0 {
        while p < len && cls(ch(buf, p), big) == start {
            p += 1;
        }
    }
    while p < len && cls(ch(buf, p), big) == 0 && !is_empty_line_at(buf, p) {
        p += 1;
    }
    p
}

/// `e`: 単語の末尾 (少なくとも 1 文字進む)。
pub fn next_word_end(buf: &TextBuffer, p: usize, big: bool) -> usize {
    let len = buf.len_chars();
    let mut p = p + 1;
    while p < len && cls(ch(buf, p), big) == 0 {
        p += 1;
    }
    if p >= len {
        return len.saturating_sub(1);
    }
    let c = cls(ch(buf, p), big);
    while p + 1 < len && cls(ch(buf, p + 1), big) == c {
        p += 1;
    }
    p
}

/// `b`: 前の単語の先頭。
pub fn prev_word_start(buf: &TextBuffer, p: usize, big: bool) -> usize {
    if p == 0 {
        return 0;
    }
    let mut p = p - 1;
    while p > 0 && cls(ch(buf, p), big) == 0 && !is_empty_line_at(buf, p) {
        p -= 1;
    }
    let c = cls(ch(buf, p), big);
    if c != 0 {
        while p > 0 && cls(ch(buf, p - 1), big) == c {
            p -= 1;
        }
    }
    p
}

/// 演算子と組み合わせた `w` の移動先。最後に越えた単語が行末にあれば、次の行へは進まない。
pub fn word_end_for_operator(buf: &TextBuffer, pos: usize, n: usize, big: bool) -> usize {
    let mut p = pos;
    let mut prev = pos;
    for _ in 0..n {
        prev = p;
        p = next_word_start(buf, p, big);
    }
    let prev_line = buf.char_to_line(prev);
    if buf.char_to_line(p) > prev_line {
        let end = buf.line_end(prev_line);
        if end > prev {
            return end;
        }
    }
    p
}

fn find_char(buf: &TextBuffer, pos: usize, c: char, forward: bool, till: bool, repeat: bool, n: usize) -> Option<usize> {
    let (line, col) = buf.line_col(pos);
    let start = buf.line_to_char(line);
    let chars: Vec<char> = buf.line(line).chars().collect();
    // `;` で t を繰り返すときは、すぐ隣の文字は飛ばす (同じ位置に留まらないように)
    let skip = usize::from(till && repeat);
    let mut found = 0;
    if forward {
        for (i, &x) in chars.iter().enumerate().skip(col + 1 + skip) {
            if x == c {
                found += 1;
                if found == n {
                    return Some(start + if till { i - 1 } else { i });
                }
            }
        }
    } else {
        for i in (0..col.saturating_sub(skip)).rev() {
            if chars[i] == c {
                found += 1;
                if found == n {
                    return Some(start + if till { i + 1 } else { i });
                }
            }
        }
    }
    None
}

const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// `%`: カーソル位置以降で行内の最初の括弧と対になる括弧。
fn match_pair(buf: &TextBuffer, pos: usize) -> Option<usize> {
    let line = buf.char_to_line(pos);
    let end = buf.line_end(line);
    let p = (pos..end).find(|&p| PAIRS.iter().any(|&(o, c)| ch(buf, p) == o || ch(buf, p) == c))?;
    let c = ch(buf, p);
    let &(open, close) = PAIRS.iter().find(|&&(o, cl)| c == o || c == cl)?;
    if c == open {
        find_close(buf, p + 1, open, close)
    } else {
        find_open(buf, p, open, close)
    }
}

/// `from` 以降で、深さ 0 の閉じ括弧。
fn find_close(buf: &TextBuffer, from: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in buf.rope().chars_at(from).enumerate() {
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                return Some(from + i);
            }
            depth -= 1;
        }
    }
    None
}

/// `before` より前で、深さ 0 の開き括弧。
fn find_open(buf: &TextBuffer, before: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut it = buf.rope().chars_at(before);
    let mut p = before;
    while let Some(c) = it.prev() {
        p -= 1;
        if c == close {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                return Some(p);
            }
            depth -= 1;
        }
    }
    None
}

/// `}` / `{`: 次 (前) の空行。なければ文書の端。
fn paragraph(buf: &TextBuffer, pos: usize, forward: bool) -> usize {
    let empty = |l: usize| buf.line_len(l) == 0;
    let last = buf.len_lines() - 1;
    let mut l = buf.char_to_line(pos);
    if forward {
        while l < last && empty(l) {
            l += 1;
        }
        while l < last && !empty(l) {
            l += 1;
        }
        if empty(l) {
            buf.line_to_char(l)
        } else {
            buf.len_chars()
        }
    } else {
        while l > 0 && empty(l) {
            l -= 1;
        }
        while l > 0 && !empty(l) {
            l -= 1;
        }
        buf.line_to_char(l)
    }
}

// ---- テキストオブジェクト ----

/// テキストオブジェクトの範囲 (文字単位、終端を含まない)。
pub fn object_range(buf: &TextBuffer, pos: usize, obj: TextObject, inner: bool, count: usize) -> Option<Range<usize>> {
    match obj {
        TextObject::Word { big } => word_object(buf, pos, big, inner, count),
        TextObject::Quote(q) => quote_object(buf, pos, q, inner),
        TextObject::Bracket(open, close) => bracket_object(buf, pos, open, close, inner, count),
    }
}

fn word_object(buf: &TextBuffer, pos: usize, big: bool, inner: bool, count: usize) -> Option<Range<usize>> {
    let line = buf.char_to_line(pos);
    let (ls, le) = (buf.line_to_char(line), buf.line_end(line));
    if ls == le {
        return None;
    }
    let pos = pos.min(le - 1);
    // 行内で同じ種類が続く範囲
    let run = |p: usize| {
        let c = cls(ch(buf, p), big);
        let mut s = p;
        while s > ls && cls(ch(buf, s - 1), big) == c {
            s -= 1;
        }
        let mut e = p + 1;
        while e < le && cls(ch(buf, e), big) == c {
            e += 1;
        }
        s..e
    };
    let first = run(pos);
    let mut start = first.start;
    let mut end = first.end;
    let on_blank = cls(ch(buf, pos), big) == 0;
    let mut runs = if inner { count } else { count * 2 };
    runs = runs.saturating_sub(1);
    for _ in 0..runs {
        if end >= le {
            break;
        }
        end = run(end).end;
    }
    if !inner && !on_blank && (end == first.end || cls(ch(buf, end - 1), big) != 0) {
        // aw で後ろに空白がなければ前の空白を含める
        let mut s = start;
        while s > ls && cls(ch(buf, s - 1), big) == 0 {
            s -= 1;
        }
        start = s;
    }
    Some(start..end)
}

fn quote_object(buf: &TextBuffer, pos: usize, q: char, inner: bool) -> Option<Range<usize>> {
    let (line, col) = buf.line_col(pos);
    let ls = buf.line_to_char(line);
    let chars: Vec<char> = buf.line(line).chars().collect();
    let quotes: Vec<usize> =
        (0..chars.len()).filter(|&i| chars[i] == q && (i == 0 || chars[i - 1] != '\\')).collect();
    let (open, close) = quotes
        .chunks_exact(2)
        .map(|w| (w[0], w[1]))
        .find(|&(_, c)| col <= c)?;
    if inner {
        return Some(ls + open + 1..ls + close);
    }
    let mut end = close + 1;
    while end < chars.len() && chars[end] == ' ' {
        end += 1;
    }
    let mut start = open;
    if end == close + 1 {
        while start > 0 && chars[start - 1] == ' ' {
            start -= 1;
        }
    }
    Some(ls + start..ls + end)
}

fn bracket_object(buf: &TextBuffer, pos: usize, open: char, close: char, inner: bool, count: usize) -> Option<Range<usize>> {
    let mut o = match buf.char(pos) {
        Some(c) if c == open => pos,
        Some(c) if c == close => find_open(buf, pos, open, close)?,
        _ => find_open(buf, pos, open, close)?,
    };
    for _ in 1..count {
        o = find_open(buf, o, open, close)?;
    }
    let c = find_close(buf, o + 1, open, close)?;
    if !inner {
        return Some(o..c + 1);
    }
    let mut start = o + 1;
    let mut end = c;
    // 括弧の直後の改行と、閉じ括弧の前の (インデントだけの) 行頭は含めない
    if buf.char(start) == Some('\n') {
        start += 1;
    }
    let cl = buf.char_to_line(c);
    if cl > buf.char_to_line(o) && buf.line(cl).chars().take(c - buf.line_to_char(cl)).all(|x| x == ' ' || x == '\t') {
        end = buf.line_to_char(cl);
    }
    Some(start..end.max(start))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> TextBuffer {
        TextBuffer::from_str(s)
    }

    #[test]
    fn words() {
        let t = b("foo.bar  baz\n\nqux");
        assert_eq!(next_word_start(&t, 0, false), 3); // foo|.
        assert_eq!(next_word_start(&t, 3, false), 4);
        assert_eq!(next_word_start(&t, 4, false), 9);
        assert_eq!(next_word_start(&t, 0, true), 9);
        assert_eq!(next_word_start(&t, 9, false), 13); // 空行で止まる
        assert_eq!(next_word_start(&t, 13, false), 14);
        assert_eq!(next_word_end(&t, 0, false), 2);
        assert_eq!(next_word_end(&t, 2, false), 3);
        assert_eq!(next_word_end(&t, 0, true), 6);
        assert_eq!(prev_word_start(&t, 14, false), 13);
        assert_eq!(prev_word_start(&t, 13, false), 9);
        assert_eq!(prev_word_start(&t, 9, false), 4);
        assert_eq!(word_end_for_operator(&t, 9, 1, false), 12);
    }

    #[test]
    fn japanese_words() {
        let t = b("今日はRustで書く");
        assert_eq!(next_word_start(&t, 0, false), 2);
        assert_eq!(next_word_start(&t, 2, false), 3);
        assert_eq!(next_word_end(&t, 3, false), 6);
    }

    #[test]
    fn find_and_pairs() {
        let t = b("a(b[c]d)e");
        assert_eq!(find_char(&t, 0, 'd', true, false, false, 1), Some(6));
        assert_eq!(find_char(&t, 0, 'd', true, true, false, 1), Some(5));
        assert_eq!(find_char(&t, 6, 'b', false, false, false, 1), Some(2));
        assert_eq!(find_char(&t, 6, 'b', false, true, false, 1), Some(3));
        assert_eq!(find_char(&t, 0, 'z', true, false, false, 1), None);
        assert_eq!(match_pair(&t, 0), Some(7));
        assert_eq!(match_pair(&t, 7), Some(1));
        assert_eq!(match_pair(&t, 3), Some(5));
    }

    #[test]
    fn objects() {
        let t = b("say \"hi there\" (a (b) c)");
        assert_eq!(object_range(&t, 0, TextObject::Word { big: false }, true, 1), Some(0..3));
        assert_eq!(object_range(&t, 0, TextObject::Word { big: false }, false, 1), Some(0..4));
        assert_eq!(object_range(&t, 6, TextObject::Quote('"'), true, 1), Some(5..13));
        assert_eq!(object_range(&t, 6, TextObject::Quote('"'), false, 1), Some(4..15));
        assert_eq!(object_range(&t, 0, TextObject::Quote('"'), true, 1), Some(5..13));
        assert_eq!(object_range(&t, 19, TextObject::Bracket('(', ')'), true, 1), Some(19..20));
        assert_eq!(object_range(&t, 19, TextObject::Bracket('(', ')'), true, 2), Some(16..23));
        assert_eq!(object_range(&t, 16, TextObject::Bracket('(', ')'), false, 1), Some(15..24));
        let m = b("f(\n    x\n)");
        assert_eq!(object_range(&m, 4, TextObject::Bracket('(', ')'), true, 1), Some(3..9));
    }

    #[test]
    fn paragraphs() {
        let t = b("a\nb\n\nc\n\n\nd");
        assert_eq!(paragraph(&t, 0, true), 4);
        assert_eq!(paragraph(&t, 4, true), 7);
        assert_eq!(paragraph(&t, 9, false), 8);
        assert_eq!(paragraph(&t, 2, false), 0);
    }
}
