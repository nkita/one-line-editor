//! 詳細 (`*[…]`) を編集するモーダルウィンドウ。
//!
//! 複数行の入力には Windows 標準の EDIT コントロールを使う (IME・Undo・クリップボード・
//! スクロールを OS に任せる)。Ctrl+Enter で確定、Esc でキャンセル。
//! `App` を借用していない状態 (`Deferred` の実行中) で呼ぶこと。

use std::cell::Cell;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, HBRUSH, CLEARTYPE_QUALITY, COLOR_BTNFACE, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, FW_NORMAL, HGDIOBJ,
    OUT_DEFAULT_PRECIS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetKeyState, SetFocus, VK_CONTROL, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::util::{loword, wide};

const ID_EDIT: i32 = 100;
const ID_HINT: i32 = 101;
const CLASS: PCWSTR = w!("lvt.detail_editor");

thread_local! {
    /// None: 編集中、Some(true): 確定、Some(false): キャンセル。
    static RESULT: Cell<Option<bool>> = const { Cell::new(None) };
    static SCALE: Cell<f32> = const { Cell::new(1.0) };
}

/// 編集ウィンドウを開き、確定したら編集後の文字列 (改行は `\n`) を返す。
pub fn edit(owner: HWND, title: &str, initial: &str) -> Option<String> {
    RESULT.set(None);
    let scale = unsafe { GetDpiForWindow(owner) }.max(96) as f32 / 96.0;
    SCALE.set(scale);
    let px = |v: f32| (v * scale) as i32;
    let hinst = unsafe { GetModuleHandleW(None) }.ok()?;
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut _),
            lpszClassName: CLASS,
            ..Default::default()
        };
        // 2 回目以降は「登録済み」で失敗するが問題ない
        RegisterClassExW(&wc);
    }

    // 親ウィンドウの中央に置く
    let mut rc = RECT::default();
    unsafe {
        let _ = GetWindowRect(owner, &mut rc);
    }
    let (w, h) = (px(640.0), px(420.0));
    let x = rc.left + ((rc.right - rc.left) - w) / 2;
    let y = rc.top + ((rc.bottom - rc.top) - h) / 2;
    let title = wide(title);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            CLASS,
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME,
            x,
            y,
            w,
            h,
            Some(owner),
            None,
            Some(hinst.into()),
            None,
        )
    }
    .ok()?;

    let font = unsafe {
        CreateFontW(
            -px(15.0),
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Yu Gothic UI"),
        )
    };
    let child = |class: PCWSTR, text: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, id: i32| {
        let t = wide(text);
        let h = unsafe {
            CreateWindowExW(
                ex,
                class,
                PCWSTR(t.as_ptr()),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(id as usize as *mut _)),
                Some(hinst.into()),
                None,
            )
        };
        if let Ok(h) = h {
            unsafe {
                SendMessageW(h, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            }
        }
        h
    };
    let edit_style = WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32) | WS_VSCROLL | WS_TABSTOP;
    let edit = child(w!("EDIT"), &initial.replace('\n', "\r\n"), edit_style, WS_EX_CLIENTEDGE, ID_EDIT).ok();
    let _ = child(w!("STATIC"), "Ctrl+Enter: 確定    Esc: キャンセル", WINDOW_STYLE(0), WINDOW_EX_STYLE(0), ID_HINT);
    let _ = child(w!("BUTTON"), "OK", WINDOW_STYLE(BS_DEFPUSHBUTTON as u32) | WS_TABSTOP, WINDOW_EX_STYLE(0), IDOK.0);
    let _ = child(w!("BUTTON"), "キャンセル", WINDOW_STYLE(BS_PUSHBUTTON as u32) | WS_TABSTOP, WINDOW_EX_STYLE(0), IDCANCEL.0);
    layout(hwnd);

    unsafe {
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
        if let Some(e) = edit {
            let _ = SetFocus(Some(e));
            // キャレットを末尾へ
            SendMessageW(e, EM_SETSEL, Some(WPARAM(usize::MAX)), Some(LPARAM(-1)));
        }
    }

    let mut msg = MSG::default();
    while RESULT.get().is_none() {
        if !unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
            // WM_QUIT は外側のメッセージループに渡し直す
            unsafe { PostQuitMessage(msg.wParam.0 as i32) };
            RESULT.set(Some(false));
            break;
        }
        if msg.message == WM_KEYDOWN && ctrl_down() {
            if msg.wParam.0 == VK_RETURN.0 as usize {
                RESULT.set(Some(true));
                continue;
            }
            if msg.wParam.0 == 'A' as usize && Some(msg.hwnd) == edit {
                unsafe { SendMessageW(msg.hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1))) };
                continue;
            }
        }
        unsafe {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    let text = (RESULT.get() == Some(true)).then(|| edit.map(window_text).unwrap_or_default().replace("\r\n", "\n"));
    unsafe {
        // 先に親を有効にしてから閉じる (親がアクティブに戻るように)
        let _ = EnableWindow(owner, true);
        let _ = DestroyWindow(hwnd);
        let _ = SetForegroundWindow(owner);
        let _ = DeleteObject(HGDIOBJ(font.0));
    }
    text
}

fn ctrl_down() -> bool {
    unsafe { GetKeyState(VK_CONTROL.0 as i32) < 0 }
}

fn window_text(h: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(h) }.max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    let n = unsafe { GetWindowTextW(h, &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// 子ウィンドウの配置: 上に入力欄、下に説明とボタン。
fn layout(hwnd: HWND) {
    let s = SCALE.get();
    let px = |v: f32| (v * s) as i32;
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    let (w, h) = (rc.right, rc.bottom);
    let (margin, btn_w, btn_h) = (px(10.0), px(96.0), px(30.0));
    let bottom = h - margin - btn_h;
    let place = |id: i32, x: i32, y: i32, cw: i32, ch: i32| unsafe {
        if let Ok(c) = GetDlgItem(Some(hwnd), id) {
            let _ = MoveWindow(c, x, y, cw.max(0), ch.max(0), true);
        }
    };
    place(ID_EDIT, margin, margin, w - margin * 2, bottom - margin * 2);
    place(ID_HINT, margin, bottom + px(6.0), w - margin * 3 - btn_w * 2, btn_h);
    place(IDCANCEL.0, w - margin - btn_w, bottom, btn_w, btn_h);
    place(IDOK.0, w - margin * 2 - btn_w * 2, bottom, btn_w, btn_h);
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => match loword(wp.0) as i32 {
            id if id == IDOK.0 => RESULT.set(Some(true)),
            id if id == IDCANCEL.0 => RESULT.set(Some(false)),
            _ => {}
        },
        WM_CLOSE => RESULT.set(Some(false)),
        WM_SIZE => layout(hwnd),
        WM_GETMINMAXINFO => {
            let s = SCALE.get();
            let info = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
            info.ptMinTrackSize.x = (360.0 * s) as i32;
            info.ptMinTrackSize.y = (220.0 * s) as i32;
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}
