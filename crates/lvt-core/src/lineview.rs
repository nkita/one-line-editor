//! 1 行の表示モデル (.txt / .lvt の装飾表示)。
//!
//! 文書上の行から、表示用の文字列・書式・文書上の桁 ⇔ 表示上の桁の対応を作る。
//! - 詳細 `*detail[…]` は隠す (幅 0)。詳細があることは行番号の欄のバッジで示す (UI 層)。
//! - `[label](url)` は label だけ、`` `code` `` と `**強調**` は記号を隠して表示する。
//! - 行頭の `#`〜`######` + 空白は見出し。記号を隠して太字にする (背景の帯は UI 層が描く)。
//!   キャレットが要素の上 (両端を含む) にあるときは、その要素だけ元の記法のまま表示する。
//! - 中身が空の要素 (`****`、`` `` ``、`[]()`) は、空の位置に薄い文字のプレースホルダを表示する
//!   (表示だけで文書には入らない。キャレットはプレースホルダの前に置く)。
//! - 入力中の開き記号 (`**`、`` ` ``、`[`) の直後では、残りの記法を薄い文字 (ゴースト) で示す
//!   (`with_completion`)。Tab で閉じ記号を入れ (`completion_at`)、要素の中の Tab は次の位置へ進む (`tab_stop`)。
//!
//! 状態は持たず、描画・クリック判定・上下移動のたびに作る (見えている行だけなので軽い)。

use std::ops::Range;

use crate::detail::{self, Token};
use crate::syntax::{StyleSpan, TokenKind};

/// 文書上の範囲 `doc` を、表示では `view_len` 文字 (0 = 隠す) にする置き換え。
/// `doc` が空の範囲ならプレースホルダ (表示だけに文字を足す)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fold {
    doc: (usize, usize),
    view_len: usize,
    /// 要素の閉じ記号か。表示上の境界は閉じ記号の後ろに対応させる (キャレットが要素の外に出るように)。
    closing: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineView {
    /// 表示用の文字列。
    pub text: String,
    /// 表示用の文字列に対する書式 (バイト範囲)。
    pub styles: Vec<StyleSpan>,
    /// `[label](url)` の label の表示上のバイト範囲と url (Ctrl+クリックで開く)。
    pub links: Vec<(Range<usize>, String)>,
    /// 行の詳細 (文書上の位置)。
    pub detail: Option<Token>,
    /// 見出しのレベル (1〜6)。
    pub heading: Option<u8>,
    folds: Vec<Fold>,
}

/// 装飾の要素 (行内の文字位置。`end` は終端の直後)。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Element {
    start: usize,
    end: usize,
    kind: ElementKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ElementKind {
    /// `` `…` `` (記号は 1 文字ずつ)
    Code,
    /// `**…**` (記号は 2 文字ずつ)
    Strong,
    /// `[label](url)`。`label_end` は `]` の位置。
    Link { label_end: usize, url: String },
}

/// 入力中の記法の補完。Tab でキャレット位置に `insert` を入れる (キャレットは `insert` の前に残る)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    pub insert: &'static str,
    /// 補完の候補として薄く表示する文字列。
    pub ghost: &'static str,
}

/// 空の要素に表示するプレースホルダ。
pub const PLACEHOLDER_STRONG: &str = "強調";
pub const PLACEHOLDER_CODE: &str = "コード";
pub const PLACEHOLDER_LABEL: &str = "リンクテキスト";
pub const PLACEHOLDER_URL: &str = "URL";

impl LineView {
    /// 装飾なし (そのまま表示する)。
    pub fn plain(line: &str) -> Self {
        Self { text: line.to_string(), ..Default::default() }
    }

