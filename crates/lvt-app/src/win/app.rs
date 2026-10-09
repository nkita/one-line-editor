//! アプリケーション状態 (タブ・入力モード・検索・表示状態) と、その操作。
//!
//! Win32 メッセージは win/mod.rs で解釈され、ここにあるメソッドに渡される。
//! モーダルダイアログが必要な処理は `Deferred` として呼び出し元に返し、
//! `App` の借用を解いてから実行する (ダイアログ中の再入で二重借用しないため)。

use std::path::{Path, PathBuf};
use std::time::Instant;

use lvt_core::editor::{self, EditorConfig, ViewInfo};
use lvt_core::encoding::{SaveError, TextEncoding};
use lvt_core::history::MergeKind;
use lvt_core::keymap::{InputMode, Key, KeyChord, Keymap, ModeResponse, StandardMode};
use lvt_core::detail;
use lvt_core::lineview::LineView;
use lvt_core::link;
use lvt_core::settings::Settings;
use lvt_core::vim::{self, ex, Register, VimEdit, VimMode};
use lvt_core::search::{self, SearchQuery};
use lvt_core::theme::Theme;
use lvt_core::{AppCommand, Command, Document, EditCommand, Selection};
use regex::Regex;
use windows::core::Result;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient, LOGFONTW, SHIFTJIS_CHARSET};
use windows::Win32::UI::Input::Ime::{
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext, ImmSetCandidateWindow, ImmSetCompositionFontW,
    ImmSetOpenStatus,
    ImmSetCompositionWindow, ATTR_TARGET_CONVERTED, ATTR_TARGET_NOTCONVERTED, CANDIDATEFORM, CFS_EXCLUDE, CFS_POINT,
    COMPOSITIONFORM, GCS_COMPATTR, GCS_COMPSTR, GCS_CURSORPOS, GCS_RESULTSTR, HIMC, IME_COMPOSITION_STRING,
};
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyMenu, GetClientRect, GetCursorPos, GetScrollInfo, SetMenu, WindowFromPoint, KillTimer, LoadCursorW, SetCursor, SetTimer, SetWindowTextW, IDC_ARROW,
    IDC_HAND, IDC_IBEAM, IDC_SIZEALL, IDC_WAIT, SB_HORZ, SB_VERT, SCROLLBAR_CONSTANTS, SCROLLINFO, SIF_ALL, SIF_DISABLENOSCROLL, SIF_TRACKPOS,
};

use super::input;
use super::menu::Menus;
use super::panel::{DetailEdit, PanelRect, PanelScrollbar};
use super::prompt::{Prompt, PromptKind};
use super::render::{Renderer, DEFAULT_FONT_SIZE};
use super::util::{char_to_utf16, utf16_to_char, wide};

/// 1 行のうち描画・ヒットテストの対象にする最大文字数 (極端に長い行対策)。
pub const MAX_RENDER_CHARS: usize = 20_000;
/// これより大きい文書ではインクリメンタル検索をしない (Enter で検索)。
const INCREMENTAL_SEARCH_LIMIT: usize = 32 << 20;
const AUTOSCROLL_TIMER: usize = 1;

pub struct Tab {
    pub doc: Document,
    pub scroll_line: usize,
    pub scroll_x: f32,
    /// これまでに描画した行の最大幅 (横スクロール範囲)。
    pub max_width: f32,
}

impl Tab {
    pub fn new(doc: Document) -> Self {
        Self { doc, scroll_line: 0, scroll_x: 0.0, max_width: 0.0 }
    }
}

/// `App` の借用外で行う必要がある処理 (モーダルダイアログを伴う)。
pub enum Deferred {
    Open,
    OpenPaths(Vec<PathBuf>),
    Save(usize),
    SaveAs(usize),
    CloseTab(usize),
    Exit,
    Reopen(TextEncoding),
    OpenUrl(String),
    /// 詳細のバッジの右クリックメニュー (`x`, `y` はクライアント座標)。
    DetailMenu { line: usize, x: i32, y: i32 },
    /// Vim の :q / :q! / :wq。最後のタブなら終了する。
    Quit { idx: usize, save: bool, force: bool },
}

/// IME で変換中の文字列。本文のキャレット位置に埋め込んで描画する。
pub struct ImeComposition {
    pub text: String,
    /// 変換中の文字列内のカーソル位置 (UTF-16)。
    pub cursor: usize,
    /// 変換対象の文節 (UTF-16 の範囲)。
    pub target: Option<std::ops::Range<usize>>,
}

pub enum KeyOutcome {
    Unhandled,
    Handled(Option<Deferred>),
}

/// 画面の領域 (DIP)。
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub width: f32,
    pub height: f32,
    pub tabbar_h: f32,
    pub text_top: f32,
    pub text_bottom: f32,
    pub prompt_top: f32,
    pub status_top: f32,
    pub gutter_w: f32,
    pub text_left: f32,
}

/// `App::hit_view` の結果。
struct ViewHit {
    line: usize,
    /// 表示用の行の文字位置。
    vcol: usize,
    view: LineView,
}

#[derive(Clone, Copy)]
pub struct TabHit {
    pub left: f32,
    pub right: f32,
    pub close_left: f32,
}

#[derive(Clone, Copy)]
enum Drag {
    Text,
    Lines { anchor_line: usize },
    /// タブの並べ替え。`moved` はしきい値を超えて動かしたか。
    Tab { idx: usize, start_x: f32, moved: bool },
    /// 詳細パネルの移動 (`ox`, `oy` はパネルの左上からのマウスの位置)。
    Panel { ox: f32, oy: f32 },
    /// 詳細パネルのスクロールバーのつまみ (`grab` はつまみの上端からのマウスの位置)。
    PanelScroll { grab: f32 },
}

/// タブのドラッグを開始とみなす移動量 (DIP)。
const TAB_DRAG_THRESHOLD: f32 = 4.0;

pub struct App {
    pub hwnd: HWND,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub mode: Box<dyn InputMode>,
    pub editor_cfg: EditorConfig,
    pub theme: Theme,
    pub renderer: Renderer,
    pub menus: Menus,
    pub prompt: Option<Prompt>,
    pub search: SearchQuery,
    pub search_re: Option<Regex>,
    /// 検索一致箇所を強調表示するか。
    pub highlight_matches: bool,
    last_replacement: String,
    pub status: Option<String>,
    pub focused: bool,
    pub tab_hits: Vec<TabHit>,
    drag: Option<Drag>,
    mouse: (f32, f32),
    wheel_accum: i32,
    high_surrogate: Option<u16>,
    pub ime: Option<ImeComposition>,
    pub suppress_syschar: bool,
    title: String,
    /// Vim キーバインドが有効か。
    pub vim_enabled: bool,
    /// 最後にヤンク・削除した文字列 (クリップボードの内容が行単位かどうかの判断に使う)。
    register: Option<Register>,
    /// 直前の入力の時点でノーマルモードだったか (ノーマルモードに戻ったら IME を閉じる)。
    was_normal: bool,
    pub(super) settings: Settings,
    settings_path: Option<PathBuf>,
    /// 入力欄の処理 (`:w` など) から返す `Deferred`。
    pending_deferred: Option<Deferred>,
    /// .txt で詳細の折りたたみ表示を使うか (.lvt は常に使う)。
    pub txt_detail_view: bool,
    /// 詳細パネルの領域。表示していなければ None (paint.rs が描画のたびに更新する)。
    pub detail_panel: Option<PanelRect>,
    /// パネル内で編集中の詳細。
    pub detail_edit: Option<DetailEdit>,
    /// パネルの位置 (本文の領域に対する割合。0,0 = 左上、1,0 = 右上)。None なら右上でキャレット行を避ける。
    pub panel_pos: Option<(f32, f32)>,
    /// 表示中のパネルの縦スクロール量 (DIP) と、その対象の行 (行が変わったら先頭に戻す)。
    pub panel_scroll: f32,
    pub panel_scroll_line: Option<usize>,
    /// パネルのスクロールバー (スクロールできるときだけ。paint.rs が更新する)。
    pub panel_scrollbar: Option<PanelScrollbar>,
    /// 表示中のパネルの見出しの削除ボタン (left, top, right, bottom)。paint.rs が更新する。
    pub panel_button: Option<(f32, f32, f32, f32)>,
}

/// Vim キーバインドを使うか。設定がなければ Vim を既定にする。
pub fn vim_by_default(settings: &Settings) -> bool {
    settings.get("input_mode") != Some("standard")
}

/// 入力モードを作る。
pub fn make_mode(vim: bool) -> Box<dyn InputMode> {
    if vim {
        Box::new(VimMode::new(Keymap::windows_default()))
    } else {
        Box::new(StandardMode::default())
    }
}

/// 設定ファイルの場所 (`%APPDATA%\lvt\settings.toml`)。
pub fn settings_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("lvt").join("settings.toml"))
}

