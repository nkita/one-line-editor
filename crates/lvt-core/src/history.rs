//! Undo/Redo 履歴。
//!
//! 1 回の操作 (コマンド) を 1 つの `Transaction` とし、その中に複数の `Edit`
//! (置換操作) を記録する。連続した文字入力・削除は 1 つのトランザクションに
//! まとめられる。

use crate::selection::Selection;

/// `pos` から `deleted` を削除し、そこに `inserted` を挿入した、という記録。
#[derive(Clone, Debug)]
pub struct Edit {
    pub pos: usize,
    pub deleted: String,
    pub inserted: String,
}

#[derive(Clone, Debug)]
pub struct Transaction {
    id: u64,
    pub edits: Vec<Edit>,
    pub before: Selection,
    pub after: Selection,
}

/// 連続入力をまとめるための分類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeKind {
    None,
    Typing,
    Deleting,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    next_id: u64,
    last_merge: Option<MergeKind>,
    /// 保存時点の undo 先頭の id (0 = 履歴なし)。
    saved_id: u64,
    /// 保存時点の状態に戻れなくなったか (redo が破棄されたなど)。
    saved_unreachable: bool,
}

impl History {
    pub fn new() -> Self {
        Self { next_id: 1, ..Default::default() }
    }

    fn top_id(&self) -> u64 {
        self.undo.last().map_or(0, |t| t.id)
    }

    pub fn is_modified(&self) -> bool {
        self.saved_unreachable || self.top_id() != self.saved_id
    }

    pub fn mark_saved(&mut self) {
        self.saved_id = self.top_id();
        self.saved_unreachable = false;
        self.last_merge = None;
    }

    /// 次の入力を直前のトランザクションにまとめないようにする (カーソル移動時など)。
    pub fn break_merge(&mut self) {
        self.last_merge = None;
    }

    /// トランザクションを記録する。`merge` が直前と同じ種類ならまとめる。
    pub fn record(&mut self, edits: Vec<Edit>, before: Selection, after: Selection, merge: MergeKind) {
        if edits.is_empty() {
            return;
        }
        if !self.redo.is_empty() {
            // 保存時点が redo 側にあった場合、もう戻れない
            if self.redo.iter().any(|t| t.id == self.saved_id) {
                self.saved_unreachable = true;
            }
            self.redo.clear();
        }
        let can_merge = merge != MergeKind::None && self.last_merge == Some(merge);
        if can_merge {
            if let Some(top) = self.undo.last_mut() {
                top.edits.extend(edits);
                top.after = after;
                return;
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.undo.push(Transaction { id, edits, before, after });
        self.last_merge = Some(merge);
    }

    pub fn pop_undo(&mut self) -> Option<Transaction> {
        self.last_merge = None;
        let t = self.undo.pop()?;
        Some(t)
    }

    pub fn push_redo(&mut self, t: Transaction) {
        self.redo.push(t);
    }

    pub fn pop_redo(&mut self) -> Option<Transaction> {
        self.last_merge = None;
        self.redo.pop()
    }

    /// redo したトランザクションを undo 側へ戻す (id を保ったまま)。
    pub fn push_undo(&mut self, t: Transaction) {
        self.undo.push(t);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
