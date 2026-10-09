//! 1 行の表示モデル (.txt / .lvt の装飾表示)。
//!
//! 文書上の行から、表示用の文字列・書式・文書上の桁 ⇔ 表示上の桁の対応を作る。
//! - 詳細 `*detail[…]` は隠す (幅 0)。詳細があることは行番号の欄のバッジで示す (UI 層)。
//! - `[label](url)` は label だけ、`` `code` `` と `**強調**` は記号を隠して表示する。
//!   キャレットが要素の上 (両端を含む) にあるときは、その要素だけ元の記法のまま表示する。
//!
//! 状態は持たず、描画・クリック判定・上下移動のたびに作る (見えている行だけなので軽い)。

use std::ops::Range;

use crate::detail::{self, Token};
use crate::syntax::{StyleSpan, TokenKind};

/// 文書上の範囲 `doc` を、表示では `view_len` 文字 (0 = 隠す) にする置き換え。
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
                    let kind = match e.kind {
                        ElementKind::Code => TokenKind::Code,
                        ElementKind::Strong => TokenKind::Strong,
                        ElementKind::Link { .. } => TokenKind::Link,
                    };
                    let inner = e.start + open..e.end - close;
                    if expanded {
                        let v0 = vlen;
                        push_raw(&mut view, &mut vlen, e.start..e.end);
                        char_styles.push((v0..v0 + open, TokenKind::Punctuation));
                        char_styles.push((v0 + open..v0 + open + inner.len(), kind));
                        char_styles.push((v0 + open + inner.len()..vlen, TokenKind::Punctuation));
                    } else {
                        folds.push(Fold { doc: (e.start, inner.start), view_len: 0, closing: false });
                        let v0 = vlen;
                        push_raw(&mut view, &mut vlen, inner.clone());
                        folds.push(Fold { doc: (inner.end, e.end), view_len: 0, closing: true });
                        char_styles.push((v0..vlen, kind));
                        if let ElementKind::Link { url, .. } = &e.kind {
                            char_links.push((v0..vlen, url.clone()));
                        }
                    }
                    pos = e.end;
                }
            }
        }
        push_raw(&mut view, &mut vlen, pos..chars.len());

        let to_byte = |s: &str, c: usize| s.char_indices().nth(c).map_or(s.len(), |(b, _)| b);
        let byte_range = |r: &Range<usize>| to_byte(&view, r.start)..to_byte(&view, r.end);
        let styles =
            char_styles.iter().filter(|(r, _)| !r.is_empty()).map(|(r, k)| StyleSpan { range: byte_range(r), kind: *k }).collect();
        let links = char_links.iter().map(|(r, u)| (byte_range(r), u.clone())).collect();
        Self { styles, links, detail: token, folds, text: view }
    }

    /// 文書上の桁 → 表示上の桁。隠した範囲の内側は、`round_up` なら置き換えの後ろ、そうでなければ前。
    pub fn to_view(&self, col: usize, round_up: bool) -> usize {
        let mut shift = 0usize; // 文書上の桁 − 表示上の桁
        for f in &self.folds {
            let (s, e) = f.doc;
            if col <= s {
                break;
            }
            if col < e {
                return s - shift + if round_up { f.view_len } else { 0 };
            }
            shift += (e - s) - f.view_len;
        }
        col - shift
    }

    /// 表示上の桁 → 文書上の桁。置き換え (アイコン) の後ろは、置き換えた範囲の直後。
    /// 隠した記号の位置では、開き記号なら記号の前、閉じ記号なら記号の後ろ (要素の外側) にする。
    pub fn to_doc(&self, vcol: usize) -> usize {
        let mut shift = 0usize;
        for f in &self.folds {
            let (s, e) = f.doc;
            let vs = s - shift;
            if vcol < vs || (vcol == vs && !(f.closing && f.view_len == 0)) {
                break;
            }
            if vcol < vs + f.view_len {
                return e;
            }
            shift += (e - s) - f.view_len;
        }
        vcol + shift
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
        let found = match chars[i] {
            '`' => (i + 2..limit).find(|&j| chars[j] == '`').map(|j| Element { start: i, end: j + 1, kind: ElementKind::Code }),
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

/// `[label](url)` (label は空でなく `]` を含まない、url は空白と `)` を含まない)。
fn link_at(chars: &[char], i: usize, limit: usize) -> Option<Element> {
    let label_end = (i + 1..limit).find(|&j| chars[j] == ']' || chars[j] == '[')?;
    if chars[label_end] != ']' || label_end == i + 1 || chars.get(label_end + 1) != Some(&'(') {
        return None;
    }
    let url_start = label_end + 2;
    let close = (url_start..limit).find(|&j| chars[j] == ')' || chars[j].is_whitespace())?;
    if chars[close] != ')' || close == url_start {
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
    fn not_markup() {
        for line in ["a ** b", "`", "``", "[x]", "[x](a b)", "[](u)", "a * b * c", "****"] {
            let v = LineView::build(line, None);
            assert_eq!(v.text, line, "{line}");
            assert!(v.styles.is_empty(), "{line}");
        }
    }
}