impl App {
    pub fn new(
        hwnd: HWND,
        mode: Box<dyn InputMode>,
        menus: Menus,
        settings: Settings,
        settings_path: Option<PathBuf>,
    ) -> Result<Self> {
        let vim_enabled = vim_by_default(&settings);
        let txt_detail_view = settings.get("txt_detail_view") != Some("false");
        let panel_pos = settings.get("detail_panel_pos").and_then(|v| {
            let (x, y) = v.split_once(',')?;
            Some((x.trim().parse::<f32>().ok()?.clamp(0.0, 1.0), y.trim().parse::<f32>().ok()?.clamp(0.0, 1.0)))
        });
        let editor_cfg = EditorConfig::default();
        let renderer = Renderer::new(hwnd, editor_cfg.tab_width)?;
        Ok(Self {
            hwnd,
            tabs: vec![Tab::new(Document::new())],
            active: 0,
            mode,
            editor_cfg,
            theme: Theme::light(),
            renderer,
            menus,
            prompt: None,
            search: SearchQuery::default(),
            search_re: None,
            highlight_matches: false,
            last_replacement: String::new(),
            status: None,
            focused: true,
            tab_hits: Vec::new(),
            drag: None,
            mouse: (0.0, 0.0),
            wheel_accum: 0,
            high_surrogate: None,
            ime: None,
            suppress_syschar: false,
            title: String::new(),
            vim_enabled,
            register: None,
            was_normal: false,
            settings,
            settings_path,
            pending_deferred: None,
            txt_detail_view,
            detail_panel: None,
            detail_edit: None,
            panel_pos,
            panel_scroll: 0.0,
            panel_scroll_line: None,
            panel_scrollbar: None,
            panel_button: None,
        })
    }

    /// 操作対象のタブ。詳細をパネルで編集中は、その編集用のタブ。
    pub fn tab(&self) -> &Tab {
        match &self.detail_edit {
            Some(e) => &e.tab,
            None => &self.tabs[self.active],
        }
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        match &mut self.detail_edit {
            Some(e) => &mut e.tab,
            None => &mut self.tabs[self.active],
        }
    }

