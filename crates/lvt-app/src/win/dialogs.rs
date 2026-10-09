//! モーダルダイアログとクリップボード。
//!
//! モーダルダイアログは内部でメッセージループを回すため、`App` を借用したまま
//! 呼び出してはならない (win/mod.rs の「遅延処理」から呼ぶ)。

use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_OVERWRITEPROMPT,
    OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDNO, IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_ICONWARNING, MB_OK, MB_YESNO, MB_YESNOCANCEL,
};

use super::util::wide;

const APP_NAME: &str = "lvt";
const FILTER: &str = "すべてのファイル (*.*)\0*.*\0テキスト (*.txt)\0*.txt\0Markdown (*.md)\0*.md;*.markdown\0\0";

pub fn open_files(hwnd: HWND) -> Vec<PathBuf> {
    let filter: Vec<u16> = FILTER.encode_utf16().collect();
    let mut buf = vec![0u16; 64 * 1024];
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_ALLOWMULTISELECT,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return Vec::new();
    }
    // 単一選択: "C:\dir\file\0"、複数選択: "C:\dir\0a.txt\0b.txt\0\0"
    let parts: Vec<String> = buf
        .split(|&c| c == 0)
        .take_while(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect();
    match parts.as_slice() {
        [] => Vec::new(),
        [single] => vec![PathBuf::from(single)],
        [dir, files @ ..] => files.iter().map(|f| Path::new(dir).join(f)).collect(),
    }
}

pub fn save_file(hwnd: HWND, suggested: Option<&Path>) -> Option<PathBuf> {
    let filter: Vec<u16> = FILTER.encode_utf16().collect();
    let mut buf = vec![0u16; 32 * 1024];
    if let Some(name) = suggested {
        let w: Vec<u16> = name.to_string_lossy().encode_utf16().collect();
        let n = w.len().min(buf.len() - 1);
        buf[..n].copy_from_slice(&w[..n]);
    }
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        Flags: OFN_EXPLORER | OFN_PATHMUSTEXIST | OFN_OVERWRITEPROMPT,
        ..Default::default()
    };
    if !unsafe { GetSaveFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Cancel,
}

pub fn ask_yes_no_cancel(hwnd: HWND, text: &str) -> Answer {
    let (t, c) = (wide(text), wide(APP_NAME));
    match unsafe { MessageBoxW(Some(hwnd), PCWSTR(t.as_ptr()), PCWSTR(c.as_ptr()), MB_YESNOCANCEL | MB_ICONWARNING) } {
        IDYES => Answer::Yes,
        IDNO => Answer::No,
        _ => Answer::Cancel,
    }
}

pub fn ask_yes_no(hwnd: HWND, text: &str) -> bool {
    let (t, c) = (wide(text), wide(APP_NAME));
    let r = unsafe { MessageBoxW(Some(hwnd), PCWSTR(t.as_ptr()), PCWSTR(c.as_ptr()), MB_YESNO | MB_ICONQUESTION) };
    r == IDYES
}

pub fn error(hwnd: HWND, text: &str) {
    let (t, c) = (wide(text), wide(APP_NAME));
    unsafe { MessageBoxW(Some(hwnd), PCWSTR(t.as_ptr()), PCWSTR(c.as_ptr()), MB_OK | MB_ICONERROR) };
}

/// クリップボードへテキストを書き込む (内部の LF は CRLF に変換)。
pub fn set_clipboard(hwnd: HWND, text: &str) -> bool {
    let w: Vec<u16> = text.replace('\n', "\r\n").encode_utf16().chain(Some(0)).collect();
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return false;
        }
        let ok = (|| -> windows::core::Result<()> {
            EmptyClipboard()?;
            let h = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2)?;
            let p = GlobalLock(h) as *mut u16;
            if p.is_null() {
                return Err(windows::core::Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
            let _ = GlobalUnlock(h);
            SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0)))?;
            Ok(())
        })()
        .is_ok();
        let _ = CloseClipboard();
        ok
    }
}

/// クリップボードのテキスト (改行は LF に正規化)。
pub fn get_clipboard(hwnd: HWND) -> Option<String> {
    unsafe {
        OpenClipboard(Some(hwnd)).ok()?;
        let text = (|| {
            let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut len = 0;
            while *p.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            let _ = GlobalUnlock(g);
            Some(s)
        })();
        let _ = CloseClipboard();
        text.map(|s| s.replace("\r\n", "\n").replace('\r', "\n"))
    }
}
