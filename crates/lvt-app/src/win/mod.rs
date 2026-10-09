//! Windows フロントエンドの入口: ウィンドウ生成・メッセージ処理・ダイアログを伴う処理。

mod app;
mod detail_editor;
mod dialogs;
mod input;
mod menu;
mod paint;
mod prompt;
mod render;
mod util;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use lvt_core::encoding::{SaveError, TextEncoding};
use lvt_core::settings::Settings;
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL;
use windows::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

use app::{App, Deferred, KeyOutcome};
use menu::Menus;
use util::{hiword, loword, point_from_lparam, wheel_delta, wide};

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// `App` を借用して処理する。再入 (モーダルダイアログ中のメッセージ等) で
/// 借用できない場合は何もしない。
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut b = cell.try_borrow_mut().ok()?;
        Some(f(b.as_mut()?))
    })
}

pub fn run() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    if let Err(e) = start() {
        let msg = wide(&format!("起動に失敗しました: {e}"));
        unsafe { MessageBoxW(None, PCWSTR(msg.as_ptr()), w!("lvt"), MB_OK | MB_ICONERROR) };
    }
}

fn start() -> Result<()> {
    unsafe {
        let hinst = HINSTANCE(GetModuleHandleW(None)?.0);
        let class = w!("lvt.main");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: LoadIconW(None, IDI_APPLICATION)?,
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(windows::core::Error::from_thread());
        }

        // 入力モード (標準 / Vim) は前回の設定を引き継ぐ
        let settings_path = app::settings_path();
        let settings = settings_path.as_deref().map(Settings::load).unwrap_or_default();
        let mode = app::make_mode(app::vim_by_default(&settings));
        let menus = Menus::build(mode.as_ref())?;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("lvt"),
            WS_OVERLAPPEDWINDOW | WS_VSCROLL | WS_HSCROLL,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            Some(menus.bar),
            Some(hinst),
            None,
        )?;
        let app = App::new(hwnd, mode, menus, settings, settings_path)?;
        APP.with(|c| *c.borrow_mut() = Some(app));
        DragAcceptFiles(hwnd, true);
        let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);

        let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        if !files.is_empty() {
            run_deferred(hwnd, Deferred::OpenPaths(files));
        }
        with_app(|a| a.refresh());

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let handled = match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                BeginPaint(hwnd, &mut ps);
                with_app(|a| a.paint());
                let _ = EndPaint(hwnd, &ps);
            }
            true
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_SIZE => {
            with_app(|a| a.refresh());
            true
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            with_app(|a| {
                a.focused = msg == WM_SETFOCUS;
                a.invalidate();
            });
            true
        }
        // Ctrl の押下・解放で、リンク上のカーソル (手の形) を切り替える
        WM_KEYDOWN | WM_KEYUP if wp.0 == VK_CONTROL.0 as usize => {
            with_app(|a| a.refresh_cursor());
            false
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => match with_app(|a| a.on_key_down(loword(wp.0))) {
            Some(KeyOutcome::Handled(d)) => {
                if let Some(d) = d {
                    run_deferred(hwnd, d);
                }
                true
            }
            _ => false,
        },
        WM_SYSCHAR => with_app(|a| std::mem::take(&mut a.suppress_syschar)).unwrap_or(false),
        WM_CHAR => {
            if let Some(d) = with_app(|a| a.on_char(loword(wp.0))).flatten() {
                run_deferred(hwnd, d);
            }
            true
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = point_from_lparam(lp);
            if let Some(d) = with_app(|a| a.on_mouse_down(x, y, msg == WM_LBUTTONDBLCLK)).flatten() {
                run_deferred(hwnd, d);
            }
            true
        }
        WM_MOUSEMOVE => {
            let (x, y) = point_from_lparam(lp);
            with_app(|a| a.on_mouse_move(x, y));
            true
        }
        WM_LBUTTONUP => {
            with_app(|a| a.on_mouse_up());
            true
        }
        WM_MBUTTONUP => {
            let (x, y) = point_from_lparam(lp);
            if let Some(d) = with_app(|a| a.on_middle_click(x, y)).flatten() {
                run_deferred(hwnd, d);
            }
            true
        }
        WM_TIMER => {
            with_app(|a| a.on_timer());
            true
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            with_app(|a| a.on_wheel(wheel_delta(wp), msg == WM_MOUSEHWHEEL));
            true
        }
        WM_VSCROLL | WM_HSCROLL => {
            with_app(|a| a.on_scroll(msg == WM_VSCROLL, loword(wp.0)));
            true
        }
        WM_SETCURSOR if loword(lp.0 as usize) as u32 == HTCLIENT => {
            with_app(|a| a.update_cursor()).is_some()
        }
        WM_COMMAND => {
            if let Some(cmd) = with_app(|a| a.menus.command(loword(wp.0))).flatten() {
                if let Some(d) = with_app(|a| a.execute_commands(vec![cmd])).flatten() {
                    run_deferred(hwnd, d);
                }
            }
            true
        }
        WM_INITMENUPOPUP => {
            with_app(|a| a.update_menu());
            true
        }
        // 本文への入力では変換中の文字列を自前で描画するので、既定処理 (IME の
        // 変換ウィンドウ) に渡さない。入力欄では既定処理に任せる。
        WM_IME_STARTCOMPOSITION => with_app(|a| {
            a.position_ime();
            a.prompt.is_none()
        })
        .unwrap_or(false),
        WM_IME_COMPOSITION => match with_app(|a| a.on_ime_composition(lp.0 as u32)).flatten() {
            Some(d) => {
                if let Some(d) = d {
                    run_deferred(hwnd, d);
                }
                true
            }
            None => false,
        },
        WM_IME_ENDCOMPOSITION => {
            with_app(|a| a.end_ime_composition());
            false
        }
        WM_DROPFILES => {
            let paths = dropped_files(HDROP(wp.0 as _));
            run_deferred(hwnd, Deferred::OpenPaths(paths));
            true
        }
        WM_DPICHANGED => {
            with_app(|a| a.set_dpi(hiword(wp.0) as f32));
            let rc = unsafe { *(lp.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rc.left,
                    rc.top,
                    rc.right - rc.left,
                    rc.bottom - rc.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            true
        }
        WM_CLOSE => {
            run_deferred(hwnd, Deferred::Exit);
            true
        }
        WM_DESTROY => {
            APP.with(|c| c.try_borrow_mut().map(|mut a| a.take()).ok());
            unsafe { PostQuitMessage(0) };
            true
        }
        _ => false,
    };
    if handled {
        LRESULT(0)
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }
}

fn dropped_files(hdrop: HDROP) -> Vec<PathBuf> {
    unsafe {
        let n = DragQueryFileW(hdrop, u32::MAX, None);
        let paths = (0..n)
            .map(|i| {
                let len = DragQueryFileW(hdrop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(hdrop, i, Some(&mut buf));
                PathBuf::from(String::from_utf16_lossy(&buf[..len]))
            })
            .collect();
        DragFinish(hdrop);
        paths
    }
}

// ---- モーダルダイアログを伴う処理 (App を借用していない状態で呼ぶ) ----

fn run_deferred(hwnd: HWND, d: Deferred) {
    match d {
        Deferred::Open => {
            let files = dialogs::open_files(hwnd);
            if !files.is_empty() {
                open_paths(hwnd, files);
            }
        }
        Deferred::OpenPaths(paths) => open_paths(hwnd, paths),
        Deferred::Save(i) => {
            save(hwnd, i, false);
        }
        Deferred::SaveAs(i) => {
            save(hwnd, i, true);
        }
        Deferred::CloseTab(i) => {
            close_tab(hwnd, i);
        }
        Deferred::Exit => {
            if confirm_close_all(hwnd) {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return;
            }
        }
        Deferred::Reopen(enc) => reopen(hwnd, enc),
        Deferred::OpenUrl(url) => dialogs::open_url(hwnd, &url),
        Deferred::EditDetail { idx, line } => {
            if let Some(initial) = with_app(|a| a.detail_text(idx, line)).flatten() {
                let title = format!("詳細の編集 ({} 行目)", line + 1);
                if let Some(text) = detail_editor::edit(hwnd, &title, &initial) {
                    with_app(|a| a.write_detail(idx, line, &text));
                }
            }
        }
        Deferred::Quit { idx, save: write, force } => {
            if write && !save(hwnd, idx, false) {
                with_app(|a| a.refresh());
                return;
            }
            let last_tab = with_app(|a| a.tabs.len()).unwrap_or(0) <= 1;
            if last_tab {
                if force || confirm_close_all(hwnd) {
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                    return;
                }
            } else if force {
                with_app(|a| a.remove_tab(idx));
            } else {
                close_tab(hwnd, idx);
            }
        }
    }
    with_app(|a| a.refresh());
}

fn open_paths(hwnd: HWND, paths: Vec<PathBuf>) {
    let errors = with_app(|a| a.open_paths(paths)).unwrap_or_default();
    if !errors.is_empty() {
        dialogs::error(hwnd, &format!("ファイルを開けませんでした。\n\n{}", errors.join("\n\n")));
    }
}

/// 保存する。保存できたら true。
fn save(hwnd: HWND, idx: usize, force_dialog: bool) -> bool {
    let Some((path, title)) =
        with_app(|a| a.tabs.get(idx).map(|t| (t.doc.path().map(Path::to_path_buf), t.doc.title()))).flatten()
    else {
        return false;
    };
    let path = match path {
        Some(p) if !force_dialog => p,
        other => {
            let suggested = other.unwrap_or_else(|| PathBuf::from(format!("{title}.txt")));
            match dialogs::save_file(hwnd, Some(&suggested)) {
                Some(p) => p,
                None => return false,
            }
        }
    };
    loop {
        match with_app(|a| a.save_tab(idx, &path)) {
            Some(Ok(())) => return true,
            Some(Err(e @ SaveError::Unmappable(_))) => {
                if !dialogs::ask_yes_no(hwnd, &format!("{e}。\n\n文字コードを UTF-8 に変更して保存しますか？")) {
                    return false;
                }
                with_app(|a| a.tabs[idx].doc.set_encoding(TextEncoding::Utf8));
            }
            Some(Err(e)) => {
                dialogs::error(hwnd, &format!("保存できませんでした。\n{}\n\n{e}", path.display()));
                return false;
            }
            None => return false,
        }
    }
}

/// 未保存なら確認する。続行してよければ true。
fn confirm_discard(hwnd: HWND, idx: usize) -> bool {
    let Some((modified, title)) =
        with_app(|a| a.tabs.get(idx).map(|t| (t.doc.is_modified(), t.doc.title()))).flatten()
    else {
        return true;
    };
    if !modified {
        return true;
    }
    with_app(|a| {
        a.activate(idx);
        a.refresh();
    });
    match dialogs::ask_yes_no_cancel(hwnd, &format!("{title} への変更を保存しますか？")) {
        dialogs::Answer::Yes => save(hwnd, idx, false),
        dialogs::Answer::No => true,
        dialogs::Answer::Cancel => false,
    }
}

fn close_tab(hwnd: HWND, idx: usize) {
    if confirm_discard(hwnd, idx) {
        with_app(|a| a.remove_tab(idx));
    }
}

fn confirm_close_all(hwnd: HWND) -> bool {
    let n = with_app(|a| a.tabs.len()).unwrap_or(0);
    (0..n).all(|i| confirm_discard(hwnd, i))
}

fn reopen(hwnd: HWND, enc: TextEncoding) {
    let modified = with_app(|a| a.tab().doc.is_modified()).unwrap_or(false);
    if modified && !dialogs::ask_yes_no(hwnd, "変更を破棄して開き直しますか？") {
        return;
    }
    if let Some(Err(e)) = with_app(|a| a.reopen(enc)) {
        dialogs::error(hwnd, &format!("開き直せませんでした。\n{e}"));
    }
}
