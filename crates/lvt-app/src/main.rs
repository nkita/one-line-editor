//! lvt: 軽量テキストエディタ (Windows 専用)。
//!
//! Windows 以外でビルドした場合は何もしない (lvt-core のテストを Linux で回すため)。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

fn main() {
    #[cfg(windows)]
    win::run();
    #[cfg(not(windows))]
    eprintln!("lvt は Windows 専用です。x86_64-pc-windows-gnu 向けにビルドしてください。");
}
