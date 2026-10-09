//! 選択範囲。`anchor` は選択開始点、`head` はキャレット位置 (いずれも文字インデックス)。
//!
//! 将来マルチカーソルを導入する場合は `Vec<Selection>` に拡張する。

use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(pos: usize) -> Self {
        Self { anchor: pos, head: pos }
    }

    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }

    /// `extend` が真なら anchor を保ったまま head を動かし、偽ならキャレットにする。
    pub fn moved_to(&self, pos: usize, extend: bool) -> Self {
        if extend {
            Self { anchor: self.anchor, head: pos }
        } else {
            Self::caret(pos)
        }
    }

    pub fn clamp(&self, len: usize) -> Self {
        Self { anchor: self.anchor.min(len), head: self.head.min(len) }
    }
}
