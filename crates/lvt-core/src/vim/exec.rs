//! `VimEdit` の実行。テキストの変更は他のコマンドと同じく `Document::apply` を経由する。

use std::ops::Range;

use crate::buffer::TextBuffer;
use crate::document::Document;
use crate::editor::{indent_unit, outdent_len, EditorConfig, ViewInfo};
use crate::history::MergeKind;
use crate::selection::Selection;

use super::motion::{self, first_nonblank, Ctx, MotionKind};
use super::{InsertAt, OpTarget, Operator, Register, Visual, VimEdit, VimMotion};

/// 演算子を適用する範囲。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Span {
    Chars(Range<usize>),
    /// 行の範囲 (終端の行を含まない)。
    Lines(Range<usize>),
}

/// ヤンク・削除した文字列があれば返す (UI 層がクリップボードに置く)。
pub fn execute(doc: &mut Document, cmd: &VimEdit, cfg: &EditorConfig, view: &ViewInfo) -> Option<Register> {
    use VimEdit as V;
    match cmd {
        V::Move { motion, count } => move_cursor(doc, *motion, *count, cfg, view),
        V::Operate { op, target, count } => return operate(doc, *op, *target, *count, cfg, view),
        V::Insert(at) => enter_insert(doc, *at, cfg),
        V::ExitInsert => {
            doc.end_undo_group();
            let p = doc.selection().head;
            let start = doc.buffer().line_to_char(doc.buffer().char_to_line(p));
            doc.set_selection(Selection::caret(if p > start { p - 1 } else { p }));
            doc.preferred_col = None;
        }
        V::Visual { linewise } => match (linewise, doc.visual) {
            (Some(l), Some(mut v)) => {
                v.linewise = *l;
                set_visual(doc, v);
            }
            (Some(l), None) => {
                let p = doc.selection().head;
                set_visual(doc, Visual { anchor: p, cursor: p, linewise: *l });
            }
            (None, _) => exit_visual(doc),
        },
        V::VisualSwap => {
            if let Some(v) = doc.visual {
                set_visual(doc, Visual { anchor: v.cursor, cursor: v.anchor, ..v });
            }
        }
        V::VisualObject { obj, inner } => {
            let pos = cursor(doc);
            if let Some(r) = motion::object_range(doc.buffer(), pos, *obj, *inner, 1).filter(|r| !r.is_empty()) {
                set_visual(doc, Visual { anchor: r.start, cursor: r.end - 1, linewise: false });
            }
        }
        V::VisualFromSelection => {
            let sel = doc.selection();
            if sel.is_empty() {
                doc.visual = None;
            } else {
                let (anchor, cursor) = if sel.head > sel.anchor { (sel.anchor, sel.head - 1) } else { (sel.anchor - 1, sel.head) };
                set_visual(doc, Visual { anchor, cursor, linewise: false });
            }
        }
        V::Paste { text, linewise, before, count } => paste(doc, text, *linewise, *before, *count),
        V::ReplaceChar { c, count } => replace_char(doc, *c, *count),
        V::Join { count } => join(doc, *count),
        V::ToggleCaseChars { count } => {
            let p = doc.selection().head;
            let end = (p + count).min(doc.buffer().line_end(doc.buffer().char_to_line(p)));
            if end > p {
                let text = map_case(&doc.buffer().slice_to_string(p..end), Operator::ToggleCase);
                doc.apply(vec![(p..end, text)], MergeKind::None, |_, _| Selection::caret(end));
            }
        }
    }
    None
}

/// Vim のカーソル位置 (ビジュアルモードでは選択の端ではなくカーソル)。
fn cursor(doc: &Document) -> usize {
    doc.caret_pos()
}

fn ctx<'a>(doc: &'a Document, cfg: &'a EditorConfig, view: &'a ViewInfo) -> Ctx<'a> {
    Ctx { buf: doc.buffer(), cfg, view, pref: doc.preferred_col, detail_view: doc.detail_view() }
}

