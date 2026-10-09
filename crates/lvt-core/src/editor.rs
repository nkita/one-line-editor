//! 編集コマンドの実行。OS に依存しないので単体テストで動作を確認できる。

use std::ops::Range;

use crate::buffer::TextBuffer;
use crate::command::{EditCommand, Motion};
use crate::detail;
use crate::document::Document;
use crate::history::MergeKind;
use crate::selection::Selection;
use crate::vim::{self, Register};
use crate::width::{self, char_class, CharClass};

#[derive(Clone, Debug)]
pub struct EditorConfig {
    pub tab_width: usize,
    /// Tab キーで空白を挿入するか。
    pub insert_spaces: bool,
    /// 改行時に前の行のインデントを引き継ぐか。
    pub auto_indent: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self { tab_width: 4, insert_spaces: false, auto_indent: true }
    }
}

/// コマンド実行に必要な表示側の情報。
#[derive(Clone, Copy, Debug)]
pub struct ViewInfo {
    /// 1 画面に表示できる行数 (PageUp/PageDown の移動量)。
    pub page_lines: usize,
    /// 画面の先頭に表示している行 (Vim の H / M / L で使う)。
    pub top_line: usize,
}

/// コマンドを実行する。ヤンク・削除した文字列 (Vim) があれば返す。
pub fn execute(doc: &mut Document, cmd: &EditCommand, cfg: &EditorConfig, view: &ViewInfo) -> Option<Register> {
    use EditCommand as E;
    match cmd {
        E::Vim(v) => return vim::execute(doc, v, cfg, view),
        E::Move { motion, extend } => move_caret(doc, *motion, *extend, cfg, view),
        E::SelectAll => {
            let len = doc.buffer().len_chars();
            doc.set_selection(Selection::new(0, len));
        }
        E::InsertText(text) => {
            let merge = if text.chars().count() == 1 && text != "\n" { MergeKind::Typing } else { MergeKind::None };
            doc.replace_selection(text, merge);
        }
        E::InsertNewline => {
            let indent = if cfg.auto_indent { leading_indent(doc) } else { String::new() };
            doc.replace_selection(&format!("\n{indent}"), MergeKind::None);
        }
        E::InsertTab => insert_tab(doc, cfg),
        E::Outdent => outdent(doc, cfg),
        E::DeleteBackward => delete_with(doc, MergeKind::Deleting, |_, p| p.saturating_sub(1)..p),
        E::DeleteForward => delete_with(doc, MergeKind::Deleting, |b, p| p..(p + 1).min(b.len_chars())),
        E::DeleteWordBackward => delete_with(doc, MergeKind::None, |b, p| word_left(b, p)..p),
        E::DeleteWordForward => delete_with(doc, MergeKind::None, |b, p| p..word_right(b, p)),
        E::DeleteSelection => {
            if !doc.selection().is_empty() {
                doc.replace_selection("", MergeKind::None);
            }
        }
        E::DuplicateLine => duplicate(doc),
        E::Undo => {
            doc.undo();
        }
        E::Redo => {
            doc.redo();
        }
    }
    None
}

// ---- 移動 ----

