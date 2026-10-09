//! 文書 = テキストバッファ + 選択範囲 + 履歴 + ファイル情報 + ハイライト状態。
//!
//! テキストの変更は必ず `Document` のメソッド (`apply` 系) を経由させ、
//! 履歴とハイライトキャッシュの整合性を保つ。

use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::buffer::TextBuffer;
use crate::detail::{self, Token};
use crate::encoding::{self, DecodedText, LineEnding, SaveError, TextEncoding};
use crate::history::{Edit, History, MergeKind};
use crate::selection::Selection;
use crate::vim::Visual;
use crate::syntax::{self, HighlightCache, Highlighter, LineState, StyleSpan};

pub struct Document {
    buffer: TextBuffer,
    selection: Selection,
    /// 上下移動時に維持する表示桁。
    pub preferred_col: Option<usize>,
    /// Vim のビジュアルモードの状態 (選択範囲は `vim` モジュールがこれから作る)。
    pub visual: Option<Visual>,
    /// 装飾表示 (詳細 `*detail[…]` の折りたたみなど) を使うか (.txt / .lvt で UI 層が有効にする)。
    detail_view: bool,
    history: History,
    path: Option<PathBuf>,
    pub encoding: TextEncoding,
    pub line_ending: LineEnding,
    /// 文字コード・改行コードの変更など、テキスト以外の未保存の変更。
    meta_modified: bool,
    highlighter: Box<dyn Highlighter>,
    highlight_cache: HighlightCache,
    /// テキストが変更されるたびに増える (UI 側のキャッシュ無効化用)。
    revision: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub fn new() -> Self {
        Self {
            buffer: TextBuffer::new(),
            selection: Selection::default(),
            preferred_col: None,
            visual: None,
            detail_view: false,
            history: History::new(),
            path: None,
            encoding: TextEncoding::Utf8,
            line_ending: LineEnding::platform_default(),
            meta_modified: false,
            highlighter: syntax::highlighter_for_path(None),
            highlight_cache: HighlightCache::default(),
            revision: 0,
        }
    }

    pub fn from_text(text: &str) -> Self {
        let mut d = Self::new();
        d.buffer = TextBuffer::from_rope(encoding::build_rope_normalized(text));
        d
    }

    pub fn from_decoded(decoded: DecodedText, path: Option<PathBuf>) -> Self {
        let mut d = Self::new();
        d.buffer = TextBuffer::from_rope(decoded.rope);
        d.encoding = decoded.encoding;
        d.line_ending = decoded.line_ending;
        d.highlighter = syntax::highlighter_for_path(path.as_deref());
        d.path = path;
        d
    }

    pub fn open(path: &Path, forced: Option<TextEncoding>) -> std::io::Result<(Self, bool)> {
        let decoded = encoding::load_file(path, forced)?;
        let had_errors = decoded.had_errors;
        Ok((Self::from_decoded(decoded, Some(path.to_path_buf())), had_errors))
    }

    pub fn save(&mut self) -> Result<(), SaveError> {
        let path = self.path.clone().ok_or_else(|| SaveError::Io(std::io::Error::other("no file path")))?;
        self.save_as(&path)
    }

    pub fn save_as(&mut self, path: &Path) -> Result<(), SaveError> {
        encoding::save_file(path, self.buffer.rope(), self.encoding, self.line_ending)?;
        if self.path.as_deref() != Some(path) {
            self.highlighter = syntax::highlighter_for_path(Some(path));
            self.highlight_cache = HighlightCache::default();
            self.path = Some(path.to_path_buf());
        }
        self.history.mark_saved();
        self.meta_modified = false;
        Ok(())
    }

    // ---- 参照系 ----

