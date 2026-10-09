//! テキストバッファ。
//!
//! 現在の実装は ropey (B木ベースの rope) の薄いラッパー。数百MB までのファイルを
//! 全量メモリに載せて扱う前提。将来 GB 級に対応する場合は、この型の内部を
//! mmap + ピースツリーに差し替える (公開 API はこのファイルに閉じている)。
//!
//! 位置はすべて「文字 (Unicode scalar value) インデックス」で表す。
//! 改行は内部的に常に LF (`\n`) に正規化されている。

use std::ops::Range;

use ropey::{Rope, RopeSlice};

#[derive(Clone, Default)]
pub struct TextBuffer {
    rope: Rope,
}

impl TextBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_rope(rope: Rope) -> Self {
        Self { rope }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Self {
        Self { rope: Rope::from_str(text) }
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// 行数。空バッファでも 1 行、末尾が改行なら最後に空行が 1 つ加わる。
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn char_to_line(&self, pos: usize) -> usize {
        self.rope.char_to_line(pos.min(self.len_chars()))
    }

    pub fn line_to_char(&self, line: usize) -> usize {
        self.rope.line_to_char(line.min(self.len_lines()))
    }

    /// 行の内容 (改行を含まない)。
    pub fn line(&self, line: usize) -> RopeSlice<'_> {
        let s = self.rope.line(line);
        let n = s.len_chars();
        if n > 0 && s.char(n - 1) == '\n' {
            s.slice(..n - 1)
        } else {
            s
        }
    }

    /// 行の文字数 (改行を含まない)。
    pub fn line_len(&self, line: usize) -> usize {
        self.line(line).len_chars()
    }

    /// 行末 (改行の直前) の文字位置。
    pub fn line_end(&self, line: usize) -> usize {
        self.line_to_char(line) + self.line_len(line)
    }

    /// 位置 → (行, 行内の文字位置)。
    pub fn line_col(&self, pos: usize) -> (usize, usize) {
        let line = self.char_to_line(pos);
        (line, pos.min(self.len_chars()) - self.line_to_char(line))
    }

    pub fn char(&self, pos: usize) -> Option<char> {
        (pos < self.len_chars()).then(|| self.rope.char(pos))
    }

    pub fn slice_to_string(&self, range: Range<usize>) -> String {
        self.rope.slice(range).to_string()
    }

    pub fn insert(&mut self, pos: usize, text: &str) {
        self.rope.insert(pos, text);
    }

    pub fn remove(&mut self, range: Range<usize>) {
        self.rope.remove(range);
    }

    pub fn char_to_byte(&self, pos: usize) -> usize {
        self.rope.char_to_byte(pos)
    }

    pub fn byte_to_char(&self, byte: usize) -> usize {
        self.rope.byte_to_char(byte)
    }
}

impl std::fmt::Display for TextBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for chunk in self.rope.chunks() {
            f.write_str(chunk)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_exclude_newline() {
        let b = TextBuffer::from_str("ab\ncde\n");
        assert_eq!(b.len_lines(), 3);
        assert_eq!(b.line(0).to_string(), "ab");
        assert_eq!(b.line(1).to_string(), "cde");
        assert_eq!(b.line(2).to_string(), "");
        assert_eq!(b.line_end(1), 6);
        assert_eq!(b.line_col(4), (1, 1));
    }
}