    /// 装飾表示の行を作る。`caret` はキャレットがこの行にあるときの行内の文字位置。
    pub fn build(line: &str, caret: Option<usize>) -> Self {
        let chars: Vec<char> = line.chars().collect();
        let token = detail::find_iter(chars.iter().copied());
        let elements = parse(&chars, token);
        let mut folds = Vec::new();
        // 表示上の文字位置で書式を記録し、最後にバイト範囲へ直す
        let mut char_styles: Vec<(Range<usize>, TokenKind)> = Vec::new();
        let mut char_links: Vec<(Range<usize>, String)> = Vec::new();
        let mut items: Vec<(usize, Option<&Element>)> = elements.iter().map(|e| (e.start, Some(e))).collect();
        if let Some(t) = token {
            items.push((t.start, None));
        }
        items.sort_by_key(|i| i.0);

        let mut view = String::with_capacity(line.len());
        let mut vlen = 0; // 表示上の文字数
        let mut pos = 0; // 文書上の文字位置
        let push_raw = |view: &mut String, vlen: &mut usize, r: Range<usize>| {
            view.extend(&chars[r.clone()]);
            *vlen += r.len();
        };
        // 文書上の位置 `at` (空の範囲) に、表示だけのプレースホルダを置く
        let push_placeholder = |view: &mut String,
                                vlen: &mut usize,
                                folds: &mut Vec<Fold>,
                                styles: &mut Vec<(Range<usize>, TokenKind)>,
                                at: usize,
                                text: &str| {
            let n = text.chars().count();
            view.push_str(text);
            folds.push(Fold { doc: (at, at), view_len: n, closing: false });
            styles.push((*vlen..*vlen + n, TokenKind::Placeholder));
            *vlen += n;
        };
        // 見出し: 行頭の記号を隠す (キャレットがこの行にあれば記号を見せる)
        let heading = heading_prefix(&chars);
        if let Some((_, prefix)) = heading {
            if caret.is_some() {
                push_raw(&mut view, &mut vlen, 0..prefix);
                char_styles.push((0..prefix, TokenKind::Punctuation));
            } else {
                folds.push(Fold { doc: (0, prefix), view_len: 0, closing: false });
            }
            pos = prefix;
        }
        let heading_text_start = vlen;
        for (start, el) in items {
            push_raw(&mut view, &mut vlen, pos..start);
            match el {
                None => {
                    let t = token.expect("詳細");
                    folds.push(Fold { doc: (t.start, t.end), view_len: 0, closing: false });
                    pos = t.end;
                }
                Some(e) => {
                    let expanded = caret.is_some_and(|c| e.start <= c && c <= e.end);
                    let (open, close) = match &e.kind {
                        ElementKind::Code => (1, 1),
                        ElementKind::Strong => (2, 2),
                        ElementKind::Link { label_end, .. } => (1, e.end - label_end),
                    };
                    let (kind, placeholder) = match e.kind {
                        ElementKind::Code => (TokenKind::Code, PLACEHOLDER_CODE),
                        ElementKind::Strong => (TokenKind::Strong, PLACEHOLDER_STRONG),
                        ElementKind::Link { .. } => (TokenKind::Link, PLACEHOLDER_LABEL),
                    };
                    let inner = e.start + open..e.end - close;
                    if expanded {
                        let v0 = vlen;
                        push_raw(&mut view, &mut vlen, e.start..inner.start);
                        char_styles.push((v0..vlen, TokenKind::Punctuation));
                        let v1 = vlen;
                        if inner.is_empty() {
                            push_placeholder(&mut view, &mut vlen, &mut folds, &mut char_styles, inner.start, placeholder);
                        } else {
                            push_raw(&mut view, &mut vlen, inner.clone());
                            char_styles.push((v1..vlen, kind));
                        }
                        let v2 = vlen;
                        match &e.kind {
                            // URL が空なら `](` と `)` の間にもプレースホルダ
                            ElementKind::Link { label_end, url } if url.is_empty() => {
                                push_raw(&mut view, &mut vlen, inner.end..label_end + 2);
                                char_styles.push((v2..vlen, TokenKind::Punctuation));
                                push_placeholder(&mut view, &mut vlen, &mut folds, &mut char_styles, label_end + 2, PLACEHOLDER_URL);
                                let v3 = vlen;
                                push_raw(&mut view, &mut vlen, label_end + 2..e.end);
                                char_styles.push((v3..vlen, TokenKind::Punctuation));
                            }
                            _ => {
                                push_raw(&mut view, &mut vlen, inner.end..e.end);
                                char_styles.push((v2..vlen, TokenKind::Punctuation));
                            }
                        }
                    } else {
                        folds.push(Fold { doc: (e.start, inner.start), view_len: 0, closing: false });
                        let v0 = vlen;
                        if inner.is_empty() {
                            push_placeholder(&mut view, &mut vlen, &mut folds, &mut char_styles, inner.start, placeholder);
                        } else {
                            push_raw(&mut view, &mut vlen, inner.clone());
                            char_styles.push((v0..vlen, kind));
                        }
                        folds.push(Fold { doc: (inner.end, e.end), view_len: 0, closing: true });
                        if let ElementKind::Link { url, .. } = &e.kind {
                            if !url.is_empty() {
                                char_links.push((v0..vlen, url.clone()));
                            }
                        }
                    }
                    pos = e.end;
                }
            }
        }
        push_raw(&mut view, &mut vlen, pos..chars.len());
        if heading.is_some() {
            // 見出しの本文全体を太字にする (先頭に入れて、コードなどの書式を後から重ねる)
            char_styles.insert(0, (heading_text_start..vlen, TokenKind::Strong));
        }

        let to_byte = |s: &str, c: usize| s.char_indices().nth(c).map_or(s.len(), |(b, _)| b);
        let byte_range = |r: &Range<usize>| to_byte(&view, r.start)..to_byte(&view, r.end);
        let styles =
            char_styles.iter().filter(|(r, _)| !r.is_empty()).map(|(r, k)| StyleSpan { range: byte_range(r), kind: *k }).collect();
        let links = char_links.iter().map(|(r, u)| (byte_range(r), u.clone())).collect();
        Self { styles, links, detail: token, heading: heading.map(|h| h.0), folds, text: view }
    }

