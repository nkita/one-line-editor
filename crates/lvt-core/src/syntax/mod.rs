//! シンタックスハイライトの拡張点。
//!
//! 言語ごとに `Highlighter` を実装し、`highlighter_for_path` に登録する。
//! ハイライタは 1 行ずつ呼ばれ、行をまたぐ構文 (複数行コメント、コードブロック等) は
//! `LineState` で次の行へ状態を引き継ぐ。状態は `HighlightCache` が一定行ごとに
//! キャッシュするので、巨大ファイルの末尾を表示しても毎回先頭から解析し直さない。
//!
//! 将来 Markdown のプレビュー的な表示 (見出しの文字サイズ変更など) を行う場合は、
//! `StyleSpan` とは別に行単位の装飾情報を返す API をここに追加する想定 (docs/DESIGN.md 参照)。

mod markdown;

use std::ops::Range;
use std::path::Path;

use crate::buffer::TextBuffer;

pub use markdown::MarkdownHighlighter;

/// トークンの意味。色や太字などの見た目は `theme::Theme` が決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    String,
    Comment,
    Number,
    Heading,
    Emphasis,
    Strong,
    Code,
    Link,
    Quote,
    ListMarker,
    Punctuation,
    /// 空の要素に表示するプレースホルダ (装飾表示)。
    Placeholder,
}

impl TokenKind {
    pub const COUNT: usize = 13;
}

/// 行内のバイト範囲とその種類。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleSpan {
    pub range: Range<usize>,
    pub kind: TokenKind,
}

/// 行の開始時点の解析状態 (ハイライタごとに意味を決める不透明な値)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineState(pub u32);

pub trait Highlighter: Send {
    fn name(&self) -> &'static str;

    /// 1 行 (改行を含まない) を解析して `out` に追加し、次の行の開始状態を返す。
    fn highlight_line(&self, line: &str, state: LineState, out: &mut Vec<StyleSpan>) -> LineState;

    /// 行をまたぐ状態を持たないなら false (キャッシュ処理を省略できる)。
    fn is_stateful(&self) -> bool {
        true
    }
}

/// 何もしないハイライタ。
pub struct PlainText;

impl Highlighter for PlainText {
    fn name(&self) -> &'static str {
        "Text"
    }

    fn highlight_line(&self, _line: &str, state: LineState, _out: &mut Vec<StyleSpan>) -> LineState {
        state
    }

    fn is_stateful(&self) -> bool {
        false
    }
}

/// ファイル名から言語を選ぶ。新しい言語はここに登録する。
pub fn highlighter_for_path(path: Option<&Path>) -> Box<dyn Highlighter> {
    let ext = path
        .and_then(|p| p.extension())
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" | "mkd" => Box::new(MarkdownHighlighter),
        _ => Box::new(PlainText),
    }
}

/// 行開始状態のキャッシュ。`INTERVAL` 行ごとに状態を記録する。
#[derive(Default)]
pub struct HighlightCache {
    checkpoints: Vec<LineState>,
}

impl HighlightCache {
    const INTERVAL: usize = 256;

    /// `line` 行目以降の状態を無効化する (編集時に呼ぶ)。
    pub fn invalidate_from(&mut self, line: usize) {
        let keep = line / Self::INTERVAL + 1;
        self.checkpoints.truncate(keep);
    }

    /// `line` 行目の開始状態を求める。
    pub fn state_at(&mut self, hl: &dyn Highlighter, buf: &TextBuffer, line: usize) -> LineState {
        if !hl.is_stateful() {
            return LineState::default();
        }
        if self.checkpoints.is_empty() {
            self.checkpoints.push(LineState::default());
        }
        let target_cp = line / Self::INTERVAL;
        let mut scratch = Vec::new();
        let mut text = String::new();
        let mut run = |from: usize, to: usize, mut st: LineState| {
            for l in from..to.min(buf.len_lines()) {
                text.clear();
                for chunk in buf.line(l).chunks() {
                    text.push_str(chunk);
                }
                scratch.clear();
                st = hl.highlight_line(&text, st, &mut scratch);
            }
            st
        };
        while self.checkpoints.len() <= target_cp {
            let cp = self.checkpoints.len() - 1;
            let st = run(cp * Self::INTERVAL, (cp + 1) * Self::INTERVAL, self.checkpoints[cp]);
            self.checkpoints.push(st);
        }
        run(target_cp * Self::INTERVAL, line, self.checkpoints[target_cp])
    }
}