fn move_cursor(doc: &mut Document, m: VimMotion, count: Option<usize>, cfg: &EditorConfig, view: &ViewInfo) {
    let Some(j) = motion::target(&ctx(doc, cfg, view), cursor(doc), m, count) else { return };
    match doc.visual {
        Some(v) => set_visual(doc, Visual { cursor: j.pos, ..v }),
        None => doc.set_selection(Selection::caret(j.pos)),
    }
    doc.preferred_col = j.pref;
}

// ---- ビジュアルモード ----

fn set_visual(doc: &mut Document, v: Visual) {
    let buf = doc.buffer();
    let len = buf.len_chars();
    let sel = if v.linewise {
        let (l0, l1) = visual_lines(buf, &v);
        let (s, e) = (buf.line_to_char(l0), (buf.line_end(l1) + 1).min(len));
        if v.cursor >= v.anchor { Selection::new(s, e) } else { Selection::new(e, s) }
    } else if v.cursor >= v.anchor {
        Selection::new(v.anchor, (v.cursor + 1).min(len))
    } else {
        Selection::new((v.anchor + 1).min(len), v.cursor)
    };
    doc.set_selection(sel);
    doc.visual = Some(v);
}

fn visual_lines(buf: &TextBuffer, v: &Visual) -> (usize, usize) {
    let a = buf.char_to_line(v.anchor);
    let c = buf.char_to_line(v.cursor);
    (a.min(c), a.max(c))
}

fn exit_visual(doc: &mut Document) {
    if let Some(v) = doc.visual.take() {
        doc.set_selection(Selection::caret(v.cursor));
    }
}

/// ノーマルモードのキャレットを整える: 選択を解除し、改行の上に置かない。
/// UI 層が、ノーマルモードでの入力 (やマウス操作・検索) のあとに呼ぶ。
pub fn normalize_caret(doc: &mut Document) {
    if doc.visual.is_some() {
        return;
    }
    let sel = doc.selection();
    let p = if sel.is_empty() { sel.head } else { sel.start() };
    let line = doc.buffer().char_to_line(p);
    let p = p.min(motion::last_visible_char(doc.buffer(), line, doc.detail_view()));
    doc.set_selection(Selection::caret(p));
}

/// ビジュアルモードのカーソルを `pos` へ移す (検索で移動した場合など)。
pub fn set_visual_cursor(doc: &mut Document, pos: usize) {
    if let Some(v) = doc.visual {
        set_visual(doc, Visual { cursor: pos, ..v });
    }
}

/// ビジュアルモードの行範囲 (`:'<,'>` 用、0 始まり)。
pub fn visual_line_range(doc: &Document) -> Option<(usize, usize)> {
    doc.visual.map(|v| visual_lines(doc.buffer(), &v))
}

/// ビジュアルモードを抜ける (UI 層から)。
pub fn leave_visual(doc: &mut Document) {
    exit_visual(doc);
}

// ---- 演算子 ----

fn operate(
    doc: &mut Document,
    op: Operator,
    target: OpTarget,
    count: Option<usize>,
    cfg: &EditorConfig,
    view: &ViewInfo,
) -> Option<Register> {
    // x / X: 見た目が空の行 (詳細だけの行) では詳細を消す (BS / Del と同じ)
    if op == Operator::Delete
        && matches!(target, OpTarget::Motion(VimMotion::Right | VimMotion::Left))
        && doc.visual.is_none()
    {
        let line = doc.buffer().char_to_line(cursor(doc));
        if doc.delete_detail_on_blank_line(line) {
            return None;
        }
    }
    let buf = doc.buffer();
    let pos = cursor(doc);
    let n = count.unwrap_or(1).max(1);
    let span = match target {
        OpTarget::Visual | OpTarget::VisualLines => {
            let v = doc.visual?;
            if v.linewise || target == OpTarget::VisualLines {
                let (l0, l1) = visual_lines(buf, &v);
                Span::Lines(l0..l1 + 1)
            } else {
                let s = v.anchor.min(v.cursor);
                let e = (v.anchor.max(v.cursor) + 1).min(buf.len_chars());
                Span::Chars(s..e)
            }
        }
        OpTarget::Lines => {
            let l = buf.char_to_line(pos);
            Span::Lines(l..(l + n).min(buf.len_lines()))
        }
        OpTarget::Object { obj, inner } => Span::Chars(motion::object_range(buf, pos, obj, inner, n)?),
        OpTarget::Motion(m) => motion_span(doc, op, m, count, cfg, view)?,
    };
    // 見えない詳細に合わせて範囲を調整する (ヤンクする文字列も実際に消す範囲に合わせる)
    let span = match span {
        Span::Chars(r) => {
            let adjusted = doc.adjust_range(r.clone());
            // 1 行の中の範囲が、詳細を飛び越えたことで次の行へはみ出さないようにする (x で行を結合しない)
            let buf = doc.buffer();
            let line_end = buf.line_end(buf.char_to_line(r.start));
            if r.end <= line_end && adjusted.end > line_end {
                Span::Chars(adjusted.start.min(line_end)..line_end)
            } else {
                Span::Chars(adjusted)
            }
        }
        lines => lines,
    };
    doc.visual = None;
    let reg = apply_op(doc, op, span, pos, cfg);
    doc.preferred_col = None;
    reg
}

