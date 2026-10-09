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
use lvt_core::keymap::{InputMode, Key, KeyChord, ModeResponse};
use lvt_core::search::{self, SearchQuery};
use lvt_core::theme::Theme;
use lvt_core::{AppCommand, Command, Document, EditCommand, Selection};
use regex::Regex;
use windows::core::Result;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{InvalidateRect, LOGFONTW};
use windows::Win32::UI::Input::Ime::{
    ImmGetContext, ImmReleaseContext, ImmSetCompositionFontW, ImmSetCompositionWindow, CFS_POINT, COMPOSITIONFORM,
};
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetScrollInfo, KillTimer, LoadCursorW, SetCursor, SetTimer, SetWindowTextW, IDC_ARROW,
    IDC_IBEAM, IDC_WAIT, SB_HORZ, SB_VERT, SCROLLBAR_CONSTANTS, SCROLLINFO, SIF_ALL, SIF_DISABLENOSCROLL, SIF_TRACKPOS,
};

use super::input;
use super::menu::Menus;
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
    fn new(doc: Document) -> Self {
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

pub struct TabHit {
    pub left: f32,
    pub right: f32,
    pub close_left: f32,
}

#[derive(Clone, Copy)]
enum Drag {
    Text,
    Lines { anchor_line: usize },
}

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
    pub suppress_syschar: bool,
    title: String,
}