    /// 表示中のタブ (詳細の編集中でも本体)。
    pub fn main_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    /// 詳細の編集中に、一時的に本体を操作対象にして `f` を実行する (スクロールバーなど)。
    fn with_main<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let edit = self.detail_edit.take();
        let r = f(self);
        self.detail_edit = edit;
        r
    }

    fn doc(&self) -> &Document {
        &self.tab().doc
    }

    fn doc_mut(&mut self) -> &mut Document {
        &mut self.tab_mut().doc
    }

    // ---- 画面配置 ----

    /// 操作対象の本文の配置。詳細をパネルで編集中はパネルの内側。
    pub fn geometry(&self) -> Geometry {
        let g = self.main_geometry();
        if self.detail_edit.is_some() {
            self.edit_geometry(g)
        } else {
            g
        }
    }

    /// 本体の画面配置。
    pub fn main_geometry(&self) -> Geometry {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let r = &self.renderer;
        let s = r.scale();
        let width = (rc.right - rc.left) as f32 / s;
        let height = (rc.bottom - rc.top) as f32 / s;
        let tabbar_h = r.snap(r.ui_line_height + 12.0);
        let status_h = r.snap(r.ui_line_height + 8.0);
        let prompt_bar = self.prompt.as_ref().is_some_and(|p| !p.in_status_bar());
        let prompt_h = if prompt_bar { r.snap(r.line_height.max(r.ui_line_height) + 12.0) } else { 0.0 };
        let status_top = (height - status_h).max(tabbar_h);
        let prompt_top = (status_top - prompt_h).max(tabbar_h);
        let digits = self.main_tab().doc.buffer().len_lines().to_string().len().max(3);
        // 装飾表示では、行番号の右隣に詳細のバッジの欄を設ける
        let badge = if self.main_tab().doc.detail_view() { 2.0 } else { 0.0 };
        let gutter_w = r.snap((digits as f32 + 2.0 + badge) * r.char_width);
        Geometry {
            width,
            height,
            tabbar_h,
            text_top: tabbar_h,
            text_bottom: prompt_top,
            prompt_top,
            status_top,
            gutter_w,
            text_left: gutter_w + r.snap(r.char_width * 0.5),
        }
    }

    /// 完全に表示できる行数。
    pub fn visible_lines(&self) -> usize {
        let g = self.geometry();
        (((g.text_bottom - g.text_top) / self.renderer.line_height).floor() as usize).max(1)
    }

    pub fn line_text(doc: &Document, line: usize) -> String {
        let s = doc.buffer().line(line);
        let n = s.len_chars().min(MAX_RENDER_CHARS);
        s.slice(..n).to_string()
    }

    /// 表示用の行。装飾表示 (.txt / .lvt) では詳細をアイコンにし、記法の記号を隠す
    /// (キャレットのある要素だけは記法のまま)。
    pub fn view_line(doc: &Document, line: usize) -> LineView {
        let text = Self::line_text(doc, line);
        if !doc.detail_view() {
            return LineView::plain(&text);
        }
        let (caret_line, caret_col) = doc.buffer().line_col(doc.caret_pos());
        LineView::build(&text, (caret_line == line).then_some(caret_col))
    }

    /// 行内の文字位置の x 座標 (行頭からの DIP)。
    fn col_x(&self, line: usize, col: usize) -> f32 {
        let view = Self::view_line(self.doc(), line);
        let vcol = view.to_view(col, false);
        let text = view.text;
        let u: Vec<u16> = text.encode_utf16().collect();
        match self.renderer.layout(&u, &self.renderer.text_format) {
            Some(l) => Renderer::x_of(&l, char_to_utf16(&text, vcol)),
            None => vcol as f32 * self.renderer.char_width,
        }
    }

    /// .txt / .lvt かどうか (と設定) で、各タブの詳細の折りたたみを有効・無効にする。
    fn update_detail_views(&mut self) {
        let txt = self.txt_detail_view;
        for t in &mut self.tabs {
            let ext = t.doc.path().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_ascii_lowercase());
            let on = match ext.as_deref() {
                Some("lvt") => true,
                Some("txt") => txt,
                _ => false,
            };
            if t.doc.detail_view() != on {
                t.doc.set_detail_view(on);
            }
        }
    }

    /// 詳細の内容 (エスケープを戻したもの。行に詳細がなければ空)。
    pub fn detail_text(&self, idx: usize, line: usize) -> Option<String> {
        let doc = &self.tabs.get(idx)?.doc;
        let Some(t) = doc.detail_at_line(line) else { return Some(String::new()) };
        let ls = doc.buffer().line_to_char(line);
        let c = t.content();
        Some(detail::unescape(&doc.buffer().slice_to_string(ls + c.start..ls + c.end)))
    }

    /// 編集した文字列を書き戻す (1 回の Undo で戻せる)。行に詳細がなければ行末に作る。
    pub fn write_detail(&mut self, idx: usize, line: usize, text: &str) {
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        let doc = &mut tab.doc;
        if line >= doc.buffer().len_lines() {
            return;
        }
        let escaped = detail::escape(text);
        let ls = doc.buffer().line_to_char(line);
        let n = escaped.chars().count();
        match doc.detail_at_line(line) {
            Some(t) => {
                let c = t.content();
                if doc.buffer().slice_to_string(ls + c.start..ls + c.end) == escaped {
                    return;
                }
                let end = ls + c.start + n + 1;
                doc.apply(vec![(ls + c.start..ls + c.end, escaped)], MergeKind::None, |_, _| Selection::caret(end));
            }
            None if text.is_empty() => return,
            None => {
                let line_text = doc.buffer().line(line);
                let len = line_text.len_chars();
                let sep = if len > 0 && !matches!(line_text.char(len - 1), ' ' | '\t') { " " } else { "" };
                let at = ls + len;
                let caret = doc.selection().head;
                let token = format!("{sep}{}{escaped}]", detail::PREFIX);
                doc.apply(vec![(at..at, token)], MergeKind::None, |_, _| Selection::caret(caret));
            }
        }
        doc.end_undo_group();
        if idx == self.active {
            self.ensure_caret_visible();
        }
        self.status = Some("詳細を更新しました".into());
        self.refresh();
    }

    // ---- 再描画・スクロール ----

    /// 状態変更後の共通処理: スクロールバー・タイトル更新と再描画。
    pub fn refresh(&mut self) {
        if self.detail_edit.is_some() {
            self.clamp_scroll();
        }
        // スクロールバーとタイトルは常に本体を表す
        self.with_main(|a| {
            a.clamp_scroll();
            a.update_scrollbars();
            a.update_title();
        });
        self.invalidate();
    }

    pub(super) fn save_settings(&mut self) {
        if let Some(path) = &self.settings_path {
            if let Err(e) = self.settings.save(path) {
                self.status = Some(format!("設定を保存できませんでした: {e}"));
            }
        }
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn update_title(&mut self) {
        let doc = self.doc();
        let title = format!(
            "{}{} - lvt",
            if doc.is_modified() { "* " } else { "" },
            doc.path().map(|p| p.display().to_string()).unwrap_or_else(|| doc.title())
        );
        if title != self.title {
            let w = wide(&title);
            unsafe {
                let _ = SetWindowTextW(self.hwnd, windows::core::PCWSTR(w.as_ptr()));
            }
            self.title = title;
        }
    }

    fn clamp_scroll(&mut self) {
        let vis = self.visible_lines();
        let max_x = (self.content_width() - self.text_area_width()).max(0.0);
        let tab = self.tab_mut();
        let max = tab.doc.buffer().len_lines().saturating_sub(vis);
        tab.scroll_line = tab.scroll_line.min(max);
        tab.scroll_x = tab.scroll_x.clamp(0.0, max_x);
    }

    /// 本文の表示領域の幅 (DIP)。
    fn text_area_width(&self) -> f32 {
        let g = self.geometry();
        (g.width - g.text_left).max(1.0)
    }

    /// 横スクロールの対象とする内容の幅 (最長行の右に少し余白を足す)。
    fn content_width(&self) -> f32 {
        self.tab().max_width + self.renderer.char_width * 4.0
    }

    fn update_scrollbars(&self) {
        let vis = self.visible_lines();
        let tab = self.tab();
        let lines = tab.doc.buffer().len_lines();
        let set = |bar: SCROLLBAR_CONSTANTS, max: i32, page: u32, pos: i32| {
            let si = SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_ALL | SIF_DISABLENOSCROLL,
                nMin: 0,
                nMax: max,
                nPage: page,
                nPos: pos,
                nTrackPos: 0,
            };
            unsafe { SetScrollInfo(self.hwnd, bar, &si, true) };
        };
        set(SB_VERT, (lines.saturating_sub(1)).min(i32::MAX as usize) as i32, vis as u32, tab.scroll_line as i32);
        set(SB_HORZ, self.content_width() as i32, self.text_area_width() as u32, tab.scroll_x as i32);
    }

    pub fn scroll_by(&mut self, lines: i64) {
        let tab = self.tab_mut();
        tab.scroll_line = (tab.scroll_line as i64 + lines).max(0) as usize;
        self.refresh();
    }

    pub fn scroll_x_by(&mut self, dx: f32) {
        self.tab_mut().scroll_x += dx;
        self.refresh();
    }

    /// キャレットが見えるようにスクロールする。
    pub fn ensure_caret_visible(&mut self) {
        let vis = self.visible_lines();
        let g = self.geometry();
        let (line, col) = {
            let doc = self.doc();
            doc.buffer().line_col(doc.caret_pos())
        };
        let x = self.col_x(line, col);
        let cw = self.renderer.char_width;
        let text_w = (g.width - g.text_left).max(cw * 4.0);
        let tab = self.tab_mut();
        if line < tab.scroll_line {
            tab.scroll_line = line;
        } else if line >= tab.scroll_line + vis {
            tab.scroll_line = line + 1 - vis;
        }
        tab.max_width = tab.max_width.max(x);
        if x < tab.scroll_x {
            tab.scroll_x = (x - text_w / 4.0).max(0.0);
        } else if x > tab.scroll_x + text_w - cw * 2.0 {
            tab.scroll_x = x - text_w * 3.0 / 4.0;
        }
    }

    /// 指定行が画面中央付近に来るようにスクロールする (検索・行移動用)。
    fn center_line(&mut self, line: usize) {
        let vis = self.visible_lines();
        let tab = self.tab_mut();
        if line < tab.scroll_line || line >= tab.scroll_line + vis {
            tab.scroll_line = line.saturating_sub(vis / 2);
        }
    }

    // ---- コマンド ----

    pub fn execute(&mut self, cmd: Command) -> Option<Deferred> {
        self.status = None;
        let deferred = match cmd {
            Command::Edit(e) => {
                let view =
                    ViewInfo { page_lines: self.visible_lines().saturating_sub(1).max(1), top_line: self.tab().scroll_line };
                let cfg = self.editor_cfg.clone();
                if let Some(r) = editor::execute(self.doc_mut(), &e, &cfg, &view) {
                    self.set_register(r);
                }
                self.ensure_caret_visible();
                None
            }
            Command::App(a) => self.execute_app(a),
        };
        self.refresh();
        deferred
    }

    /// 入力 (キー・文字・メニュー) から得たコマンド列を実行する。
    pub fn execute_commands(&mut self, cmds: Vec<Command>) -> Option<Deferred> {
        let mut deferred = None;
        for c in cmds {
            deferred = self.execute(c);
            if deferred.is_some() {
                break;
            }
        }
        self.after_input();
        deferred
    }

    /// 入力の処理が終わったあとの共通処理。Vim のノーマルモードではキャレットを整え、
    /// ノーマルモードに戻ったときは IME を閉じる (日本語入力のまま j などを押さないように)。
    fn after_input(&mut self) {
        if self.drag.is_some() {
            return;
        }
        let normal = self.mode.normal_caret();
        if normal {
            vim::normalize_caret(self.doc_mut());
            if !self.was_normal {
                self.close_ime();
            }
        }
        self.was_normal = normal;
        self.refresh();
    }

    fn close_ime(&self) {
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if !himc.is_invalid() {
                let _ = ImmSetOpenStatus(himc, false);
                let _ = ImmReleaseContext(self.hwnd, himc);
            }
        }
    }

    /// ヤンク・削除した文字列をクリップボードに置く。
    fn set_register(&mut self, r: Register) {
        if !r.text.is_empty() {
            super::dialogs::set_clipboard(self.hwnd, &r.text);
            self.register = Some(r);
        }
    }

    /// Vim キーバインドの有効・無効を切り替え、設定に保存する。
    fn set_vim(&mut self, on: bool) {
        self.finish_detail_edit(true);
        for t in &mut self.tabs {
            t.doc.end_undo_group();
            vim::leave_visual(&mut t.doc);
        }
        self.mode = make_mode(on);
        self.vim_enabled = on;
        self.was_normal = false;
        // ショートカットの表示が変わるのでメニューを作り直す
        if let Ok(menus) = Menus::build(self.mode.as_ref()) {
            let old = std::mem::replace(&mut self.menus, menus);
            unsafe {
                let _ = SetMenu(self.hwnd, Some(self.menus.bar));
                let _ = DestroyMenu(old.bar);
            }
        }
        self.settings.set("input_mode", if on { "vim" } else { "standard" });
        self.status = Some(if on { "Vim キーバインド: オン" } else { "Vim キーバインド: オフ" }.into());
        self.save_settings();
    }

    fn execute_app(&mut self, cmd: AppCommand) -> Option<Deferred> {
        use AppCommand as A;
        // 本体のファイル・タブ・設定に関わる操作の前に、編集中の詳細を反映して閉じる
        if self.detail_edit.is_some()
            && matches!(
                cmd,
                A::NewTab
                    | A::Open
                    | A::Save
                    | A::SaveAs
                    | A::CloseTab
                    | A::Exit
                    | A::NextTab
                    | A::PrevTab
                    | A::MoveTab(_)
                    | A::ReopenWithEncoding(_)
                    | A::SetSaveEncoding(_)
                    | A::SetLineEnding(_)
                    | A::ToggleTxtDetailView
            )
        {
            self.finish_detail_edit(true);
        }
        match cmd {
            A::NewTab => {
                self.tabs.push(Tab::new(Document::new()));
                self.activate(self.tabs.len() - 1);
            }
            A::Open => return Some(Deferred::Open),
            A::Save => return Some(Deferred::Save(self.active)),
            A::SaveAs => return Some(Deferred::SaveAs(self.active)),
            A::CloseTab => return Some(Deferred::CloseTab(self.active)),
            A::Exit => return Some(Deferred::Exit),
            A::NextTab => self.activate((self.active + 1) % self.tabs.len()),
            A::PrevTab => self.activate((self.active + self.tabs.len() - 1) % self.tabs.len()),
            A::MoveTab(delta) => {
                let to = (self.active as i64 + delta as i64).clamp(0, self.tabs.len() as i64 - 1) as usize;
                self.move_tab(self.active, to);
            }
            A::Cut => {
                if self.copy() {
                    return self.execute(EditCommand::DeleteSelection.into());
                }
            }
            A::Copy => {
                self.copy();
            }
            A::Paste => {
                if let Some(text) = super::dialogs::get_clipboard(self.hwnd) {
                    return self.execute(EditCommand::InsertText(text).into());
                }
            }
            A::Find => self.open_prompt(PromptKind::Find),
            A::Replace => self.open_prompt(PromptKind::ReplaceFind),
            A::GoToLine => self.open_prompt(PromptKind::GoToLine),
            A::FindNext => self.find(true),
            A::FindPrev => self.find(false),
            A::ZoomIn => self.set_font_size(self.renderer.font_size + 1.0),
            A::ZoomOut => self.set_font_size(self.renderer.font_size - 1.0),
            A::ZoomReset => self.set_font_size(DEFAULT_FONT_SIZE),
            A::ToggleTheme => {
                self.theme = if self.theme.name == "dark" { Theme::light() } else { Theme::dark() };
            }
            A::ReopenWithEncoding(e) => return Some(Deferred::Reopen(e)),
            A::SetSaveEncoding(e) => {
                self.doc_mut().set_encoding(e);
                self.status = Some(format!("保存時の文字コード: {}", e.name()));
            }
            A::SetLineEnding(e) => {
                self.doc_mut().set_line_ending(e);
                self.status = Some(format!("改行コード: {}", e.name()));
            }
            A::ScrollLines(n) => self.scroll_by(n as i64),
            A::ToggleVim => self.set_vim(!self.vim_enabled),
            A::VimPaste { before, count } => {
                let text = super::dialogs::get_clipboard(self.hwnd)?;
                // 自分でヤンクした内容ならその種類、外から来た文字列なら改行で終わるかで判断する
                let linewise = match &self.register {
                    Some(r) if r.text == text => r.linewise,
                    _ => text.ends_with('\n'),
                };
                return self.execute(EditCommand::Vim(VimEdit::Paste { text, linewise, before, count }).into());
            }
            A::VimCommandLine { visual } => {
                let range = if visual { vim::visual_line_range(self.doc()) } else { None };
                vim::leave_visual(self.doc_mut());
                let initial = if range.is_some() { "'<,'>".to_string() } else { String::new() };
                let origin = self.doc().caret_pos();
                self.prompt = Some(Prompt::new(PromptKind::Command { visual: range }, initial, origin));
            }
            A::VimSearch { forward } => {
                let origin = self.doc().caret_pos();
                self.prompt = Some(Prompt::new(PromptKind::VimSearch { forward }, String::new(), origin));
            }
            A::SearchWord { forward } => self.search_word(forward),
            A::EditDetail => {
                if self.detail_edit.is_some() {
                    self.finish_detail_edit(true);
                } else {
                    let doc = &self.main_tab().doc;
                    let line = doc.buffer().char_to_line(doc.caret_pos());
                    self.start_detail_edit(line);
                }
            }
            A::ToggleTxtDetailView => {
                self.txt_detail_view = !self.txt_detail_view;
                self.update_detail_views();
                self.settings.set("txt_detail_view", if self.txt_detail_view { "true" } else { "false" });
                self.status = Some(format!(".txt の装飾表示: {}", if self.txt_detail_view { "オン" } else { "オフ" }));
                self.save_settings();
            }
            A::Cancel => {
                if self.prompt.take().is_none() {
                    if self.detail_edit.is_some() {
                        self.finish_detail_edit(true);
                    } else {
                        self.highlight_matches = false;
                    }
                }
            }
        }
        None
    }

    pub fn activate(&mut self, idx: usize) {
        if idx != self.active {
            self.finish_detail_edit(true);
        }
        if idx < self.tabs.len() {
            if idx != self.active {
                let doc = self.doc_mut();
                doc.end_undo_group();
                vim::leave_visual(doc);
                self.mode.reset();
            }
            self.active = idx;
            self.drag = None;
        }
    }

    /// タブを `from` から `to` へ移す。アクティブなタブは同じ文書を指し続ける。
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = moved_index(self.active, from, to);
        // 次の描画までのマウス移動でも正しく判定できるよう、当たり判定も並べ替える
        if self.tab_hits.len() == self.tabs.len() {
            let hit = self.tab_hits.remove(from);
            self.tab_hits.insert(to, hit);
            let mut x = self.tab_hits[0].left.min(hit.left);
            for h in &mut self.tab_hits {
                let (w, close) = (h.right - h.left, h.right - h.close_left);
                *h = TabHit { left: x, right: x + w, close_left: x + w - close };
                x += w;
            }
        }
    }

    fn set_font_size(&mut self, size: f32) {
        if self.renderer.set_font_size(size).is_ok() {
            for t in &mut self.tabs {
                t.max_width = 0.0;
            }
            self.status = Some(format!("文字サイズ: {:.0}", self.renderer.font_size));
            self.ensure_caret_visible();
        }
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        let _ = self.renderer.set_dpi(dpi);
        self.refresh();
    }

    fn copy(&mut self) -> bool {
        let doc = self.doc();
        if doc.selection().is_empty() {
            return false;
        }
        let text = doc.selected_text();
        super::dialogs::set_clipboard(self.hwnd, &text)
    }

    // ---- ファイル ----

    /// ファイルを開く。エラーメッセージの一覧を返す。
    pub fn open_paths(&mut self, paths: Vec<PathBuf>) -> Vec<String> {
        self.finish_detail_edit(true);
        let mut errors = Vec::new();
        for path in paths {
            let path = std::path::absolute(&path).unwrap_or(path);
            if let Some(i) = self.tabs.iter().position(|t| t.doc.path() == Some(path.as_path())) {
                self.activate(i);
                continue;
            }
            set_wait_cursor();
            let start = Instant::now();
            match Document::open(&path, None) {
                Ok((doc, had_errors)) => {
                    let tab = Tab::new(doc);
                    if self.doc().is_pristine() {
                        self.tabs[self.active] = tab;
                    } else {
                        self.tabs.push(tab);
                        self.active = self.tabs.len() - 1;
                    }
                    let mut msg = format!("読み込み完了 ({:.2} 秒)", start.elapsed().as_secs_f64());
                    if had_errors {
                        msg.push_str(" ※不正なバイト列を U+FFFD に置き換えました");
                    }
                    self.status = Some(msg);
                }
                Err(e) => errors.push(format!("{}\n{}", path.display(), e)),
            }
        }
        self.update_detail_views();
        self.refresh();
        errors
    }

    pub fn save_tab(&mut self, idx: usize, path: &Path) -> std::result::Result<(), SaveError> {
        set_wait_cursor();
        let tab = self.tabs.get_mut(idx).ok_or_else(|| SaveError::Io(std::io::Error::other("tab not found")))?;
        tab.doc.save_as(path)?;
        self.update_detail_views(); // 拡張子が変わった場合
        self.status = Some(format!("保存しました: {}", path.display()));
        self.refresh();
        Ok(())
    }

    pub fn remove_tab(&mut self, idx: usize) {
        self.finish_detail_edit(true);
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.tabs.push(Tab::new(Document::new()));
        }
        if self.active >= self.tabs.len() || self.active > idx {
            self.active = self.active.saturating_sub(1).min(self.tabs.len() - 1);
        }
        self.refresh();
    }

    pub fn reopen(&mut self, enc: TextEncoding) -> std::result::Result<(), String> {
        let Some(path) = self.doc().path().map(Path::to_path_buf) else { return Ok(()) };
        set_wait_cursor();
        let (doc, _) = Document::open(&path, Some(enc)).map_err(|e| format!("{}\n{e}", path.display()))?;
        let tab = self.tab_mut();
        let line = tab.scroll_line;
        *tab = Tab::new(doc);
        tab.scroll_line = line;
        self.update_detail_views();
        self.status = Some(format!("{} で開き直しました", enc.name()));
        self.refresh();
        Ok(())
    }

    // ---- 検索・プロンプト ----

    fn open_prompt(&mut self, kind: PromptKind) {
        let doc = self.doc();
        let sel = doc.selection();
        let initial = match kind {
            PromptKind::GoToLine | PromptKind::Command { .. } | PromptKind::VimSearch { .. } => String::new(),
            _ => {
                let s = doc.selected_text();
                if !s.is_empty() && !s.contains('\n') && s.len() < 1000 {
                    s
                } else {
                    self.search.pattern.clone()
                }
            }
        };
        self.prompt = Some(Prompt::new(kind, initial, sel.start()));
        self.apply_prompt_query();
    }

    /// 入力欄の内容を検索条件に反映する。
    fn apply_prompt_query(&mut self) {
        let Some(p) = &self.prompt else { return };
        if !p.is_search() {
            return;
        }
        self.search.pattern = p.text.clone();
        self.compile_search();
    }

    fn compile_search(&mut self) {
        if self.search.pattern.is_empty() {
            self.search_re = None;
            return;
        }
        match self.search.compile() {
            Ok(re) => self.search_re = Some(re),
            Err(e) => {
                self.search_re = None;
                self.status = Some(format!("正規表現エラー: {}", e.to_string().lines().last().unwrap_or_default()));
            }
        }
    }

    fn prompt_changed(&mut self) {
        self.status = None;
        self.apply_prompt_query();
        let Some(p) = &self.prompt else { return };
        let incremental = matches!(p.kind, PromptKind::Find | PromptKind::VimSearch { .. });
        if !incremental || self.doc().buffer().len_bytes() > INCREMENTAL_SEARCH_LIMIT {
            return;
        }
        let origin = p.origin;
        let kind = p.kind.clone();
        match self.search_re.clone() {
            Some(re) => {
                self.highlight_matches = true;
                let buf = self.doc().buffer();
                let found = match kind {
                    // Vim はカーソル位置の一致を飛ばす
                    PromptKind::VimSearch { forward: true } => {
                        search::find_next(buf, &re, (origin + 1).min(buf.len_chars()), true)
                    }
                    PromptKind::VimSearch { forward: false } => search::find_prev(buf, &re, origin, true),
                    _ => search::find_next(buf, &re, origin, true),
                };
                match found {
                    Some(r) => self.select_match(r),
                    None => self.status = Some("見つかりません".into()),
                }
            }
            None => self.doc_mut().set_selection(Selection::caret(origin)),
        }
    }

    fn select_match(&mut self, r: std::ops::Range<usize>) {
        let line = self.doc().buffer().char_to_line(r.start);
        if self.doc().visual.is_some() {
            vim::set_visual_cursor(self.doc_mut(), r.start);
        } else {
            self.doc_mut().set_selection(Selection::new(r.start, r.end));
        }
        self.center_line(line);
        self.ensure_caret_visible();
    }

    fn find(&mut self, forward: bool) {
        if self.search_re.is_none() {
            self.compile_search();
        }
        let Some(re) = self.search_re.clone() else {
            if self.search.pattern.is_empty() {
                self.open_prompt(PromptKind::Find);
            }
            return;
        };
        self.highlight_matches = true;
        let sel = self.doc().selection();
        let buf = self.doc().buffer();
        // Vim のノーマル・ビジュアルモードでは、キャレット位置の一致を飛ばして次を探す
        let vim_caret = (self.mode.normal_caret() || self.doc().visual.is_some()).then(|| self.doc().caret_pos());
        let (from, result) = if forward {
            let from = match vim_caret {
                Some(p) => (p + 1).min(buf.len_chars()),
                None if sel.is_empty() => sel.head,
                None => sel.end(),
            };
            (from, search::find_next(buf, &re, from, true))
        } else {
            let before = vim_caret.unwrap_or(sel.start());
            (before, search::find_prev(buf, &re, before, true))
        };
        match result {
            Some(r) => {
                let wrapped = if forward { r.start < from } else { r.start >= from };
                self.select_match(r);
                if wrapped {
                    self.status = Some(if forward { "末尾まで検索したので先頭から検索しました" } else { "先頭まで検索したので末尾から検索しました" }.into());
                }
            }
            None => self.status = Some(format!("見つかりません: {}", self.search.pattern)),
        }
    }

    fn replace_all(&mut self, replacement: &str) {
        let Some(re) = self.search_re.clone() else { return };
        set_wait_cursor();
        let edits = search::replace_all(self.doc().buffer(), &self.search, &re, replacement);
        let n = edits.len();
        let caret = self.doc().selection().start();
        self.doc_mut().apply(edits, MergeKind::None, |b, _| Selection::caret(caret.min(b.len_chars())));
        self.status = Some(format!("{n} 件置換しました"));
        self.ensure_caret_visible();
    }

    fn goto(&mut self, input: &str) {
        let mut parts = input.trim().splitn(2, [':', ',']);
        let Some(line) = parts.next().and_then(|s| s.trim().parse::<usize>().ok()) else {
            self.status = Some("行番号を入力してください".into());
            return;
        };
        let col = parts.next().and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1);
        let buf = self.doc().buffer();
        let line = line.clamp(1, buf.len_lines()) - 1;
        let pos = buf.line_to_char(line) + (col.max(1) - 1).min(buf.line_len(line));
        self.doc_mut().set_selection(Selection::caret(pos));
        self.center_line(line);
        self.ensure_caret_visible();
    }

    fn prompt_submit(&mut self, shift: bool) {
        let Some(p) = self.prompt.take() else { return };
        match p.kind.clone() {
            PromptKind::Find => {
                self.prompt = Some(p);
                self.apply_prompt_query();
                self.find(!shift);
            }
            PromptKind::ReplaceFind => {
                self.search.pattern = p.text.clone();
                self.compile_search();
                if self.search_re.is_some() {
                    self.highlight_matches = true;
                    self.prompt =
                        Some(Prompt::new(PromptKind::ReplaceWith { find: p.text }, self.last_replacement.clone(), p.origin));
                } else {
                    self.prompt = Some(p);
                }
            }
            PromptKind::ReplaceWith { .. } => {
                self.last_replacement = p.text.clone();
                self.replace_all(&p.text);
            }
            PromptKind::GoToLine => self.goto(&p.text),
            PromptKind::VimSearch { forward } => {
                self.search.pattern = p.text.clone();
                self.compile_search();
                if let Some(re) = self.search_re.clone() {
                    self.highlight_matches = true;
                    let buf = self.doc().buffer();
                    let found = if forward {
                        search::find_next(buf, &re, (p.origin + 1).min(buf.len_chars()), true)
                    } else {
                        search::find_prev(buf, &re, p.origin, true)
                    };
                    match found {
                        Some(r) => self.select_match(r),
                        None => {
                            self.doc_mut().set_selection(Selection::caret(p.origin));
                            self.status = Some(format!("見つかりません: {}", self.search.pattern));
                        }
                    }
                }
            }
            PromptKind::Command { visual } => self.run_ex(&p.text, visual),
        }
    }

    /// Vim の `*` / `#`。
    fn search_word(&mut self, forward: bool) {
        let pos = self.doc().caret_pos();
        let r = editor::word_at(self.doc().buffer(), pos);
        if r.is_empty() {
            self.status = Some("カーソル位置に単語がありません".into());
            return;
        }
        let word = self.doc().buffer().slice_to_string(r.clone());
        self.search.pattern = if self.search.regex { format!(r"\b{}\b", regex::escape(&word)) } else { word };
        self.search_re = None;
        self.doc_mut().set_selection(Selection::caret(r.start));
        self.find(forward);
    }

    /// `:` コマンドラインの実行。
    fn run_ex(&mut self, text: &str, visual: Option<(usize, usize)>) {
        let cmd = match ex::parse(text) {
            Ok(c) => c,
            Err(e) => {
                if !e.is_empty() {
                    self.status = Some(e);
                }
                return;
            }
        };
        let idx = self.active;
        let buf = self.doc().buffer();
        let current = buf.char_to_line(self.doc().caret_pos());
        let n = buf.len_lines();
        match cmd {
            // 詳細の編集中: :w で反映、:q / :wq で反映して閉じる、:q! で破棄して閉じる
            ex::ExCommand::Write if self.detail_edit.is_some() => self.write_detail_edit(),
            ex::ExCommand::Quit { force } if self.detail_edit.is_some() => self.finish_detail_edit(!force),
            ex::ExCommand::WriteQuit if self.detail_edit.is_some() => self.finish_detail_edit(true),
            ex::ExCommand::Write => self.pending_deferred = Some(Deferred::Save(idx)),
            ex::ExCommand::Quit { force } => self.pending_deferred = Some(Deferred::Quit { idx, save: false, force }),
            ex::ExCommand::WriteQuit => self.pending_deferred = Some(Deferred::Quit { idx, save: true, force: false }),
            ex::ExCommand::NoHighlight => self.highlight_matches = false,
            ex::ExCommand::GotoLine(spec) => {
                let line = spec.resolve(current, n);
                let pos = vim::motion::first_nonblank(buf, line);
                self.doc_mut().set_selection(Selection::caret(pos));
                self.center_line(line);
                self.ensure_caret_visible();
            }
            ex::ExCommand::Substitute { range, pattern, replacement, global, ignore_case } => {
                let lines = match range {
                    ex::ExRange::Current => current..current + 1,
                    ex::ExRange::All => 0..n,
                    ex::ExRange::Lines(a, b) => {
                        let (a, b) = (a.resolve(current, n), b.resolve(current, n));
                        a.min(b)..a.max(b) + 1
                    }
                    ex::ExRange::Visual => match visual {
                        Some((a, b)) => a..b + 1,
                        None => {
                            self.status = Some("'<,'> の範囲がありません".into());
                            return;
                        }
                    },
                };
                let re = match ex::compile(&pattern, ignore_case) {
                    Ok(re) => re,
                    Err(e) => {
                        self.status = Some(e);
                        return;
                    }
                };
                set_wait_cursor();
                let edits = ex::substitute(buf, lines, &re, &ex::translate_replacement(&replacement), global);
                let count = edits.len();
                if count == 0 {
                    self.status = Some(format!("見つかりません: {pattern}"));
                    return;
                }
                let first = edits.iter().map(|e| e.0.start).min().unwrap_or(0);
                self.doc_mut().apply(edits, MergeKind::None, |b, _| {
                    let line = b.char_to_line(first.min(b.len_chars()));
                    Selection::caret(vim::motion::first_nonblank(b, line))
                });
                self.status = Some(format!("{count} 件置換しました"));
                self.ensure_caret_visible();
            }
        }
    }

    /// 入力欄が開いているときのキー処理。処理したら true。
    fn prompt_key(&mut self, chord: KeyChord) -> bool {
        let m = chord.mods;
        let Some(p) = self.prompt.as_mut() else { return false };
        let is_search = p.is_search();
        let changed = match (chord.key, m.ctrl, m.alt) {
            (Key::Enter, false, false) => {
                self.prompt_submit(m.shift);
                return true;
            }
            (Key::Escape, _, _) => {
                // Vim の / ? は、取り消したら検索前の位置に戻る
                if let PromptKind::VimSearch { .. } = p.kind {
                    let origin = p.origin;
                    self.doc_mut().set_selection(Selection::caret(origin));
                }
                self.prompt = None;
                return true;
            }
            (Key::Backspace, false, false) => p.backspace(),
            (Key::Delete, false, false) => p.delete(),
            (Key::Left, false, false) => {
                p.left();
                false
            }
            (Key::Right, false, false) => {
                p.right();
                false
            }
            (Key::Home, false, false) => {
                p.home();
                false
            }
            (Key::End, false, false) => {
                p.end();
                false
            }
            (Key::Char('c'), false, true) if is_search => {
                self.search.case_sensitive = !self.search.case_sensitive;
                true
            }
            (Key::Char('r'), false, true) if is_search => {
                self.search.regex = !self.search.regex;
                true
            }
            (Key::Char('v'), true, false) | (Key::Insert, false, false) if m.ctrl || m.shift => {
                if let Some(t) = super::dialogs::get_clipboard(self.hwnd) {
                    p.insert(&t);
                }
                true
            }
            (Key::Char('a'), true, false) => {
                p.text.clear();
                p.cursor = 0;
                true
            }
            (Key::Tab | Key::Up | Key::Down | Key::PageUp | Key::PageDown, _, _) => false,
            _ => return false,
        };
        if changed {
            self.prompt_changed();
        }
        true
    }

    // ---- キーボード ----

    pub fn on_key_down(&mut self, vk: u16) -> KeyOutcome {
        let Some(key) = input::key_from_vk(vk) else { return KeyOutcome::Unhandled };
        let mods = input::current_modifiers();
        let chord = KeyChord::new(key, mods);
        // 詳細の編集中: Ctrl+Enter で反映して閉じる
        if self.detail_edit.is_some() && self.prompt.is_none() && key == Key::Enter && mods.ctrl && !mods.alt {
            self.finish_detail_edit(true);
            return KeyOutcome::Handled(None);
        }
        if self.prompt.is_some() && self.prompt_key(chord) {
            self.suppress_syschar = mods.alt;
            if self.prompt.is_none() {
                self.after_input();
            }
            self.refresh();
            return KeyOutcome::Handled(self.pending_deferred.take());
        }
        // 文字キーは WM_CHAR で処理する (IME やキーボード配列の違いを OS に任せる)
        if input::is_text_key(key) && !mods.ctrl && !mods.alt {
            return KeyOutcome::Unhandled;
        }
        let outcome = match self.mode.on_key(chord) {
            ModeResponse::Commands(cmds) => KeyOutcome::Handled(self.execute_commands(cmds)),
            ModeResponse::Consumed => {
                self.invalidate(); // 入力途中のキーをステータスバーに表示する
                KeyOutcome::Handled(None)
            }
            ModeResponse::Unhandled => KeyOutcome::Unhandled,
        };
        if matches!(outcome, KeyOutcome::Handled(_)) {
            self.suppress_syschar = mods.alt;
        }
        outcome
    }

    pub fn on_char(&mut self, code: u16) -> Option<Deferred> {
        let text = match code {
            0xD800..=0xDBFF => {
                self.high_surrogate = Some(code);
                return None;
            }
            0xDC00..=0xDFFF => String::from_utf16(&[self.high_surrogate.take()?, code]).ok()?,
            c if c < 0x20 || c == 0x7F => return None,
            c => String::from_utf16(&[c]).ok()?,
        };
        if let Some(p) = self.prompt.as_mut() {
            p.insert(&text);
            self.prompt_changed();
            self.refresh();
            return None;
        }
        match self.mode.on_text(&text) {
            ModeResponse::Commands(cmds) => self.execute_commands(cmds),
            _ => {
                self.invalidate();
                None
            }
        }
    }

    // ---- マウス ----

    fn to_dip(&self, x: i32, y: i32) -> (f32, f32) {
        let s = self.renderer.scale();
        (x as f32 / s, y as f32 / s)
    }

    fn line_at_y(&self, g: &Geometry, y: f32) -> usize {
        let rel = ((y - g.text_top) / self.renderer.line_height).floor();
        let line = self.tab().scroll_line as i64 + rel as i64;
        line.clamp(0, self.doc().buffer().len_lines() as i64 - 1) as usize
    }

    /// 画面座標 (DIP) → 文字位置。
    fn pos_at(&self, g: &Geometry, x: f32, y: f32) -> usize {
        let hit = self.hit_view(g, x, y);
        self.doc().buffer().line_to_char(hit.line) + hit.view.to_doc(hit.vcol)
    }

    /// 画面座標 (DIP) にある、表示用の行の文字位置。
    fn hit_view(&self, g: &Geometry, x: f32, y: f32) -> ViewHit {
        let line = self.line_at_y(g, y);
        let view = Self::view_line(self.doc(), line);
        let text = &view.text;
        let u: Vec<u16> = text.encode_utf16().collect();
        let tx = (x - g.text_left + self.tab().scroll_x).max(0.0);
        let vcol = match self.renderer.layout(&u, &self.renderer.text_format) {
            Some(l) => utf16_to_char(text, Renderer::pos_at(&l, tx)),
            None => 0,
        };
        ViewHit { line, vcol, view }
    }

    /// 画面座標 (DIP) にある URL (Ctrl+クリックで開く対象)。
    fn url_at(&self, g: &Geometry, x: f32, y: f32) -> Option<String> {
        if y < g.text_top || y >= g.text_bottom || x < g.text_left {
            return None;
        }
        let hit = self.hit_view(g, x, y);
        let text = &hit.view.text;
        let byte = text.char_indices().nth(hit.vcol).map(|(b, _)| b)?;
        // [label](url) の label、または URL そのもの
        if let Some((_, url)) = hit.view.links.iter().find(|(r, _)| r.start <= byte && byte < r.end) {
            return Some(link::normalize(url));
        }
        link::url_at(text, byte).map(|r| link::normalize(&text[r]))
    }

    fn select_lines(&mut self, a: usize, b: usize) {
        let buf = self.doc().buffer();
        let (first, last) = (a.min(b), a.max(b));
        let start = buf.line_to_char(first);
        let end = if last + 1 < buf.len_lines() { buf.line_to_char(last + 1) } else { buf.len_chars() };
        let sel = if b < a { Selection::new(end, start) } else { Selection::new(start, end) };
        self.doc_mut().set_selection(sel);
    }

    pub fn on_mouse_down(&mut self, x: i32, y: i32, double: bool) -> Option<Deferred> {
        let (x, y) = self.to_dip(x, y);
        self.status = None;
        // パネルの削除ボタン: キャレット行の詳細を削除する
        if self.panel_button.is_some_and(|(l, t, r, b)| x >= l && x < r && y >= t && y < b) {
            let doc = &self.main_tab().doc;
            let line = doc.buffer().char_to_line(doc.caret_pos());
            self.delete_detail(line);
            return None;
        }
        if let Some(sb) = self.panel_scrollbar.filter(|sb| sb.hit(x, y)) {
            let grab = if y >= sb.thumb_top && y < sb.thumb_top + sb.thumb_h { y - sb.thumb_top } else { sb.thumb_h / 2.0 };
            self.drag = Some(Drag::PanelScroll { grab });
            unsafe { SetCapture(self.hwnd) };
            self.scroll_panel_to_mouse(y, grab);
            return None;
        }
        if let Some(p) = self.detail_panel {
            // 見出しの帯: ドラッグでパネルを動かす
            if p.in_header(x, y) {
                self.drag = Some(Drag::Panel { ox: x - p.left, oy: y - p.top });
                unsafe { SetCapture(self.hwnd) };
                return None;
            }
            // 表示中のパネルのクリック: 編集を始める
            if p.contains(x, y) && self.detail_edit.is_none() {
                let doc = &self.main_tab().doc;
                let line = doc.buffer().char_to_line(doc.caret_pos());
                self.start_detail_edit(line);
                return None;
            }
            // 編集中にパネルの外をクリック: 反映して閉じ、そのクリックを本体で処理する
            if !p.contains(x, y) && self.detail_edit.is_some() {
                self.finish_detail_edit(true);
            }
        }
        let g = self.geometry();
        if y < g.tabbar_h {
            return self.tab_click(x, false);
        }
        if y < g.text_top || y >= g.text_bottom {
            return None;
        }
        // 行番号の欄の詳細のバッジ: 詳細を編集する
        if let Some(line) = self.badge_at(&g, x, y) {
            self.start_detail_edit(line);
            return None;
        }
        let mods = input::current_modifiers();
        if mods.ctrl && !mods.shift && !double {
            if let Some(url) = self.url_at(&g, x, y) {
                return Some(Deferred::OpenUrl(url));
            }
        }
        unsafe { SetCapture(self.hwnd) };
        let shift = mods.shift;
        // ビジュアルモードの選択はマウス操作で作り直す (離したときに mode.on_mouse へ伝える)
        vim::leave_visual(self.doc_mut());
        if x < g.gutter_w {
            let line = self.line_at_y(&g, y);
            let anchor_line = if shift { self.doc().buffer().char_to_line(self.doc().selection().anchor) } else { line };
            self.select_lines(anchor_line, line);
            self.drag = Some(Drag::Lines { anchor_line });
        } else {
            let pos = self.pos_at(&g, x, y);
            if double {
                let r = editor::word_at(self.doc().buffer(), pos);
                self.doc_mut().set_selection(Selection::new(r.start, r.end));
                self.drag = None;
                unsafe {
                    let _ = ReleaseCapture();
                }
                self.mouse_selection_done();
            } else {
                let sel = self.doc().selection().moved_to(pos, shift);
                self.doc_mut().set_selection(sel);
                self.drag = Some(Drag::Text);
            }
        }
        self.doc_mut().preferred_col = None;
        self.refresh();
        None
    }

    pub fn on_mouse_move(&mut self, x: i32, y: i32) {
        self.mouse = self.to_dip(x, y);
        match self.drag {
            None => return,
            Some(Drag::Tab { idx, start_x, moved }) => return self.drag_tab(idx, start_x, moved),
            Some(Drag::Panel { ox, oy }) => return self.move_panel_to(self.mouse.0 - ox, self.mouse.1 - oy),
            Some(Drag::PanelScroll { grab }) => return self.scroll_panel_to_mouse(self.mouse.1, grab),
            _ => {}
        }
        let g = self.geometry();
        let (_, my) = self.mouse;
        if my < g.text_top || my >= g.text_bottom {
            unsafe { SetTimer(Some(self.hwnd), AUTOSCROLL_TIMER, 30, None) };
        } else {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), AUTOSCROLL_TIMER);
            }
        }
        self.drag_to_mouse(&g);
    }

    fn drag_to_mouse(&mut self, g: &Geometry) {
        let (mx, my) = self.mouse;
        match self.drag {
            Some(Drag::Text) => {
                let pos = self.pos_at(g, mx, my);
                let sel = self.doc().selection().moved_to(pos, true);
                self.doc_mut().set_selection(sel);
            }
            Some(Drag::Lines { anchor_line }) => {
                let line = self.line_at_y(g, my);
                self.select_lines(anchor_line, line);
            }
            Some(Drag::Tab { .. } | Drag::Panel { .. } | Drag::PanelScroll { .. }) | None => return,
        }
        self.refresh();
    }

    /// タブのドラッグ中: マウスが隣のタブの中央を越えたら入れ替える。
    fn drag_tab(&mut self, mut idx: usize, start_x: f32, moved: bool) {
        let (mx, _) = self.mouse;
        if !moved && (mx - start_x).abs() < TAB_DRAG_THRESHOLD {
            return;
        }
        let mid = |h: &TabHit| (h.left + h.right) / 2.0;
        loop {
            let to = if idx > 0 && mx < mid(&self.tab_hits[idx - 1]) {
                idx - 1
            } else if idx + 1 < self.tab_hits.len() && mx > mid(&self.tab_hits[idx + 1]) {
                idx + 1
            } else {
                break;
            };
            self.move_tab(idx, to);
            idx = to;
        }
        self.drag = Some(Drag::Tab { idx, start_x, moved: true });
        self.refresh();
    }

    pub fn on_mouse_up(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        unsafe {
            let _ = ReleaseCapture();
            let _ = KillTimer(Some(self.hwnd), AUTOSCROLL_TIMER);
        }
        match drag {
            Drag::Tab { .. } | Drag::PanelScroll { .. } => {}
            Drag::Panel { .. } => self.save_panel_pos(),
            _ => self.mouse_selection_done(),
        }
    }

    /// マウスでの選択が終わったことを入力モードに伝える (Vim はビジュアルモードにする)。
    fn mouse_selection_done(&mut self) {
        let has_selection = !self.doc().selection().is_empty();
        match self.mode.on_mouse(has_selection) {
            ModeResponse::Commands(cmds) => {
                let _ = self.execute_commands(cmds);
            }
            _ => self.after_input(),
        }
    }

    /// ドラッグ中にマウスが編集領域の外に出たときの自動スクロール。
    pub fn on_timer(&mut self) {
        if self.drag.is_none() {
            return;
        }
        let g = self.geometry();
        let (_, my) = self.mouse;
        let lh = self.renderer.line_height;
        let dist = if my < g.text_top { my - g.text_top } else if my >= g.text_bottom { my - g.text_bottom } else { 0.0 };
        let lines = (dist / lh).round() as i64 + dist.signum() as i64;
        if lines != 0 {
            let tab = self.tab_mut();
            tab.scroll_line = (tab.scroll_line as i64 + lines).max(0) as usize;
            self.clamp_scroll();
        }
        self.drag_to_mouse(&g);
    }

    /// 右クリック: 行番号の欄の詳細のバッジなら、メニュー (編集・削除) を出す。
    /// メニューはモーダルなので `Deferred` で `App` の借用を解いてから出す。
    pub fn on_right_click(&mut self, x: i32, y: i32) -> Option<Deferred> {
        let (dx, dy) = self.to_dip(x, y);
        let g = self.main_geometry();
        self.badge_at(&g, dx, dy).map(|line| Deferred::DetailMenu { line, x, y })
    }

    pub fn on_middle_click(&mut self, x: i32, y: i32) -> Option<Deferred> {
        let (x, y) = self.to_dip(x, y);
        if y < self.geometry().tabbar_h {
            return self.tab_click(x, true);
        }
        None
    }

    fn tab_click(&mut self, x: f32, middle: bool) -> Option<Deferred> {
        let idx = self.tab_hits.iter().position(|h| x >= h.left && x < h.right)?;
        if middle || x >= self.tab_hits[idx].close_left {
            return Some(Deferred::CloseTab(idx));
        }
        self.activate(idx);
        if self.tab_hits.len() == self.tabs.len() {
            self.drag = Some(Drag::Tab { idx, start_x: x, moved: false });
            unsafe { SetCapture(self.hwnd) };
        }
        self.refresh();
        None
    }

    pub fn on_wheel(&mut self, delta: i32, horizontal: bool) {
        // 詳細の編集中でも、パネルの外では本体をスクロールする
        let (mx, my) = self.mouse;
        let in_panel = self.detail_panel.is_some_and(|p| p.contains(mx, my));
        if self.detail_edit.is_some() && !in_panel {
            return self.with_main(|a| a.on_wheel(delta, horizontal));
        }
        // 表示中のパネルの上: パネルの内容をスクロールする (120 = 3 行)
        if self.detail_edit.is_none() && in_panel && !horizontal {
            self.panel_scroll -= delta as f32 / 40.0 * self.renderer.line_height;
            self.invalidate();
            return;
        }
        let mods = input::current_modifiers();
        if mods.ctrl && !horizontal {
            let size = self.renderer.font_size + delta.signum() as f32;
            self.set_font_size(size);
            self.refresh();
            return;
        }
        // 120 = 1 ノッチ = 3 行。高精度タッチパッドの細かい値も積算する
        self.wheel_accum += delta;
        let steps = self.wheel_accum / 40;
        self.wheel_accum %= 40;
        if steps == 0 {
            return;
        }
        if horizontal || mods.shift {
            let dx = steps as f32 * self.renderer.char_width * if horizontal { 1.0 } else { -1.0 };
            self.scroll_x_by(dx);
        } else {
            self.scroll_by(-steps as i64);
        }
    }

    pub fn on_scroll(&mut self, vertical: bool, code: u16) {
        // スクロールバーは常に本体
        if self.detail_edit.is_some() {
            self.with_main(|a| a.on_scroll(vertical, code));
            return self.invalidate();
        }
        const LINEUP: u16 = 0;
        const LINEDOWN: u16 = 1;
        const PAGEUP: u16 = 2;
        const PAGEDOWN: u16 = 3;
        const THUMBTRACK: u16 = 5;
        const THUMBPOSITION: u16 = 4;
        const TOP: u16 = 6;
        const BOTTOM: u16 = 7;
        let bar = if vertical { SB_VERT } else { SB_HORZ };
        let track = || {
            let mut si = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_TRACKPOS, ..Default::default() };
            unsafe {
                let _ = GetScrollInfo(self.hwnd, bar, &mut si);
            }
            si.nTrackPos
        };
        if vertical {
            let page = self.visible_lines() as i64;
            match code {
                LINEUP => self.scroll_by(-1),
                LINEDOWN => self.scroll_by(1),
                PAGEUP => self.scroll_by(-page),
                PAGEDOWN => self.scroll_by(page),
                THUMBTRACK | THUMBPOSITION => {
                    self.tab_mut().scroll_line = track().max(0) as usize;
                    self.refresh();
                }
                TOP => self.scroll_by(i64::MIN / 2),
                BOTTOM => self.scroll_by(i64::MAX / 2),
                _ => {}
            }
        } else {
            let cw = self.renderer.char_width;
            let page = self.geometry().width / 2.0;
            match code {
                LINEUP => self.scroll_x_by(-cw * 4.0),
                LINEDOWN => self.scroll_x_by(cw * 4.0),
                PAGEUP => self.scroll_x_by(-page),
                PAGEDOWN => self.scroll_x_by(page),
                THUMBTRACK | THUMBPOSITION => {
                    self.tab_mut().scroll_x = track().max(0) as f32;
                    self.refresh();
                }
                TOP => self.scroll_x_by(f32::MIN / 2.0),
                BOTTOM => self.scroll_x_by(f32::MAX / 2.0),
                _ => {}
            }
        }
    }

    /// WM_SETCURSOR: 編集領域では I ビーム。
    pub fn update_cursor(&self) {
        let g = self.main_geometry();
        let (x, y) = self.mouse;
        let panel = self.detail_panel;
        let id = if self.panel_button.is_some_and(|(l, t, r, b)| x >= l && x < r && y >= t && y < b) {
            IDC_HAND
        } else if self.panel_scrollbar.is_some_and(|sb| sb.hit(x, y)) {
            IDC_ARROW
        } else if panel.is_some_and(|p| p.in_header(x, y)) {
            IDC_SIZEALL
        } else if panel.is_some_and(|p| p.contains(x, y)) {
            if self.detail_edit.is_some() {
                IDC_IBEAM
            } else {
                IDC_HAND
            }
        } else if self.badge_at(&g, x, y).is_some() {
            IDC_HAND
        } else if y < g.text_top || y >= g.text_bottom || x < g.gutter_w {
            IDC_ARROW
        } else if input::current_modifiers().ctrl
            && self.drag.is_none()
            && self.detail_edit.is_none()
            && self.url_at(&g, x, y).is_some()
        {
            IDC_HAND
        } else {
            IDC_IBEAM
        };
        unsafe {
            if let Ok(c) = LoadCursorW(None, id) {
                SetCursor(Some(c));
            }
        }
    }

    /// マウスがウィンドウ上にあれば、現在のマウス位置でカーソルの形を更新する。
    pub fn refresh_cursor(&mut self) {
        let mut pt = POINT::default();
        unsafe {
            if GetCursorPos(&mut pt).is_err() || WindowFromPoint(pt) != self.hwnd || !ScreenToClient(self.hwnd, &mut pt).as_bool() {
                return;
            }
        }
        self.mouse = self.to_dip(pt.x, pt.y);
        self.update_cursor();
    }

    pub fn update_menu(&self) {
        self.menus.update(self.doc(), self.theme.name == "dark", self.vim_enabled, self.txt_detail_view);
    }

    // ---- IME ----

    /// WM_IME_COMPOSITION。本文への入力では変換中の文字列を自前で描画し、
    /// 確定した文字列を挿入して `Some` を返す。入力欄への入力では IME の
    /// 既定の変換ウィンドウに任せるため `None` を返す (呼び出し元は既定処理に渡す)。
    pub fn on_ime_composition(&mut self, flags: u32) -> Option<Option<Deferred>> {
        if self.prompt.is_some() {
            return None;
        }
        let has = |f: IME_COMPOSITION_STRING| flags & f.0 != 0;
        let mut deferred = None;
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if himc.is_invalid() {
                return None;
            }
            if has(GCS_RESULTSTR) {
                let result = ime_string(himc, GCS_RESULTSTR);
                if !result.is_empty() {
                    self.ime = None;
                    if let ModeResponse::Commands(cmds) = self.mode.on_text(&result) {
                        deferred = self.execute_commands(cmds);
                    }
                }
            }
            if has(GCS_COMPSTR) {
                let text = ime_string(himc, GCS_COMPSTR);
                self.ime = if text.is_empty() {
                    None
                } else {
                    let len16 = text.encode_utf16().count();
                    let cursor = if has(GCS_CURSORPOS) {
                        (ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0).max(0) as usize).min(len16)
                    } else {
                        len16
                    };
                    let target = if has(GCS_COMPATTR) { ime_target(himc) } else { None };
                    Some(ImeComposition { text, cursor, target })
                };
            }
            let _ = ImmReleaseContext(self.hwnd, himc);
        }
        self.position_ime();
        self.invalidate();
        Some(deferred)
    }

    /// WM_IME_ENDCOMPOSITION。変換中の表示を消す。
    pub fn end_ime_composition(&mut self) {
        if self.ime.take().is_some() {
            self.invalidate();
        }
    }

    /// 変換ウィンドウ・候補ウィンドウの位置と書体をキャレットに合わせる。
    pub fn position_ime(&self) {
        let g = self.geometry();
        let r = &self.renderer;
        let (x, y) = if let Some(p) = self.prompt.as_ref().filter(|p| p.in_status_bar()) {
            let x = status_prompt_text_left(self) + r.text_width(&p.text[..p.cursor], &r.ui_format);
            (x, g.status_top + (g.height - g.status_top - r.ui_line_height) / 2.0)
        } else if let Some(p) = &self.prompt {
            let x = prompt_input_left(self, &g) + 4.0 + r.text_width(&p.text[..p.cursor], &r.text_format);
            (x, g.prompt_top + (g.status_top - g.prompt_top - r.line_height) / 2.0)
        } else {
            let doc = self.doc();
            let (line, col) = doc.buffer().line_col(doc.caret_pos());
            let x = g.text_left - self.tab().scroll_x + self.col_x(line, col);
            let y = g.text_top + (line as f32 - self.tab().scroll_line as f32) * r.line_height;
            (x, y)
        };
        let s = r.scale();
        let pt = POINT { x: (x * s) as i32, y: (y * s) as i32 };
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if himc.is_invalid() {
                return;
            }
            let cf = COMPOSITIONFORM { dwStyle: CFS_POINT, ptCurrentPos: pt, rcArea: RECT::default() };
            let _ = ImmSetCompositionWindow(himc, &cf);
            // 候補ウィンドウが入力中の行を隠さないよう、行の矩形を除外領域にする
            let cand = CANDIDATEFORM {
                dwIndex: 0,
                dwStyle: CFS_EXCLUDE,
                ptCurrentPos: pt,
                rcArea: RECT { left: pt.x, top: pt.y, right: pt.x + 1, bottom: ((y + r.line_height) * s) as i32 },
            };
            let _ = ImmSetCandidateWindow(himc, &cand);
            let mut lf = LOGFONTW {
                lfHeight: -((r.font_size * s) as i32),
                lfCharSet: SHIFTJIS_CHARSET,
                ..Default::default()
            };
            for (dst, src) in lf.lfFaceName.iter_mut().zip("MS Gothic".encode_utf16()) {
                *dst = src;
            }
            let _ = ImmSetCompositionFontW(himc, &lf);
            let _ = ImmReleaseContext(self.hwnd, himc);
        }
    }
}