fn move_caret(doc: &mut Document, motion: Motion, extend: bool, cfg: &EditorConfig, view: &ViewInfo) {
    let sel = doc.selection();
    let buf = doc.buffer();
    let pos = sel.head;
    let (line, _) = buf.line_col(pos);
    let vertical = matches!(motion, Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown);
    let dv = doc.detail_view();
    let pref = doc.preferred_col.unwrap_or_else(|| current_display_col(buf, pos, cfg.tab_width, dv));

    let target = match motion {
        // 選択中に Shift なしの左右キーを押したら選択の端へ縮める
        Motion::Left if !extend && !sel.is_empty() => sel.start(),
        Motion::Right if !extend && !sel.is_empty() => sel.end(),
        Motion::Left => pos.saturating_sub(1),
        Motion::Right => (pos + 1).min(buf.len_chars()),
        Motion::WordLeft => word_left(buf, pos),
        Motion::WordRight => word_right(buf, pos),
        Motion::LineStart => {
            let start = buf.line_to_char(line);
            let indent_end = start + buf.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').count();
            if pos == indent_end { start } else { indent_end }
        }
        Motion::LineEnd => buf.line_end(line),
        Motion::DocStart => 0,
        Motion::DocEnd => buf.len_chars(),
        Motion::Up if line == 0 => 0,
        Motion::Down if line + 1 >= buf.len_lines() => buf.len_chars(),
        Motion::Up => pos_at_display_col(buf, line - 1, pref, cfg.tab_width, dv),
        Motion::Down => pos_at_display_col(buf, line + 1, pref, cfg.tab_width, dv),
        Motion::PageUp => pos_at_display_col(buf, line.saturating_sub(view.page_lines.max(1)), pref, cfg.tab_width, dv),
        Motion::PageDown => {
            let l = (line + view.page_lines.max(1)).min(buf.len_lines() - 1);
            pos_at_display_col(buf, l, pref, cfg.tab_width, dv)
        }
    };
    doc.set_selection(sel.moved_to(target, extend));
    doc.preferred_col = vertical.then_some(pref);
}

pub(crate) fn current_display_col(buf: &TextBuffer, pos: usize, tab: usize, detail_view: bool) -> usize {
    let (line, col) = buf.line_col(pos);
    match detail_token(buf, line, detail_view) {
        Some(t) => {
            let view = detail::view_text(&buf.line(line).to_string(), Some(t));
            width::display_col(view.chars(), t.to_view(col, false), tab)
        }
        None => width::display_col(buf.line(line).chars(), col, tab),
    }
}

/// 行の表示桁 `col` に最も近い文字位置。詳細を折りたたんでいる行では、折りたたんだ表示で数える。
pub(crate) fn pos_at_display_col(buf: &TextBuffer, line: usize, col: usize, tab: usize, detail_view: bool) -> usize {
    let start = buf.line_to_char(line);
    match detail_token(buf, line, detail_view) {
        Some(t) => {
            let view = detail::view_text(&buf.line(line).to_string(), Some(t));
            start + t.to_doc(width::char_col_at_display(view.chars(), col, tab))
        }
        None => start + width::char_col_at_display(buf.line(line).chars(), col, tab),
    }
}

fn detail_token(buf: &TextBuffer, line: usize, detail_view: bool) -> Option<detail::Token> {
    if detail_view {
        detail::find_in_line(buf, line)
    } else {
        None
    }
}

/// 単語の先頭方向への移動先。空白を飛ばし、同じ文字種が続く範囲を 1 単語とみなす。
pub fn word_left(buf: &TextBuffer, pos: usize) -> usize {
    let mut p = pos;
    while p > 0 && char_class(buf.rope().char(p - 1)) == CharClass::Space {
        p -= 1;
    }
    if p == 0 {
        return 0;
    }
    let class = char_class(buf.rope().char(p - 1));
    if class == CharClass::Newline {
        return p - 1;
    }
    while p > 0 && char_class(buf.rope().char(p - 1)) == class {
        p -= 1;
    }
    p
}

/// 単語の末尾方向への移動先 (次の単語の先頭)。
pub fn word_right(buf: &TextBuffer, pos: usize) -> usize {
    let len = buf.len_chars();
    let mut p = pos;
    if p >= len {
        return len;
    }
    let class = char_class(buf.rope().char(p));
    if class == CharClass::Newline {
        return p + 1;
    }
    while p < len && char_class(buf.rope().char(p)) == class {
        p += 1;
    }
    while p < len && char_class(buf.rope().char(p)) == CharClass::Space {
        p += 1;
    }
    p
}

/// `pos` を含む単語の範囲 (ダブルクリックでの選択用)。
pub fn word_at(buf: &TextBuffer, pos: usize) -> Range<usize> {
    let len = buf.len_chars();
    let pos = pos.min(len);
    let Some(class) = buf.char(pos).map(char_class).filter(|c| *c != CharClass::Newline) else {
        return pos..pos;
    };
    let mut start = pos;
    while start > 0 && char_class(buf.rope().char(start - 1)) == class {
        start -= 1;
    }
    let mut end = pos;
    while end < len && char_class(buf.rope().char(end)) == class {
        end += 1;
    }
    start..end
}