impl App {
    pub fn new(hwnd: HWND, mode: Box<dyn InputMode>, menus: Menus) -> Result<Self> {
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
            suppress_syschar: false,
            title: String::new(),
        })
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    fn doc(&self) -> &Document {
        &self.tab().doc
    }

    fn doc_mut(&mut self) -> &mut Document {
        &mut self.tab_mut().doc
    }

    // ---- 画面配置 ----

    pub fn geometry(&self) -> Geometry {
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
        let prompt_h = if self.prompt.is_some() { r.snap(r.line_height.max(r.ui_line_height) + 12.0) } else { 0.0 };
        let status_top = (height - status_h).max(tabbar_h);
        let prompt_top = (status_top - prompt_h).max(tabbar_h);
        let digits = self.doc().buffer().len_lines().to_string().len().max(3);
        let gutter_w = r.snap((digits as f32 + 2.0) * r.char_width);
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

    /// 行内の文字位置の x 座標 (行頭からの DIP)。
    fn col_x(&self, line: usize, col: usize) -> f32 {
        let text = Self::line_text(self.doc(), line);
        let u: Vec<u16> = text.encode_utf16().collect();
        match self.renderer.layout(&u, &self.renderer.text_format) {
            Some(l) => Renderer::x_of(&l, char_to_utf16(&text, col)),
            None => col as f32 * self.renderer.char_width,
        }
    }

    // ---- 再描画・スクロール ----

    /// 状態変更後の共通処理: スクロールバー・タイトル更新と再描画。
    pub fn refresh(&mut self) {
        self.clamp_scroll();
        self.update_scrollbars();
        self.update_title();
        self.invalidate();
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
        let tab = self.tab_mut();
        let max = tab.doc.buffer().len_lines().saturating_sub(vis);
        tab.scroll_line = tab.scroll_line.min(max);
        tab.scroll_x = tab.scroll_x.clamp(0.0, tab.max_width.max(0.0));
    }

    fn update_scrollbars(&self) {
        let g = self.geometry();
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
        let text_w = (g.width - g.text_left).max(1.0);
        let content_w = tab.max_width + self.renderer.char_width * 4.0;
        set(SB_HORZ, content_w as i32, text_w as u32, tab.scroll_x as i32);
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
            doc.buffer().line_col(doc.selection().head)
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
                let view = ViewInfo { page_lines: self.visible_lines().saturating_sub(1).max(1) };
                let cfg = self.editor_cfg.clone();
                editor::execute(self.doc_mut(), &e, &cfg, &view);
                self.ensure_caret_visible();
                None
            }
            Command::App(a) => self.execute_app(a),
        };
        self.refresh();
        deferred
    }

    fn execute_commands(&mut self, cmds: Vec<Command>) -> Option<Deferred> {
        for c in cmds {
            if let Some(d) = self.execute(c) {
                return Some(d);
            }
        }
        None
    }

    fn execute_app(&mut self, cmd: AppCommand) -> Option<Deferred> {
        use AppCommand as A;
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
            A::Cancel => {
                if self.prompt.take().is_none() {
                    self.highlight_matches = false;
                }
            }
        }
        None
    }

    pub fn activate(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.active = idx;
            self.drag = None;
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
        self.refresh();
        errors
    }

    pub fn save_tab(&mut self, idx: usize, path: &Path) -> std::result::Result<(), SaveError> {
        set_wait_cursor();
        let tab = self.tabs.get_mut(idx).ok_or_else(|| SaveError::Io(std::io::Error::other("tab not found")))?;
        tab.doc.save_as(path)?;
        self.status = Some(format!("保存しました: {}", path.display()));
        self.refresh();
        Ok(())
    }

    pub fn remove_tab(&mut self, idx: usize) {
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
        self.status = Some(format!("{} で開き直しました", enc.name()));
        self.refresh();
        Ok(())
    }

    // ---- 検索・プロンプト ----

    fn open_prompt(&mut self, kind: PromptKind) {
        let doc = self.doc();
        let sel = doc.selection();
        let initial = match kind {
            PromptKind::GoToLine => String::new(),
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
        if p.kind != PromptKind::Find || self.doc().buffer().len_bytes() > INCREMENTAL_SEARCH_LIMIT {
            return;
        }
        let origin = p.origin;
        match self.search_re.clone() {
            Some(re) => {
                self.highlight_matches = true;
                match search::find_next(self.doc().buffer(), &re, origin, true) {
                    Some(r) => self.select_match(r),
                    None => self.status = Some("見つかりません".into()),
                }
            }
            None => self.doc_mut().set_selection(Selection::caret(origin)),
        }
    }

    fn select_match(&mut self, r: std::ops::Range<usize>) {
        let line = self.doc().buffer().char_to_line(r.start);
        self.doc_mut().set_selection(Selection::new(r.start, r.end));
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
        let (from, result) = if forward {
            let from = if sel.is_empty() { sel.head } else { sel.end() };
            (from, search::find_next(buf, &re, from, true))
        } else {
            (sel.start(), search::find_prev(buf, &re, sel.start(), true))
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
        if self.prompt.is_some() && self.prompt_key(chord) {
            self.suppress_syschar = mods.alt;
            self.refresh();
            return KeyOutcome::Handled(None);
        }
        // 文字キーは WM_CHAR で処理する (IME やキーボード配列の違いを OS に任せる)
        if input::is_text_key(key) && !mods.ctrl && !mods.alt {
            return KeyOutcome::Unhandled;
        }
        let outcome = match self.mode.on_key(chord) {
            ModeResponse::Commands(cmds) => KeyOutcome::Handled(self.execute_commands(cmds)),
            ModeResponse::Consumed => KeyOutcome::Handled(None),
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
            _ => None,
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
        let line = self.line_at_y(g, y);
        let doc = self.doc();
        let text = Self::line_text(doc, line);
        let u: Vec<u16> = text.encode_utf16().collect();
        let tx = (x - g.text_left + self.tab().scroll_x).max(0.0);
        let col = match self.renderer.layout(&u, &self.renderer.text_format) {
            Some(l) => utf16_to_char(&text, Renderer::pos_at(&l, tx)),
            None => 0,
        };
        doc.buffer().line_to_char(line) + col
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
        let g = self.geometry();
        self.status = None;
        if y < g.tabbar_h {
            return self.tab_click(x, false);
        }
        if y < g.text_top || y >= g.text_bottom {
            return None;
        }
        unsafe { SetCapture(self.hwnd) };
        let shift = input::current_modifiers().shift;
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
        if self.drag.is_none() {
            return;
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
            None => return,
        }
        self.refresh();
    }

    pub fn on_mouse_up(&mut self) {
        if self.drag.take().is_some() {
            unsafe {
                let _ = ReleaseCapture();
                let _ = KillTimer(Some(self.hwnd), AUTOSCROLL_TIMER);
            }
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
        self.refresh();
        None
    }

    pub fn on_wheel(&mut self, delta: i32, horizontal: bool) {
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
        let g = self.geometry();
        let (x, y) = self.mouse;
        let id = if y >= g.text_top && y < g.text_bottom && x >= g.gutter_w { IDC_IBEAM } else { IDC_ARROW };
        unsafe {
            if let Ok(c) = LoadCursorW(None, id) {
                SetCursor(Some(c));
            }
        }
    }

    pub fn update_menu(&self) {
        self.menus.update(self.doc(), self.theme.name == "dark");
    }

    // ---- IME ----

    /// 変換中の文字列ウィンドウをキャレット位置に合わせる。
    pub fn position_ime(&self) {
        let g = self.geometry();
        let r = &self.renderer;
        let (x, y) = if let Some(p) = &self.prompt {
            let x = prompt_input_left(self, &g) + 4.0 + r.text_width(&p.text[..p.cursor], &r.text_format);
            (x, g.prompt_top + (g.status_top - g.prompt_top - r.line_height) / 2.0)
        } else {
            let doc = self.doc();
            let (line, col) = doc.buffer().line_col(doc.selection().head);
            let x = g.text_left - self.tab().scroll_x + self.col_x(line, col);
            let y = g.text_top + (line as f32 - self.tab().scroll_line as f32) * r.line_height;
            (x, y)
        };
        let s = r.scale();
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if himc.is_invalid() {
                return;
            }
            let cf = COMPOSITIONFORM {
                dwStyle: CFS_POINT,
                ptCurrentPos: POINT { x: (x * s) as i32, y: (y * s) as i32 },
                rcArea: RECT::default(),
            };
            let _ = ImmSetCompositionWindow(himc, &cf);
            let mut lf = LOGFONTW { lfHeight: -((r.font_size * s) as i32), ..Default::default() };
            for (dst, src) in lf.lfFaceName.iter_mut().zip("Consolas".encode_utf16()) {
                *dst = src;
            }
            let _ = ImmSetCompositionFontW(himc, &lf);
            let _ = ImmReleaseContext(self.hwnd, himc);
        }
    }
}

/// 入力欄のテキスト領域の左端 (paint.rs と IME 位置計算で共有)。
pub fn prompt_input_left(app: &App, _g: &Geometry) -> f32 {
    let r = &app.renderer;
    let label = app.prompt.as_ref().map_or("", |p| p.label());
    12.0 + r.text_width(label, &r.ui_format) + 10.0
}

fn set_wait_cursor() {
    unsafe {
        if let Ok(c) = LoadCursorW(None, IDC_WAIT) {
            SetCursor(Some(c));
        }
    }
}