/// 変換中・確定した文字列を取り出す。
unsafe fn ime_string(himc: HIMC, kind: IME_COMPOSITION_STRING) -> String {
    let bytes = unsafe { ImmGetCompositionStringW(himc, kind, None, 0) };
    if bytes <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; bytes as usize / 2];
    unsafe { ImmGetCompositionStringW(himc, kind, Some(buf.as_mut_ptr().cast()), bytes as u32) };
    String::from_utf16_lossy(&buf)
}

/// 変換中の文字列のうち、変換対象の文節 (UTF-16 の範囲)。
unsafe fn ime_target(himc: HIMC) -> Option<std::ops::Range<usize>> {
    let len = unsafe { ImmGetCompositionStringW(himc, GCS_COMPATTR, None, 0) };
    if len <= 0 {
        return None;
    }
    let mut attrs = vec![0u8; len as usize];
    unsafe { ImmGetCompositionStringW(himc, GCS_COMPATTR, Some(attrs.as_mut_ptr().cast()), len as u32) };
    let is_target = |a: &u8| matches!(*a as u32, ATTR_TARGET_CONVERTED | ATTR_TARGET_NOTCONVERTED);
    let start = attrs.iter().position(is_target)?;
    let end = attrs[start..].iter().position(|a| !is_target(a)).map_or(attrs.len(), |n| start + n);
    Some(start..end)
}

/// ステータスバーに表示する入力欄 (Vim の `:` など) の、入力文字列の左端。
pub fn status_prompt_text_left(app: &App) -> f32 {
    let r = &app.renderer;
    let label = app.prompt.as_ref().map_or("", |p| p.label());
    12.0 + r.text_width(label, &r.ui_format)
}

/// 入力欄のテキスト領域の左端 (paint.rs と IME 位置計算で共有)。
pub fn prompt_input_left(app: &App, _g: &Geometry) -> f32 {
    let r = &app.renderer;
    let label = app.prompt.as_ref().map_or("", |p| p.label());
    12.0 + r.text_width(label, &r.ui_format) + 10.0
}

/// タブを `from` から `to` へ移したとき、位置 `i` にあったタブの新しい位置。
fn moved_index(i: usize, from: usize, to: usize) -> usize {
    if i == from {
        to
    } else if from < i && i <= to {
        i - 1
    } else if to <= i && i < from {
        i + 1
    } else {
        i
    }
}

fn set_wait_cursor() {
    unsafe {
        if let Ok(c) = LoadCursorW(None, IDC_WAIT) {
            SetCursor(Some(c));
        }
    }
}