    /// 文書上の桁 → 表示上の桁。隠した範囲の内側は、`round_up` なら置き換えの後ろ、そうでなければ前。
    pub fn to_view(&self, col: usize, round_up: bool) -> usize {
        let mut shift = 0isize; // 文書上の桁 − 表示上の桁 (プレースホルダがあると負にもなる)
        for f in &self.folds {
            let (s, e) = f.doc;
            if col <= s {
                break;
            }
            if col < e {
                return (s as isize - shift) as usize + if round_up { f.view_len } else { 0 };
            }
            shift += (e - s) as isize - f.view_len as isize;
        }
        (col as isize - shift) as usize
    }

    /// 表示上の桁 → 文書上の桁。置き換え (アイコン) の後ろは、置き換えた範囲の直後。
    /// 隠した記号の位置では、開き記号なら記号の前、閉じ記号なら記号の後ろ (要素の外側) にする。
    pub fn to_doc(&self, vcol: usize) -> usize {
        let mut shift = 0isize;
        for f in &self.folds {
            let (s, e) = f.doc;
            let vs = (s as isize - shift) as usize;
            if vcol < vs || (vcol == vs && !(f.closing && f.view_len == 0)) {
                break;
            }
            if vcol < vs + f.view_len {
                return e;
            }
            shift += (e - s) as isize - f.view_len as isize;
        }
        (vcol as isize + shift) as usize
    }
}

impl LineView {
    /// キャレットの位置に補完の候補があれば、ゴーストとして表示に足す (表示だけ。幅 0 の置き換え)。
    pub fn with_completion(mut self, line: &str, caret: usize) -> Self {
        let Some(c) = completion_at(line, caret) else { return self };
        let vcol = self.to_view(caret, false);
        let at = self.text.char_indices().nth(vcol).map_or(self.text.len(), |(b, _)| b);
        let n = c.ghost.len();
        self.text.insert_str(at, c.ghost);
        let shift = |r: &mut Range<usize>| {
            if r.start >= at {
                r.start += n;
            }
            if r.end > at {
                r.end += n;
            }
        };
        self.styles.iter_mut().for_each(|s| shift(&mut s.range));
        self.links.iter_mut().for_each(|l| shift(&mut l.0));
        self.styles.push(StyleSpan { range: at..at + n, kind: TokenKind::Placeholder });
        let i = self.folds.partition_point(|f| f.doc.0 < caret);
        self.folds.insert(i, Fold { doc: (caret, caret), view_len: c.ghost.chars().count(), closing: false });
        self
    }
}

/// キャレットの直前が入力中の開き記号 (`**`、`` ` ``、`[`) なら、その補完。
/// キャレットの後ろが行末か空白のときだけ。既にある要素の記号や詳細 (`*detail[`) は対象にしない。
pub fn completion_at(line: &str, caret: usize) -> Option<Completion> {
    let chars: Vec<char> = line.chars().collect();
    let c = caret;
    if c == 0 || c > chars.len() || chars.get(c).is_some_and(|ch| !ch.is_whitespace()) {
        return None;
    }
    let token = detail::find_iter(chars.iter().copied());
    let elements = parse(&chars, token);
    let free = |i: usize| {
        !elements.iter().any(|e| e.start <= i && i < e.end) && !token.is_some_and(|t| t.start <= i && i < t.end)
    };
    let before = |n: usize, ch: char| c >= n && chars[c - n..c].iter().all(|&x| x == ch) && (c == n || chars[c - n - 1] != ch);
    if before(2, '*') && free(c - 2) {
        return Some(Completion { insert: "**", ghost: "強調**" });
    }
    if before(1, '`') && free(c - 1) {
        return Some(Completion { insert: "`", ghost: "コード`" });
    }
    let prefix = detail::PREFIX.trim_end_matches('[');
    if chars[c - 1] == '[' && free(c - 1) && !chars[..c - 1].iter().collect::<String>().ends_with(prefix) {
        return Some(Completion { insert: "]()", ghost: "リンクテキスト](URL)" });
    }
    None
}