fn motion_span(
    doc: &Document,
    op: Operator,
    m: VimMotion,
    count: Option<usize>,
    cfg: &EditorConfig,
    view: &ViewInfo,
) -> Option<Span> {
    let buf = doc.buffer();
    let pos = cursor(doc);
    let n = count.unwrap_or(1).max(1);
    let on_blank = buf.char(pos).is_none_or(|c| c.is_whitespace());
    let m = match m {
        // cw は単語の後ろの空白を含めない (ce と同じ)
        VimMotion::WordStart { big } if op == Operator::Change && !on_blank => VimMotion::WordEnd { big },
        _ => m,
    };
    let end = match m {
        VimMotion::WordStart { big } => motion::word_end_for_operator(buf, pos, n, big),
        _ => motion::target(&ctx(doc, cfg, view), pos, m, count)?.pos,
    };
    let (s, e) = (pos.min(end), pos.max(end));
    Some(match m.kind() {
        MotionKind::Linewise => Span::Lines(buf.char_to_line(s)..buf.char_to_line(e) + 1),
        MotionKind::Inclusive => {
            let e = if buf.char(e).is_some_and(|c| c != '\n') { e + 1 } else { e };
            Span::Chars(s..e)
        }
        MotionKind::Exclusive => {
            let (ls, le) = (buf.char_to_line(s), buf.char_to_line(e));
            // 終端が後ろの行の行頭なら、前の行の行末までにする。始点がインデント以前なら行単位にする
            if e > s && le > ls && buf.line_to_char(le) == e && !matches!(m, VimMotion::WordStart { .. }) {
                if s <= first_nonblank(buf, ls) {
                    Span::Lines(ls..le)
                } else {
                    Span::Chars(s..e - 1)
                }
            } else {
                Span::Chars(s..e)
            }
        }
    })
}

/// 行範囲を、削除するときの文字範囲にする (最終行を含むなら前の改行を消す)。
fn lines_delete_range(buf: &TextBuffer, lines: &Range<usize>) -> Range<usize> {
    let n = buf.len_lines();
    if lines.end < n {
        buf.line_to_char(lines.start)..buf.line_to_char(lines.end)
    } else if lines.start > 0 {
        buf.line_end(lines.start - 1)..buf.len_chars()
    } else {
        0..buf.len_chars()
    }
}

fn lines_text(buf: &TextBuffer, lines: &Range<usize>) -> String {
    let s = buf.line_to_char(lines.start);
    let e = buf.line_end(lines.end - 1);
    format!("{}\n", buf.slice_to_string(s..e))
}

