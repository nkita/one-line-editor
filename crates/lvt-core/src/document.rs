//! 文書 = テキストバッファ + 選択範囲 + 履歴 + ファイル情報 + ハイライト状態。
//!
//! テキストの変更は必ず `Document` のメソッド (`apply` 系) を経由させ、
//! 履歴とハイライトキャッシュの整合性を保つ。

use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::buffer::TextBuffer;
use crate::encoding::{self, DecodedText, LineEnding, SaveError, TextEncoding};
use crate::history::{Edit, History, MergeKind};
use crate::selection::Selection;
use crate::syntax::{self, HighlightCache, Highlighter, LineState, StyleSpan};

pub struct Document {
    buffer: TextBuffer,
    selection: Selection,
    /// 上下移動時に維持する表示桁。
    pub preferred_col: Option<usize>,
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

    // ---- 変更系 ----

    pub fn set_selection(&mut self, sel: Selection) {
        let sel = sel.clamp(self.buffer.len_chars());
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
        // 後ろから適用すれば前方の位置がずれない
        edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
        let before = self.selection;
        let mut records = Vec::with_capacity(edits.len());
        for (range, text) in edits {
            records.push(self.raw_replace(range.start, range.end - range.start, &text));
        }
        let sel = after(&self.buffer, &records).clamp(self.buffer.len_chars());
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

    pub fn undo(&mut self) -> bool {
        let Some(tx) = self.history.pop_undo() else { return false };
        for e in tx.edits.iter().rev() {
            let n = e.inserted.chars().count();
            self.raw_replace(e.pos, n, &e.deleted);
        }
        self.selection = tx.before.clamp(self.buffer.len_chars());
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
        self.selection = tx.after.clamp(self.buffer.len_chars());
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
