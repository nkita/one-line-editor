//! lvt-core: OS に依存しないエディタの中核。
//!
//! Windows 固有のコードは一切持たず、WSL/Linux 上の `cargo test` で検証できる。
//! UI 層 (lvt-app) は以下の流れでこのクレートを使う:
//!
//! ```text
//! キー入力 → keymap::InputMode → command::Command → editor::execute → document::Document
//!                                                  ↘ (AppCommand はUI層が処理)
//! 描画     ← syntax::Highlighter / theme::Theme ← document::Document
//! ```

pub mod buffer;
pub mod command;
pub mod detail;
pub mod document;
pub mod editor;
pub mod encoding;
pub mod history;
pub mod keymap;
pub mod lineview;
pub mod link;
pub mod search;
pub mod selection;
pub mod settings;
pub mod syntax;
pub mod theme;
pub mod vim;
pub mod width;

pub use buffer::TextBuffer;
pub use command::{AppCommand, Command, EditCommand, Motion};
pub use document::Document;
pub use selection::Selection;