// ---- 入力・削除 ----

pub(crate) fn leading_indent(doc: &Document) -> String {
    let buf = doc.buffer();
    let (line, col) = buf.line_col(doc.selection().start());
    buf.line(line).chars().take(col).take_while(|c| *c == ' ' || *c == '\t').collect()
}

pub(crate) fn indent_unit(cfg: &EditorConfig, display_col: usize) -> String {
    if cfg.insert_spaces {
        let tw = cfg.tab_width.max(1);
        " ".repeat(tw - display_col % tw)
    } else {
        "\t".to_string()
    }
}

/// 選択範囲が含む行 (末尾が行頭ちょうどならその行は含めない)。
fn selected_lines(buf: &TextBuffer, sel: Selection) -> Range<usize> {
    let first = buf.char_to_line(sel.start());
    let mut last = buf.char_to_line(sel.end());
    if last > first && buf.line_to_char(last) == sel.end() {
        last -= 1;
    }
    first..last + 1
}

fn insert_tab(doc: &mut Document, cfg: &EditorConfig) {
    let sel = doc.selection();
    let buf = doc.buffer();
    let lines = selected_lines(buf, sel);
    if sel.is_empty() || lines.len() == 1 {
        let unit = indent_unit(cfg, current_display_col(buf, sel.start(), cfg.tab_width, doc.detail_view()));
        doc.replace_selection(&unit, MergeKind::None);
        return;
    }
    let unit = indent_unit(cfg, 0);
    let edits = lines
        .clone()
        .filter(|&l| buf.line_len(l) > 0)
        .map(|l| {
            let p = buf.line_to_char(l);
            (p..p, unit.clone())
        })
        .collect();
    doc.apply(edits, MergeKind::None, |b, _| select_whole_lines(b, &lines));
}

fn outdent(doc: &mut Document, cfg: &EditorConfig) {
    let sel = doc.selection();
    let buf = doc.buffer();
    let lines = selected_lines(buf, sel);
    let tw = cfg.tab_width.max(1);
    let edits: Vec<_> = lines
        .clone()
        .filter_map(|l| {
            let n = outdent_len(buf, l, tw);
            let p = buf.line_to_char(l);
            (n > 0).then(|| (p..p + n, String::new()))
        })
        .collect();
    if sel.is_empty() || lines.len() == 1 {
        let removed = edits.first().map_or(0, |(r, _)| r.len());
        let line_start = buf.line_to_char(lines.start);
        let head = sel.head.saturating_sub(removed).max(line_start);
        let anchor = sel.anchor.saturating_sub(removed).max(line_start);
        doc.apply(edits, MergeKind::None, |_, _| Selection::new(anchor, head));
    } else {
        doc.apply(edits, MergeKind::None, |b, _| select_whole_lines(b, &lines));
    }
}

/// 行頭から取り除くインデントの文字数 (タブ 1 個、または最大 tab_width 個の空白)。
pub(crate) fn outdent_len(buf: &TextBuffer, line: usize, tab_width: usize) -> usize {
    let mut n = 0;
    for c in buf.line(line).chars() {
        match c {
            '\t' if n == 0 => return 1,
            ' ' if n < tab_width => n += 1,
            _ => break,
        }
    }
    n
}

fn select_whole_lines(buf: &TextBuffer, lines: &Range<usize>) -> Selection {
    let end_line = lines.end - 1;
    let end = if end_line + 1 < buf.len_lines() { buf.line_to_char(end_line + 1) } else { buf.line_end(end_line) };
    Selection::new(buf.line_to_char(lines.start), end)
}