    pub fn buffer(&self) -> &TextBuffer {
        &self.buffer
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// キャレットを描く位置。通常は選択の head、Vim のビジュアルモードではカーソル位置。
    pub fn caret_pos(&self) -> usize {
        self.visual.map_or(self.selection.head, |v| v.cursor.min(self.buffer.len_chars()))
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn title(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "無題".to_string())
    }

    pub fn is_modified(&self) -> bool {
        self.meta_modified || self.history.is_modified()
    }

    /// 新規・未編集の空文書か (ファイルを開いたときに置き換えてよいか)。
    pub fn is_pristine(&self) -> bool {
        self.path.is_none() && !self.is_modified() && self.buffer.len_chars() == 0
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn selected_text(&self) -> String {
        self.buffer.slice_to_string(self.selection.range())
    }

    pub fn highlighter_name(&self) -> &'static str {
        self.highlighter.name()
    }

    /// 指定行のハイライト結果。`text` はその行の内容 (呼び出し側で取得済みのもの)。
    pub fn highlight_line(&mut self, line: usize, text: &str, out: &mut Vec<StyleSpan>) {
        let state: LineState = self.highlight_cache.state_at(self.highlighter.as_ref(), &self.buffer, line);
        self.highlighter.highlight_line(text, state, out);
    }

    // ---- 詳細 (`*detail[…]`) の折りたたみ ----

    pub fn detail_view(&self) -> bool {
        self.detail_view
    }

    /// 詳細の折りたたみを有効・無効にする。有効にしたら、キャレットを詳細の外へ寄せる。
    pub fn set_detail_view(&mut self, on: bool) {
        self.detail_view = on;
        let sel = self.selection;
        self.selection = self.snap_selection(sel, sel);
    }

    /// 行の詳細 (折りたたみが無効なら None)。
    pub fn detail_at_line(&self, line: usize) -> Option<Token> {
        if self.detail_view {
            detail::find_in_line(&self.buffer, line)
        } else {
            None
        }
    }

    /// 行の詳細と行頭の位置。
    fn token_of(&self, pos: usize) -> Option<(usize, Token)> {
        let line = self.buffer.char_to_line(pos);
        self.detail_at_line(line).map(|t| (self.buffer.line_to_char(line), t))
    }

    /// 詳細は本文では幅 0 (見えない) なので、キャレットは詳細の直前 (`start`) にだけ置く。
    /// 詳細の内側や直後に来たら直前へ寄せる。ただし直前から前向きに 1 歩動いた場合は、
    /// 見た目どおり次の文字を 1 つ越えた位置 (詳細の直後の文字の後ろ) へ進める。
    fn snap(&self, pos: usize, from: Option<usize>) -> usize {
        let Some((ls, t)) = self.token_of(pos) else { return pos };
        let (ts, te) = (ls + t.start, ls + t.end);
        if pos <= ts || pos > te {
            return pos;
        }
        if from == Some(ts) && pos == ts + 1 && te < self.buffer.len_chars() {
            te + 1
        } else {
            ts
        }
    }

    /// 新しい選択の端が詳細の内側・直後に来ないようにする。
    fn snap_selection(&self, old: Selection, new: Selection) -> Selection {
        if !self.detail_view {
            return new;
        }
        let head = self.snap(new.head, Some(old.head));
        let anchor = if new.anchor == new.head { head } else { self.snap(new.anchor, None) };
        Selection::new(anchor, head)
    }

    /// 削除範囲を、見えない詳細に合わせて調整する。
    /// - 詳細の内容の中に収まる範囲 (置換など) と、詳細にかからない範囲はそのまま。
    /// - 詳細の直前から始まり詳細の中で終わる範囲 (Del や Vim の `x`) は、詳細の後ろの文字を消す。
    /// - 詳細の手前から詳細の中・直後までの範囲は、詳細の手前までにする (詳細は残す)。
    /// - 詳細の中から始まる範囲は、詳細の直後からにする。
    /// - 詳細全体をまたいでその先まで消す範囲 (`dd`、複数行の選択など) は、詳細ごと消す。
    pub fn adjust_range(&self, r: Range<usize>) -> Range<usize> {
        if !self.detail_view || r.is_empty() {
            return r;
        }
        let len = self.buffer.len_chars();
        let (mut s, mut e) = (r.start, r.end);
        if let Some((ls, t)) = self.token_of(s) {
            let (ts, te) = (ls + t.start, ls + t.end);
            let c = t.content();
            if ls + c.start <= s && e <= ls + c.end {
                return r;
            }
            if s == ts && e <= te {
                let n = e - s;
                return te..(te + n).min(len);
            }
            if ts < s && s < te {
                s = te;
            }
            if s < ts && ts < e && e <= te {
                e = ts;
            }
        }
        if let Some((ls, t)) = self.token_of(e) {
            let (ts, te) = (ls + t.start, ls + t.end);
            if s < ts && ts < e && e <= te {
                e = ts;
            }
        }
        s..e.max(s)
    }

    /// 行の詳細を削除する (1 回の Undo で戻せる)。詳細の前後の余分な空白も消し、
    /// 行が空白だけになったら行ごと消す。詳細がなければ false。
    pub fn delete_detail(&mut self, line: usize) -> bool {
        self.delete_detail_with(line, false)
    }

    /// 見た目が空の行 (詳細と空白だけの行) なら、その行の詳細を削除して空行を残す。
    /// キャレットがそこにある状態で BS / Del を押したときに使う (行の結合より先に詳細を消す)。
    pub fn delete_detail_on_blank_line(&mut self, line: usize) -> bool {
        let Some(t) = self.detail_at_line(line) else { return false };
        let s = self.buffer.line(line);
        let blank = s.chars().take(t.start).chain(s.chars().skip(t.end)).all(|c| c == ' ' || c == '\t');
        blank && self.delete_detail_with(line, true)
    }

    /// `keep_line` なら、行が空白だけになっても行は残す (空行にする)。
    fn delete_detail_with(&mut self, line: usize, keep_line: bool) -> bool {
        let Some(t) = self.detail_at_line(line) else { return false };
        let buf = &self.buffer;
        let ls = buf.line_to_char(line);
        let chars: Vec<char> = buf.line(line).chars().collect();
        let is_ws = |c: &char| *c == ' ' || *c == '\t';
        let (mut s, mut e) = (t.start, t.end);
        if e == chars.len() {
            // 行末の詳細: 前の空白も消す
            while s > 0 && is_ws(&chars[s - 1]) {
                s -= 1;
            }
        } else if s > 0 && is_ws(&chars[s - 1]) && is_ws(&chars[e]) {
            // 行の途中: 前後の空白が重ならないよう、後ろの空白を 1 つ消す
            e += 1;
        }
        let rest_blank = chars[..s].iter().chain(&chars[e..]).all(is_ws);
        let n = buf.len_lines();
        let (range, caret) = if !rest_blank {
            (ls + s..ls + e, ls + s)
        } else if keep_line {
            (ls..ls + chars.len(), ls)
        } else if line + 1 < n {
            (ls..buf.line_to_char(line + 1), ls)
        } else if line > 0 {
            (buf.line_end(line - 1)..buf.len_chars(), buf.line_to_char(line - 1))
        } else {
            (0..buf.len_chars(), 0)
        };
        // 詳細の削除そのものなので、見えない詳細に合わせた範囲の調整はしない
        let dv = std::mem::replace(&mut self.detail_view, false);
        self.apply(vec![(range, String::new())], MergeKind::None, |_, _| Selection::caret(caret));
        self.detail_view = dv;
        let sel = self.selection;
        self.selection = self.snap_selection(sel, sel);
        self.preferred_col = None;
        true
    }

    /// 挿入位置を調整する。詳細の内側への挿入は詳細の後ろへ。行末にある詳細の直前に
    /// 改行を挿入する場合 (行末で Enter) は、詳細を元の行に残すため詳細の後ろへ。
    fn adjust_insert(&self, p: usize, text: &str) -> usize {
        let Some((ls, t)) = self.token_of(p) else { return p };
        let (ts, te) = (ls + t.start, ls + t.end);
        let at_line_end = te == self.buffer.line_end(self.buffer.char_to_line(p));
        if (ts < p && p < te) || (p == ts && at_line_end && text.contains('\n')) {
            te
        } else {
            p
        }
    }

    // ---- 変更系 ----

    pub fn set_selection(&mut self, sel: Selection) {
        let sel = sel.clamp(self.buffer.len_chars());
        let sel = self.snap_selection(self.selection, sel);
        if sel != self.selection {
            self.history.break_merge();
        }
        self.selection = sel;
    }

    pub fn set_encoding(&mut self, enc: TextEncoding) {
        if self.encoding != enc {
            self.encoding = enc;
            self.meta_modified = true;
        }
    }

    pub fn set_line_ending(&mut self, eol: LineEnding) {
        if self.line_ending != eol {
            self.line_ending = eol;
            self.meta_modified = true;
        }
    }

    /// 複数の置換をまとめて 1 回の操作として適用する。
    /// `edits` は互いに重ならない範囲で、順序は問わない。適用後の選択は `after` で決める。
    pub fn apply(
        &mut self,
        mut edits: Vec<(Range<usize>, String)>,
        merge: MergeKind,
        after: impl FnOnce(&TextBuffer, &[Edit]) -> Selection,
    ) {
        if edits.is_empty() {
            return;
        }
        // 見えない詳細に合わせて編集範囲を調整し、キャレットは調整した編集の位置
        // (挿入なら挿入した文字列の後ろ、削除なら先頭) に置く
        // (編集前の位置, その後ろに足す文字数)
        let mut adjusted_at: Option<(usize, usize)> = None;
        if self.detail_view {
            for e in &mut edits {
                let r = if e.0.is_empty() {
                    let p = self.adjust_insert(e.0.start, &e.1);
                    p..p
                } else {
                    self.adjust_range(e.0.clone())
                };
                if r != e.0 {
                    let caret = (r.start, if r.is_empty() { e.1.chars().count() } else { 0 });
                    adjusted_at = Some(adjusted_at.map_or(caret, |p| p.min(caret)));
                    e.0 = r;
                }
            }
            edits.retain(|e| !(e.0.is_empty() && e.1.is_empty()));
            if edits.is_empty() {
                return;
            }
            if adjusted_at.is_some() {
                edits.sort_by_key(|e| e.0.start);
                edits.dedup_by(|later, earlier| later.0.start < earlier.0.end);
            }
        }
        let caret_after_expand = adjusted_at.map(|(p, extra)| {
            let delta: isize = edits
                .iter()
                .filter(|e| e.0.end <= p && e.0.start < p)
                .map(|e| e.1.chars().count() as isize - e.0.len() as isize)
                .sum();
            (p as isize + delta) as usize + extra
        });
        // 後ろから適用すれば前方の位置がずれない
        edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
        let before = self.selection;
        let mut records = Vec::with_capacity(edits.len());
        for (range, text) in edits {
            records.push(self.raw_replace(range.start, range.end - range.start, &text));
        }
        let sel = match caret_after_expand {
            Some(p) => Selection::caret(p),
            None => after(&self.buffer, &records),
        };
        let sel = sel.clamp(self.buffer.len_chars());
        let sel = self.snap_selection(sel, sel);
        self.selection = sel;
        self.history.record(records, before, sel, merge);
    }

    /// 選択範囲を `text` で置き換え、キャレットを挿入文字列の後ろに置く。
    pub fn replace_selection(&mut self, text: &str, merge: MergeKind) {
        let range = self.selection.range();
        let end = range.start + text.chars().count();
        self.apply(vec![(range, text.to_string())], merge, |_, _| Selection::caret(end));
        self.preferred_col = None;
    }

    /// これ以降 `end_undo_group` までの変更を 1 回の Undo で戻せるようにする。
    pub fn begin_undo_group(&mut self) {
        self.history.begin_group();
    }

    pub fn end_undo_group(&mut self) {
        self.history.end_group();
    }

    pub fn undo(&mut self) -> bool {
        let Some(tx) = self.history.pop_undo() else { return false };
        for e in tx.edits.iter().rev() {
            let n = e.inserted.chars().count();
            self.raw_replace(e.pos, n, &e.deleted);
        }
        let sel = tx.before.clamp(self.buffer.len_chars());
        self.selection = self.snap_selection(sel, sel);
        self.preferred_col = None;
        self.history.push_redo(tx);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(tx) = self.history.pop_redo() else { return false };
        for e in &tx.edits {
            let n = e.deleted.chars().count();
            self.raw_replace(e.pos, n, &e.inserted);
        }
        let sel = tx.after.clamp(self.buffer.len_chars());
        self.selection = self.snap_selection(sel, sel);
        self.preferred_col = None;
        self.history.push_undo(tx);
        true
    }

    /// 履歴を記録せずに置換する。唯一の低レベル変更経路。
    fn raw_replace(&mut self, pos: usize, len: usize, text: &str) -> Edit {
        let line = self.buffer.char_to_line(pos);
        let deleted = if len > 0 {
            let d = self.buffer.slice_to_string(pos..pos + len);
            self.buffer.remove(pos..pos + len);
            d
        } else {
            String::new()
        };
        if !text.is_empty() {
            self.buffer.insert(pos, text);
        }
        self.highlight_cache.invalidate_from(line);
        self.revision += 1;
        Edit { pos, deleted, inserted: text.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_and_modified_flag() {
        let mut d = Document::from_text("hello");
        assert!(!d.is_modified());
        d.set_selection(Selection::caret(5));
        d.replace_selection(" world", MergeKind::None);
        assert_eq!(d.buffer().to_string(), "hello world");
        assert!(d.is_modified());
        d.undo();
        assert_eq!(d.buffer().to_string(), "hello");
        assert!(!d.is_modified());
        d.redo();
        assert_eq!(d.buffer().to_string(), "hello world");
        assert_eq!(d.selection(), Selection::caret(11));
    }

    #[test]
    fn typing_is_merged() {
        let mut d = Document::new();
        for c in ["a", "b", "c"] {
            d.replace_selection(c, MergeKind::Typing);
        }
        d.undo();
        assert_eq!(d.buffer().to_string(), "");
    }

    #[test]
    fn undo_group() {
        let mut d = Document::from_text("abc");
        d.set_selection(Selection::caret(3));
        d.begin_undo_group();
        d.replace_selection("\n", MergeKind::None);
        d.replace_selection("x", MergeKind::Typing);
        d.set_selection(Selection::caret(0));
        d.replace_selection("y", MergeKind::None);
        d.end_undo_group();
        d.replace_selection("z", MergeKind::None);
        assert_eq!(d.buffer().to_string(), "yzabc\nx");
        d.undo();
        assert_eq!(d.buffer().to_string(), "yabc\nx");
        d.undo();
        assert_eq!(d.buffer().to_string(), "abc");
        d.redo();
        assert_eq!(d.buffer().to_string(), "yabc\nx");
    }

    #[test]
    fn multi_edit_undo() {
        let mut d = Document::from_text("a-b-c");
        d.apply(vec![(1..2, "++".into()), (3..4, "++".into())], MergeKind::None, |_, _| Selection::caret(0));
        assert_eq!(d.buffer().to_string(), "a++b++c");
        d.undo();
        assert_eq!(d.buffer().to_string(), "a-b-c");
        d.redo();
        assert_eq!(d.buffer().to_string(), "a++b++c");
    }
}