/// キャレットが要素の記号の内側にあれば、Tab で進む先 (リンクの label → url → 要素の後ろ、強調・コードは要素の後ろ)。
pub fn tab_stop(line: &str, caret: usize) -> Option<usize> {
    let chars: Vec<char> = line.chars().collect();
    let elements = parse(&chars, detail::find_iter(chars.iter().copied()));
    let e = elements.iter().find(|e| e.start < caret && caret < e.end)?;
    Some(match e.kind {
        ElementKind::Link { label_end, .. } if caret <= label_end => label_end + 2,
        _ => e.end,
    })
}

/// 行が見出しならそのレベル (1〜6)。行頭の数文字だけを見る (固定表示する見出しの検索用)。
pub fn heading_level(line: impl IntoIterator<Item = char>) -> Option<u8> {
    let head: Vec<char> = line.into_iter().take(8).collect();
    heading_prefix(&head).map(|h| h.0)
}

/// 行頭の見出しの記号 (`#` 1〜6 個 + 空白、または `#` だけの行)。(レベル, 記号の文字数)。
fn heading_prefix(chars: &[char]) -> Option<(u8, usize)> {
    let hashes = chars.iter().take_while(|c| **c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    match chars.get(hashes) {
        None => Some((hashes as u8, hashes)),
        Some(' ' | '\t') => {
            let spaces = chars[hashes..].iter().take_while(|c| **c == ' ' || **c == '\t').count();
            Some((hashes as u8, hashes + spaces))
        }
        _ => None,
    }
}

/// 装飾の要素を左から探す (詳細の範囲は除く)。入れ子にはしない。
fn parse(chars: &[char], token: Option<Token>) -> Vec<Element> {
    let mut out = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if let Some(t) = token.filter(|t| t.start == i) {
            i = t.end;
            continue;
        }
        // 詳細と重なる要素は作らない
        let limit = token.map_or(n, |t| if t.start > i { t.start } else { n });
        // 記号だけが n 文字並んでいるか (前後にも同じ記号が続く `*****` などは空の要素にしない)
        let lone_run = |c: char, n: usize| {
            i + n <= limit
                && chars[i..i + n].iter().all(|&x| x == c)
                && (i == 0 || chars[i - 1] != c)
                && chars.get(i + n) != Some(&c)
        };
        let found = match chars[i] {
            '`' if lone_run('`', 2) => Some(Element { start: i, end: i + 2, kind: ElementKind::Code }),
            '`' => (i + 2..limit).find(|&j| chars[j] == '`').map(|j| Element { start: i, end: j + 1, kind: ElementKind::Code }),
            '*' if lone_run('*', 4) => Some(Element { start: i, end: i + 4, kind: ElementKind::Strong }),
            '*' if i + 1 < limit && chars[i + 1] == '*' => (i + 3..limit.saturating_sub(1))
                .find(|&j| chars[j] == '*' && chars[j + 1] == '*')
                .map(|j| Element { start: i, end: j + 2, kind: ElementKind::Strong }),
            '[' => link_at(chars, i, limit),
            _ => None,
        };
        match found {
            Some(e) => {
                i = e.end;
                out.push(e);
            }
            None => i += 1,
        }
    }
    out
}