/// 選択があれば選択を、なければ `range_of(caret)` を削除する。
fn delete_with(doc: &mut Document, merge: MergeKind, range_of: impl Fn(&TextBuffer, usize) -> Range<usize>) {
    let sel = doc.selection();
    let range = if sel.is_empty() { range_of(doc.buffer(), sel.head) } else { sel.range() };
    if range.is_empty() {
        return;
    }
    let start = range.start;
    let merge = if sel.is_empty() { merge } else { MergeKind::None };
    doc.apply(vec![(range, String::new())], merge, |_, _| Selection::caret(start));
    doc.preferred_col = None;
}

fn duplicate(doc: &mut Document) {
    let sel = doc.selection();
    let buf = doc.buffer();
    if sel.is_empty() {
        let line = buf.char_to_line(sel.head);
        let start = buf.line_to_char(line);
        let text = format!("{}\n", buf.line(line));
        let n = text.chars().count();
        doc.apply(vec![(start..start, text)], MergeKind::None, |_, _| Selection::caret(sel.head + n));
    } else {
        let text = doc.selected_text();
        let n = text.chars().count();
        let end = sel.end();
        doc.apply(vec![(end..end, text)], MergeKind::None, |_, _| Selection::new(end, end + n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: ViewInfo = ViewInfo { page_lines: 10, top_line: 0 };

    fn run(doc: &mut Document, cmds: &[EditCommand]) {
        let cfg = EditorConfig::default();
        for c in cmds {
            execute(doc, c, &cfg, &VIEW);
        }
    }

    fn mv(motion: Motion) -> EditCommand {
        EditCommand::Move { motion, extend: false }
    }

    #[test]
    fn typing_and_undo() {
        let mut d = Document::new();
        run(&mut d, &[EditCommand::InsertText("a".into()), EditCommand::InsertText("b".into()), EditCommand::InsertNewline]);
        assert_eq!(d.buffer().to_string(), "ab\n");
        run(&mut d, &[EditCommand::Undo]);
        assert_eq!(d.buffer().to_string(), "ab");
        run(&mut d, &[EditCommand::Undo]);
        assert_eq!(d.buffer().to_string(), "");
    }

    #[test]
    fn vertical_move_keeps_column() {
        let mut d = Document::from_text("abcdef\nab\nabcdef");
        d.set_selection(Selection::caret(5));
        run(&mut d, &[mv(Motion::Down)]);
        assert_eq!(d.buffer().line_col(d.selection().head), (1, 2));
        run(&mut d, &[mv(Motion::Down)]);
        assert_eq!(d.buffer().line_col(d.selection().head), (2, 5));
    }

    #[test]
    fn vertical_move_with_wide_chars() {
        let mut d = Document::from_text("あいうえ\nabcdefgh");
        d.set_selection(Selection::caret(2)); // "あい|" = 4 桁
        run(&mut d, &[mv(Motion::Down)]);
        assert_eq!(d.buffer().line_col(d.selection().head), (1, 4));
    }

    #[test]
    fn auto_indent_and_smart_home() {
        let mut d = Document::from_text("    foo");
        d.set_selection(Selection::caret(7));
        run(&mut d, &[EditCommand::InsertNewline]);
        assert_eq!(d.buffer().to_string(), "    foo\n    ");
        run(&mut d, &[mv(Motion::Up), mv(Motion::LineEnd), mv(Motion::LineStart)]);
        assert_eq!(d.selection().head, 4);
        run(&mut d, &[mv(Motion::LineStart)]);
        assert_eq!(d.selection().head, 0);
    }

    #[test]
    fn word_motion_japanese() {
        let b = TextBuffer::from_str("今日はRustで書く");
        assert_eq!(word_right(&b, 0), 2); // 今日 | は
        assert_eq!(word_right(&b, 2), 3); // は | Rust
        assert_eq!(word_right(&b, 3), 7);
        assert_eq!(word_left(&b, 7), 3);
        assert_eq!(word_at(&b, 4), 3..7);
    }

    #[test]
    fn block_indent_outdent() {
        let mut d = Document::from_text("a\nb\nc");
        d.set_selection(Selection::new(0, 3)); // a, b を選択
        run(&mut d, &[EditCommand::InsertTab]);
        assert_eq!(d.buffer().to_string(), "\ta\n\tb\nc");
        run(&mut d, &[EditCommand::Outdent]);
        assert_eq!(d.buffer().to_string(), "a\nb\nc");
    }

    #[test]
    fn delete_and_duplicate() {
        let mut d = Document::from_text("hello world");
        d.set_selection(Selection::caret(11));
        run(&mut d, &[EditCommand::DeleteWordBackward]);
        assert_eq!(d.buffer().to_string(), "hello ");
        run(&mut d, &[EditCommand::DeleteBackward, EditCommand::DuplicateLine]);
        assert_eq!(d.buffer().to_string(), "hello\nhello");
        run(&mut d, &[EditCommand::SelectAll, EditCommand::DeleteForward]);
        assert_eq!(d.buffer().to_string(), "");
    }
}

#[cfg(test)]
mod detail_tests {
    use super::*;

    const VIEW: ViewInfo = ViewInfo { page_lines: 10, top_line: 0 };

    fn doc(text: &str, caret: usize) -> Document {
        let mut d = Document::from_text(text);
        d.set_detail_view(true);
        d.set_selection(Selection::caret(caret));
        d
    }

    fn run(d: &mut Document, cmds: &[EditCommand]) {
        for c in cmds {
            execute(d, c, &EditorConfig::default(), &VIEW);
        }
    }

    fn mv(motion: Motion) -> EditCommand {
        EditCommand::Move { motion, extend: false }
    }

    #[test]
    fn caret_skips_detail() {
        let mut d = doc("*[abc] x", 0);
        run(&mut d, &[mv(Motion::Right)]);
        assert_eq!(d.selection().head, 6);
        run(&mut d, &[mv(Motion::Right), mv(Motion::Left), mv(Motion::Left)]);
        assert_eq!(d.selection().head, 0);
        d.set_selection(Selection::caret(3)); // 内側へは置けない (前向きの移動なので後ろの端へ)
        assert_eq!(d.selection().head, 6);
        d.set_selection(Selection::caret(0));
        run(&mut d, &[EditCommand::Move { motion: Motion::Right, extend: true }]);
        assert_eq!(d.selected_text(), "*[abc]");
    }

    #[test]
    fn deleting_part_removes_whole_detail() {
        let mut d = doc("  *[abc] x", 8);
        run(&mut d, &[EditCommand::DeleteBackward]);
        assert_eq!((d.buffer().to_string().as_str(), d.selection().head), ("   x", 2));
        run(&mut d, &[EditCommand::Undo]);
        assert_eq!(d.buffer().to_string(), "  *[abc] x");
        d.set_selection(Selection::caret(2));
        run(&mut d, &[EditCommand::DeleteForward]);
        assert_eq!(d.buffer().to_string(), "   x");
    }

    #[test]
    fn content_edits_and_insertions() {
        let mut d = doc("*[abc] x", 0);
        d.apply(vec![(2..5, "zz".into())], MergeKind::None, |_, _| Selection::caret(0));
        assert_eq!(d.buffer().to_string(), "*[zz] x");
        // 記号の間への挿入は詳細の後ろへ
        d.apply(vec![(1..1, "Q".into())], MergeKind::None, |_, _| Selection::caret(0));
        assert_eq!(d.buffer().to_string(), "*[zz]Q x");
        // 折りたたみが無効なら通常の文字列
        d.set_detail_view(false);
        d.set_selection(Selection::caret(3));
        assert_eq!(d.selection().head, 3);
    }

    #[test]
    fn vertical_move_uses_folded_columns() {
        // 表示は "📝xy" (アイコンは 2 桁)
        let mut d = doc("*[abc]xy\nabcdef", 7);
        run(&mut d, &[mv(Motion::Down)]);
        assert_eq!(d.selection().head, 12);
        run(&mut d, &[mv(Motion::Up)]);
        assert_eq!(d.selection().head, 7);
    }
}