fn apply_op(doc: &mut Document, op: Operator, span: Span, pos: usize, cfg: &EditorConfig) -> Option<Register> {
    if op == Operator::Change {
        // 削除と、続く挿入を 1 回の Undo で戻せるようにする (挿入モードを抜けるときに閉じる)
        doc.begin_undo_group();
    }
    let buf = doc.buffer();
    let register = match &span {
        Span::Chars(r) => Register { text: buf.slice_to_string(r.clone()), linewise: false },
        Span::Lines(l) => Register { text: lines_text(buf, l), linewise: true },
    };
    match op {
        Operator::Yank => {
            let caret = match &span {
                Span::Chars(r) => r.start,
                Span::Lines(l) if buf.char_to_line(pos) == l.start => pos,
                Span::Lines(l) => buf.line_to_char(l.start),
            };
            doc.set_selection(Selection::caret(caret));
            Some(register)
        }
        Operator::Delete => {
            match span {
                Span::Chars(r) => {
                    let start = r.start;
                    doc.apply(vec![(r, String::new())], MergeKind::None, |_, _| Selection::caret(start));
                }
                Span::Lines(l) => {
                    let r = lines_delete_range(buf, &l);
                    let line = l.start;
                    doc.apply(vec![(r, String::new())], MergeKind::None, |b, _| {
                        Selection::caret(first_nonblank(b, line.min(b.len_lines() - 1)))
                    });
                }
            }
            Some(register)
        }
        Operator::Change => {
            match span {
                Span::Chars(r) => {
                    let start = r.start;
                    doc.apply(vec![(r, String::new())], MergeKind::None, |_, _| Selection::caret(start));
                }
                Span::Lines(l) => {
                    let s = buf.line_to_char(l.start);
                    let e = buf.line_end(l.end - 1);
                    let indent: String = if cfg.auto_indent {
                        buf.line(l.start).chars().take_while(|c| *c == ' ' || *c == '\t').collect()
                    } else {
                        String::new()
                    };
                    let caret = s + indent.chars().count();
                    doc.apply(vec![(s..e, indent)], MergeKind::None, |_, _| Selection::caret(caret));
                }
            }
            Some(register)
        }
        Operator::Indent | Operator::Outdent => {
            let lines = match span {
                Span::Lines(l) => l,
                Span::Chars(r) => buf.char_to_line(r.start)..buf.char_to_line(r.end) + 1,
            };
            let tw = cfg.tab_width.max(1);
            let edits: Vec<_> = lines
                .clone()
                .filter_map(|l| {
                    let p = buf.line_to_char(l);
                    if op == Operator::Indent {
                        (buf.line_len(l) > 0).then(|| (p..p, indent_unit(cfg, 0)))
                    } else {
                        let n = outdent_len(buf, l, tw);
                        (n > 0).then(|| (p..p + n, String::new()))
                    }
                })
                .collect();
            let first = lines.start;
            doc.apply(edits, MergeKind::None, |b, _| Selection::caret(first_nonblank(b, first)));
            None
        }
        Operator::ToggleCase | Operator::Lower | Operator::Upper => {
            let r = match span {
                Span::Chars(r) => r,
                Span::Lines(l) => buf.line_to_char(l.start)..buf.line_end(l.end - 1),
            };
            let text = map_case(&buf.slice_to_string(r.clone()), op);
            let start = r.start;
            doc.apply(vec![(r, text)], MergeKind::None, |_, _| Selection::caret(start));
            None
        }
    }
}

fn map_case(s: &str, op: Operator) -> String {
    match op {
        Operator::Lower => s.to_lowercase(),
        Operator::Upper => s.to_uppercase(),
        _ => s
            .chars()
            .flat_map(|c| -> Box<dyn Iterator<Item = char>> {
                if c.is_uppercase() {
                    Box::new(c.to_lowercase())
                } else {
                    Box::new(c.to_uppercase())
                }
            })
            .collect(),
    }
}

// ---- 挿入・貼り付け・その他 ----

fn enter_insert(doc: &mut Document, at: InsertAt, cfg: &EditorConfig) {
    doc.begin_undo_group();
    let buf = doc.buffer();
    let p = doc.selection().head;
    let line = buf.char_to_line(p);
    let indent: String = if cfg.auto_indent {
        buf.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').collect()
    } else {
        String::new()
    };
    let caret = match at {
        InsertAt::Before => p,
        InsertAt::After => (p + 1).min(buf.line_end(line)),
        InsertAt::LineStart => first_nonblank(buf, line),
        InsertAt::LineEnd => buf.line_end(line),
        InsertAt::LineBelow => {
            let e = buf.line_end(line);
            let caret = e + 1 + indent.chars().count();
            doc.apply(vec![(e..e, format!("\n{indent}"))], MergeKind::None, |_, _| Selection::caret(caret));
            return;
        }
        InsertAt::LineAbove => {
            let s = buf.line_to_char(line);
            let caret = s + indent.chars().count();
            doc.apply(vec![(s..s, format!("{indent}\n"))], MergeKind::None, |_, _| Selection::caret(caret));
            return;
        }
    };
    doc.set_selection(Selection::caret(caret));
    doc.preferred_col = None;
}