/// `[label](url)` (label は `]` を、url は空白と `)` を含まない。どちらも空でもよい)。
fn link_at(chars: &[char], i: usize, limit: usize) -> Option<Element> {
    let label_end = (i + 1..limit).find(|&j| chars[j] == ']' || chars[j] == '[')?;
    if chars[label_end] != ']' || chars.get(label_end + 1) != Some(&'(') {
        return None;
    }
    let url_start = label_end + 2;
    let close = (url_start..limit).find(|&j| chars[j] == ')' || chars[j].is_whitespace())?;
    if chars[close] != ')' {
        return None;
    }
    let url = chars[url_start..close].iter().collect();
    Some(Element { start: i, end: close + 1, kind: ElementKind::Link { label_end, url } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(v: &LineView) -> Vec<(&str, TokenKind)> {
        v.styles.iter().map(|s| (&v.text[s.range.clone()], s.kind)).collect()
    }

    #[test]
    fn concealed_markup() {
        let line = "a **b** `c` [ラベル](https://x.jp) d";
        let v = LineView::build(line, None);
        assert_eq!(v.text, "a b c ラベル d");
        assert_eq!(styles(&v), [("b", TokenKind::Strong), ("c", TokenKind::Code), ("ラベル", TokenKind::Link)]);
        assert_eq!(v.links, vec![(6..15, "https://x.jp".to_string())]);
        // 文書上の桁 ⇔ 表示上の桁
        assert_eq!(v.to_view(0, false), 0);
        assert_eq!(v.to_view(2, false), 2); // ** の前
        assert_eq!(v.to_view(4, false), 2); // b
        assert_eq!(v.to_view(5, false), 3); // ** の内側 → b の後ろ
        assert_eq!(v.to_view(8, false), 4); // ` の前の空白の後ろ
        assert_eq!(v.to_doc(2), 2);
        assert_eq!(v.to_doc(3), 7); // b の後ろ = 閉じ記号 ** の後ろ
        assert_eq!(v.to_doc(4), 8); // c の前 = 開き記号 ` の前
        assert_eq!(v.to_doc(v.text.chars().count()), line.chars().count());
    }

    #[test]
    fn caret_expands_element() {
        let line = "x **bold** y";
        assert_eq!(LineView::build(line, Some(0)).text, "x bold y");
        for c in [2, 5, 10] {
            let v = LineView::build(line, Some(c));
            assert_eq!(v.text, line, "caret {c}");
            assert_eq!(v.to_view(c, false), c);
            assert_eq!(styles(&v), [("**", TokenKind::Punctuation), ("bold", TokenKind::Strong), ("**", TokenKind::Punctuation)]);
        }
        assert_eq!(LineView::build(line, Some(11)).text, "x bold y");
    }

    #[test]
    fn detail_is_hidden() {
        let line = "買い物 *detail[牛乳\\n卵] **!**";
        let v = LineView::build(line, Some(0));
        assert_eq!(v.text, "買い物  !");
        let t = v.detail.unwrap();
        assert_eq!((t.start, t.end), (4, 18));
        assert_eq!(v.to_view(4, false), 4);
        assert_eq!(v.to_view(10, false), 4);
        assert_eq!(v.to_view(18, false), 4);
        assert_eq!(v.to_doc(4), 4); // 詳細の直前
        assert_eq!(v.to_doc(3), 3);
        // 詳細の中の記法は装飾しない
        let v = LineView::build("*detail[**x** `y`]", None);
        assert_eq!(v.text, "");
    }

    #[test]
    fn headings() {
        let v = LineView::build("## 見出し `x`", None);
        assert_eq!(v.text, "見出し x");
        assert_eq!(v.heading, Some(2));
        assert_eq!(styles(&v), [("見出し x", TokenKind::Strong), ("x", TokenKind::Code)]);
        assert_eq!(v.to_view(3, false), 0);
        assert_eq!(v.to_doc(0), 0); // 記号の前 (行頭)
        // キャレットがこの行にあれば記号を見せる
        let v = LineView::build("## 見出し", Some(5));
        assert_eq!(v.text, "## 見出し");
        assert_eq!(styles(&v), [("見出し", TokenKind::Strong), ("## ", TokenKind::Punctuation)]);
        assert_eq!(LineView::build("#", None).heading, Some(1));
        for line in ["#tag", "####### x", " # x"] {
            assert_eq!(LineView::build(line, None).heading, None, "{line}");
        }
    }

    #[test]
    fn placeholders() {
        // 折りたたみ: 空の位置にプレースホルダ
        let v = LineView::build("a **** b", None);
        assert_eq!(v.text, "a 強調 b");
        assert_eq!(styles(&v), [("強調", TokenKind::Placeholder)]);
        assert_eq!(v.to_view(2, false), 2);
        assert_eq!(v.to_view(6, false), 4); // 要素の後ろ
        assert_eq!(v.to_view(7, false), 5);
        assert_eq!(v.to_doc(2), 2); // 開き記号の前
        assert_eq!(v.to_doc(3), 4); // プレースホルダの中 → 空の位置
        assert_eq!(v.to_doc(4), 6); // 閉じ記号の後ろ
        assert_eq!(v.to_doc(5), 7);
        assert_eq!(LineView::build("``", None).text, "コード");
        assert_eq!(LineView::build("x``y", None).text, "xコードy");
        let v = LineView::build("[](u)", None);
        assert_eq!(v.text, "リンクテキスト");
        assert_eq!(v.links, vec![(0..v.text.len(), "u".to_string())]);
        assert!(LineView::build("[x]()", None).links.is_empty());
        // 展開: 記号の間にプレースホルダ、キャレットはその前
        let v = LineView::build("a **** b", Some(4));
        assert_eq!(v.text, "a **強調** b");
        assert_eq!(v.to_view(4, false), 4);
        assert_eq!(v.to_view(5, false), 7);
        assert_eq!(v.to_doc(5), 4);
        assert_eq!(v.to_doc(6), 4);
        assert_eq!(v.to_doc(7), 5);
        let v = LineView::build("[]()", Some(1));
        assert_eq!(v.text, "[リンクテキスト](URL)");
        assert_eq!(v.to_view(1, false), 1);
        assert_eq!(v.to_view(3, false), 10); // `(` の後ろ = URL の前
        assert_eq!(v.to_view(4, false), 14);
        assert_eq!(v.to_doc(13), 3);
        assert_eq!(v.to_doc(14), 4);
        assert_eq!(
            styles(&v),
            [
                ("[", TokenKind::Punctuation),
                ("リンクテキスト", TokenKind::Placeholder),
                ("](", TokenKind::Punctuation),
                ("URL", TokenKind::Placeholder),
                (")", TokenKind::Punctuation)
            ]
        );
    }

    #[test]
    fn completion() {
        let ghost = |line: &str, c: usize| completion_at(line, c).map(|c| c.ghost);
        assert_eq!(ghost("a **", 4), Some("強調**"));
        assert_eq!(ghost("**", 2), Some("強調**"));
        assert_eq!(ghost("a ** b", 4), Some("強調**"));
        assert_eq!(ghost("`", 1), Some("コード`"));
        assert_eq!(ghost("x [", 3), Some("リンクテキスト](URL)"));
        assert_eq!(completion_at("[", 1).unwrap().insert, "]()");
        for (line, c) in [("a ***", 5), ("**x", 2), ("**b **", 6), ("`x`", 3), ("``", 2), ("*detail[", 8), ("a", 1), ("", 0)] {
            assert_eq!(ghost(line, c), None, "{line}");
        }
        // 表示: キャレットの位置にゴーストを足す。キャレットはゴーストの前
        let v = LineView::build("a **", Some(4)).with_completion("a **", 4);
        assert_eq!(v.text, "a **強調**");
        assert_eq!(styles(&v), [("強調**", TokenKind::Placeholder)]);
        assert_eq!(v.to_view(4, false), 4);
        assert_eq!(v.to_doc(6), 4);
        assert_eq!(v.to_doc(8), 4);
        let v = LineView::build("**x** ** y", Some(8)).with_completion("**x** ** y", 8);
        assert_eq!(v.text, "x **強調** y");
        assert_eq!(v.to_view(9, false), 9);
        assert_eq!(v.to_doc(9), 9);
    }

    #[test]
    fn tab_stops() {
        assert_eq!(tab_stop("****", 2), Some(4));
        assert_eq!(tab_stop("a **b**", 5), Some(7));
        assert_eq!(tab_stop("[]()", 1), Some(3));
        assert_eq!(tab_stop("[ab](u)", 3), Some(5));
        assert_eq!(tab_stop("[ab](u)", 6), Some(7));
        assert_eq!(tab_stop("``", 1), Some(2));
        assert_eq!(tab_stop("**b**", 0), None);
        assert_eq!(tab_stop("**b**", 5), None);
        assert_eq!(tab_stop("abc", 1), None);
    }

    #[test]
    fn heading_levels() {
        assert_eq!(heading_level("### a".chars()), Some(3));
        assert_eq!(heading_level("#".chars()), Some(1));
        assert_eq!(heading_level("#a".chars()), None);
        assert_eq!(heading_level("x".chars()), None);
    }

    #[test]
    fn not_markup() {
        for line in ["a ** b", "`", "[x]", "[x](a b)", "a * b * c", "x***y"] {
            let v = LineView::build(line, None);
            assert_eq!(v.text, line, "{line}");
            assert!(v.styles.is_empty(), "{line}");
        }
    }
}