fn paste(doc: &mut Document, text: &str, linewise: bool, before: bool, count: usize) {
    if text.is_empty() {
        return;
    }
    let text = text.repeat(count.max(1));
    // ビジュアルモードでは選択範囲を置き換える (クリップボードは変えない)
    if let Some(v) = doc.visual.take() {
        let buf = doc.buffer();
        let r = if v.linewise {
            let (l0, l1) = visual_lines(buf, &v);
            buf.line_to_char(l0)..(buf.line_end(l1) + 1).min(buf.len_chars())
        } else {
            v.anchor.min(v.cursor)..(v.anchor.max(v.cursor) + 1).min(buf.len_chars())
        };
        let start = r.start;
        let text = if linewise && !v.linewise { format!("\n{text}") } else { text };
        doc.apply(vec![(r, text)], MergeKind::None, |_, _| Selection::caret(start));
        return;
    }
    let buf = doc.buffer();
    let p = doc.selection().head;
    let line = buf.char_to_line(p);
    if linewise {
        let body = text.strip_suffix('\n').unwrap_or(&text).to_string();
        let (at, insert, first_line) = if before {
            (buf.line_to_char(line), format!("{body}\n"), line)
        } else if line + 1 < buf.len_lines() {
            (buf.line_to_char(line + 1), format!("{body}\n"), line + 1)
        } else {
            (buf.len_chars(), format!("\n{body}"), line + 1)
        };
        doc.apply(vec![(at..at, insert)], MergeKind::None, |b, _| Selection::caret(first_nonblank(b, first_line)));
    } else {
        let at = if before || p >= buf.line_end(line) { p } else { p + 1 };
        let n = text.chars().count();
        let caret = if text.contains('\n') { at } else { at + n - 1 };
        doc.apply(vec![(at..at, text)], MergeKind::None, |_, _| Selection::caret(caret));
    }
    doc.preferred_col = None;
}

fn replace_char(doc: &mut Document, c: char, count: usize) {
    let buf = doc.buffer();
    let p = doc.selection().head;
    let n = count.max(1);
    if p + n > buf.line_end(buf.char_to_line(p)) {
        return;
    }
    let (text, caret) = if c == '\n' { ("\n".to_string(), p + 1) } else { (c.to_string().repeat(n), p + n - 1) };
    doc.apply(vec![(p..p + n, text)], MergeKind::None, |_, _| Selection::caret(caret));
}

fn join(doc: &mut Document, count: usize) {
    let visual = doc.visual.take();
    let buf = doc.buffer();
    let (first, joins) = match visual {
        Some(v) => {
            let (l0, l1) = visual_lines(buf, &v);
            (l0, (l1 - l0).max(1))
        }
        None => (buf.char_to_line(doc.selection().head), count.max(2) - 1),
    };
    let joins = joins.min(buf.len_lines() - 1 - first);
    if joins == 0 {
        return;
    }
    let mut edits = Vec::new();
    let mut shift: isize = 0;
    let mut caret = 0;
    for l in first..first + joins {
        let nl = buf.line_end(l);
        let next = buf.line(l + 1);
        let ws = next.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let rest_empty = ws == next.len_chars();
        let prev_blank = buf.line_len(l) == 0 || matches!(buf.char(nl - 1), Some(' ' | '\t'));
        let sep = if rest_empty || prev_blank || next.chars().nth(ws) == Some(')') { "" } else { " " };
        caret = (nl as isize + shift) as usize;
        shift += sep.len() as isize - (1 + ws) as isize;
        edits.push((nl..nl + 1 + ws, sep.to_string()));
    }
    doc.apply(edits, MergeKind::None, |_, _| Selection::caret(caret));
    doc.preferred_col = None;
}
