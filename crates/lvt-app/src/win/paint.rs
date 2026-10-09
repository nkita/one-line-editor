//! 画面の描画 (WM_PAINT)。見えている行だけを 1 行ずつレイアウトして描く。

use lvt_core::keymap::CaretStyle;
use lvt_core::detail;
use lvt_core::lineview::LineView;
use lvt_core::link;
use lvt_core::syntax::{StyleSpan, TokenKind};
use lvt_core::theme::Theme;
use regex::Regex;
use windows::Win32::Graphics::DirectWrite::{
    IDWriteTextLayout, DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_WEIGHT_BOLD, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE, DWRITE_WORD_WRAPPING_WRAP,
};

use super::app::{prompt_input_left, status_prompt_text_left, App, Geometry, ImeComposition, Tab, TabHit};
use super::panel::{PanelRect, PanelScrollbar, PANEL_RADIUS};
use super::render::Renderer;
use super::util::{byte_to_utf16, char_to_utf16};

struct TextCtx<'a> {
    focused: bool,
    caret: CaretStyle,
    matches: Option<&'a Regex>,
    ime: Option<&'a ImeComposition>,
    /// 行番号の欄を描くか (詳細パネルの中では描かない)。
    gutter: bool,
    /// 行番号の欄の詳細のバッジの左右 (`App::badge_x`)。None ならバッジの欄はない。
    badge_x: Option<(f32, f32)>,
}

impl App {
    pub fn paint(&mut self) {
        if !self.renderer.begin(self.hwnd) {
            return;
        }
        let g = self.main_geometry();
        let theme = self.theme.clone();
        self.renderer.clear(theme.background);

        let editing = self.detail_edit.is_some();
        self.panel_scrollbar = None;
        let matches = if self.highlight_matches { self.search_re.as_ref() } else { None };
        // 詳細の編集中は、本体にキャレット・IME・検索の強調を描かない
        let ctx = TextCtx {
            focused: self.focused && !editing,
            caret: self.mode.caret_style(),
            matches: matches.filter(|_| !editing),
            ime: self.ime.as_ref().filter(|_| !editing),
            gutter: true,
            badge_x: self.badge_x(&g),
        };
        paint_text(&mut self.renderer, &mut self.tabs[self.active], &g, &theme, &ctx);
        if editing {
            self.paint_edit_panel(&g, &theme);
        } else {
            self.paint_detail_panel(&g, &theme);
        }

        self.paint_tabbar(&g, &theme);
        if self.prompt.as_ref().is_some_and(|p| !p.in_status_bar()) {
            self.paint_prompt(&g, &theme);
        }
        self.paint_status(&g, &theme);
        self.renderer.end();
    }

    /// パネルの枠 (角丸・見出しの帯) を描く。
    fn paint_panel_frame(&mut self, p: &PanelRect, theme: &Theme, title: &str, hint: &str, active: bool) {
        let r = &mut self.renderer;
        let rad = PANEL_RADIUS;
        r.fill_round(p.left, p.top, p.right, p.bottom, rad, theme.chrome_bg, 1.0);
        // 本文の部分 (見出しの帯の下) を背景色で塗る
        r.push_clip(p.left, p.header_bottom, p.right, p.bottom);
        r.fill_round(p.left, p.top, p.right, p.bottom, rad, theme.background, 1.0);
        r.pop_clip();
        r.fill(p.left + 1.0, p.header_bottom, p.right - 1.0, p.header_bottom + 1.0, theme.tab_inactive_bg);
        let border = if active { theme.accent } else { theme.tab_inactive_bg };
        r.stroke_round(p.left + 0.5, p.top + 0.5, p.right - 0.5, p.bottom - 0.5, rad, border, if active { 1.5 } else { 1.0 });

        let hh = p.header_bottom - p.top;
        r.push_clip(p.left + 12.0, p.top, p.right - 12.0, p.header_bottom);
        let hint_w = r.text_width(hint, &r.ui_format);
        let tw = r.draw_ui_text(title, p.left + 12.0, p.top, hh, theme.chrome_fg);
        if p.left + 12.0 + tw + 16.0 + hint_w < p.right - 12.0 {
            r.draw_ui_text(hint, p.right - 12.0 - hint_w, p.top, hh, theme.gutter_fg);
        }
        r.pop_clip();
    }

    /// キャレット行の詳細 (`*detail[…]`) の内容を表示する。位置は `panel_rect` (既定は右上で、キャレット行を避ける)。
    fn paint_detail_panel(&mut self, g: &Geometry, theme: &Theme) {
        self.detail_panel = None;
        let doc = &self.tabs[self.active].doc;
        let caret_line = doc.buffer().char_to_line(doc.caret_pos());
        let Some(token) = doc.detail_at_line(caret_line) else { return };
        let ls = doc.buffer().line_to_char(caret_line);
        let c = token.content();
        let content = detail::unescape(&doc.buffer().slice_to_string(ls + c.start..ls + c.end));
        let scroll_line = self.tabs[self.active].scroll_line;
        if self.panel_scroll_line != Some(caret_line) {
            self.panel_scroll = 0.0;
            self.panel_scroll_line = Some(caret_line);
        }

        let text_w = g.width - g.gutter_w;
        let text_h = g.text_bottom - g.text_top;
        if text_w < 280.0 || text_h < 140.0 {
            return;
        }
        let pad = 14.0;
        let w = (text_w * 0.4).clamp(260.0, 540.0);
        let header_h = self.panel_header_h();
        let empty = content.is_empty();
        // 本文と同じ装飾 (リンク・コード・強調) を行ごとに適用する
        let (shown, spans) = if empty {
            ("(空です。クリックまたは F2 で編集)".to_string(), Vec::new())
        } else {
            decorate_lines(&content)
        };
        let format = self.renderer.text_format.clone();
        let Some(layout) = self.renderer.layout_str(&shown, &format) else { return };
        apply_styles(&mut self.renderer, &layout, &shown, &spans, theme);
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            let _ = layout.SetMaxWidth(w - pad * 2.0 - 8.0);
            let _ = layout.GetMetrics(&mut m);
        }
        let r = &self.renderer;
        let max_body = (text_h * 0.5 - header_h - pad * 2.0).max(r.line_height);
        let body_h = m.height.min(max_body);
        let h = header_h + pad + body_h + pad;
        let caret_y = g.text_top + (caret_line as f32 - scroll_line as f32) * r.line_height;
        let p = self.panel_rect(g, w, h, Some((caret_y, caret_y + r.line_height)));

        let title = format!("詳細 · {} 行目", caret_line + 1);
        self.paint_panel_frame(&p, theme, &title, "クリック / F2 で編集", false);
        let max_scroll = (m.height - body_h).max(0.0);
        self.panel_scroll = self.panel_scroll.clamp(0.0, max_scroll);
        let body_top = p.header_bottom + pad;
        let r = &mut self.renderer;
        r.push_clip(p.left + pad, body_top, p.right - pad, body_top + body_h);
        r.draw_layout(&layout, p.left + pad, body_top - self.panel_scroll, if empty { theme.gutter_fg } else { theme.foreground });
        r.pop_clip();
        if max_scroll > 0.0 {
            let sb = PanelScrollbar::new(p.right - 9.0, body_top, body_top + body_h, body_h / m.height, self.panel_scroll, max_scroll);
            self.paint_panel_scrollbar(&sb, theme);
        }
        self.detail_panel = Some(p);
    }

    /// パネル内で編集中の詳細を描く (本体と同じ `paint_text` を使う)。
    fn paint_edit_panel(&mut self, g: &Geometry, theme: &Theme) {
        let p = self.edit_panel_rect(g);
        let line = self.detail_edit.as_ref().map_or(0, |e| e.line);
        let hint = if self.vim_enabled { "Esc: 反映して閉じる   :q!: 破棄" } else { "Esc / Ctrl+Enter: 反映して閉じる" };
        let title = format!("詳細を編集 · {} 行目", line + 1);
        self.paint_panel_frame(&p, theme, &title, hint, true);
        let eg = self.edit_geometry(*g);
        let ctx = TextCtx {
            focused: self.focused,
            caret: self.mode.caret_style(),
            matches: if self.highlight_matches { self.search_re.as_ref() } else { None },
            ime: self.ime.as_ref(),
            gutter: false,
            badge_x: None,
        };
        let vis = self.visible_lines();
        if let Some(e) = &mut self.detail_edit {
            paint_text(&mut self.renderer, &mut e.tab, &eg, theme, &ctx);
            let lines = e.tab.doc.buffer().len_lines();
            if lines > vis {
                let max = (lines - vis) as f32;
                let ratio = vis as f32 / lines as f32;
                let sb = PanelScrollbar::new(p.right - 9.0, eg.text_top, eg.text_bottom, ratio, e.tab.scroll_line as f32, max);
                self.paint_panel_scrollbar(&sb, theme);
            }
        }
        self.detail_panel = Some(p);
    }

    /// パネルの細いスクロールバー (角丸のトラックとつまみ)。
    fn paint_panel_scrollbar(&mut self, sb: &PanelScrollbar, theme: &Theme) {
        let r = &mut self.renderer;
        r.fill_round(sb.left, sb.top, sb.right, sb.bottom, 2.0, theme.gutter_fg, 0.15);
        r.fill_round(sb.left, sb.thumb_top, sb.right, sb.thumb_top + sb.thumb_h, 2.0, theme.gutter_fg, 0.6);
        self.panel_scrollbar = Some(*sb);
    }

    fn paint_tabbar(&mut self, g: &Geometry, theme: &Theme) {
        let r = &mut self.renderer;
        r.fill(0.0, 0.0, g.width, g.tabbar_h, theme.chrome_bg);
        let pad = 12.0;
        let close_w = 20.0;
        let top = 4.0;
        let titles: Vec<String> = self
            .tabs
            .iter()
            .map(|t| format!("{}{}", t.doc.title(), if t.doc.is_modified() { " ●" } else { "" }))
            .collect();
        let natural: Vec<f32> =
            titles.iter().map(|t| (r.text_width(t, &r.ui_format) + pad * 2.0 + close_w).min(240.0)).collect();
        let total: f32 = natural.iter().sum();
        let avail = g.width - 8.0;
        let shrink = if total > avail { avail / total } else { 1.0 };

        self.tab_hits.clear();
        let mut x = 4.0;
        for (i, title) in titles.iter().enumerate() {
            let w = (natural[i] * shrink).max(48.0);
            let active = i == self.active;
            r.fill(x, top, x + w - 2.0, g.tabbar_h, if active { theme.tab_active_bg } else { theme.tab_inactive_bg });
            if active {
                r.fill(x, top, x + w - 2.0, top + 2.0, theme.accent);
            }
            let close_left = x + w - 2.0 - close_w;
            r.push_clip(x + pad, top, close_left, g.tabbar_h);
            r.draw_ui_text(title, x + pad, top, g.tabbar_h - top, theme.chrome_fg);
            r.pop_clip();
            r.draw_ui_text("×", close_left + 4.0, top, g.tabbar_h - top, theme.gutter_fg);
            self.tab_hits.push(TabHit { left: x, right: x + w, close_left });
            x += w;
        }
    }

    fn paint_prompt(&mut self, g: &Geometry, theme: &Theme) {
        let Some(p) = &self.prompt else { return };
        let input_left = prompt_input_left(self, g);
        let r = &mut self.renderer;
        let (top, bottom) = (g.prompt_top, g.status_top);
        r.fill(0.0, top, g.width, bottom, theme.chrome_bg);
        r.fill(0.0, top, g.width, top + 1.0, theme.accent);
        r.draw_ui_text(p.label(), 12.0, top, bottom - top, theme.chrome_fg);

        let opts_w = if p.is_search() { 300.0 } else { 0.0 };
        let input_right = (g.width - opts_w - 12.0).max(input_left + 40.0);
        r.fill(input_left, top + 4.0, input_right, bottom - 4.0, theme.background);
        r.push_clip(input_left, top, input_right, bottom);
        let text_y = top + ((bottom - top) - r.line_height) / 2.0;
        if let Some(layout) = r.layout_str(&p.text, &r.text_format.clone()) {
            r.draw_layout(&layout, input_left + 4.0, text_y, theme.foreground);
            let cx = input_left + 4.0 + Renderer::x_of(&layout, byte_to_utf16(&p.text, p.cursor));
            r.fill(cx, text_y, cx + 1.5, text_y + r.line_height, theme.caret);
        }
        r.pop_clip();

        if p.is_search() {
            let mut x = input_right + 12.0;
            for (on, label) in [(self.search.case_sensitive, "Aa 大小区別 (Alt+C)"), (self.search.regex, ".* 正規表現 (Alt+R)")] {
                let color = if on { theme.accent } else { theme.gutter_fg };
                x += r.draw_ui_text(label, x, top, bottom - top, color) + 14.0;
            }
        }
    }

    fn paint_status(&mut self, g: &Geometry, theme: &Theme) {
        // 行・桁と選択は操作対象 (詳細の編集中はパネル)、文字コードなどは本体
        let doc = &self.tabs[self.active].doc;
        let cur = &self.tab().doc;
        let sel = cur.selection();
        let (line, col) = cur.buffer().line_col(cur.caret_pos());
        let left = self.status.clone().unwrap_or_else(|| {
            let mut s = format!("{} 行, {} 桁", line + 1, col + 1);
            if !sel.is_empty() {
                s.push_str(&format!("  ({} 文字選択)", sel.end() - sel.start()));
            }
            s
        });
        let mode = self.mode.name();
        let mut right: Vec<&str> = Vec::new();
        if !mode.is_empty() {
            right.push(&mode);
        }
        right.extend([doc.highlighter_name(), doc.encoding.name(), doc.line_ending.name()]);
        let right = right.join("    ");

        // Vim の `:` `/` `?` は、行・桁の代わりにステータスバーで入力する
        let status_prompt = self.prompt.as_ref().filter(|p| p.in_status_bar()).map(|p| (p.label(), p.text.clone(), p.cursor));
        let text_left = status_prompt_text_left(self);
        let r = &mut self.renderer;
        let (top, bottom) = (g.status_top, g.height);
        r.fill(0.0, top, g.width, bottom, theme.chrome_bg);
        r.fill(0.0, top, g.width, top + 1.0, theme.tab_inactive_bg);
        let rw = r.text_width(&right, &r.ui_format);
        r.push_clip(0.0, top, (g.width - rw - 24.0).max(0.0), bottom);
        match status_prompt {
            Some((label, text, cursor)) => {
                r.draw_ui_text(label, 12.0, top, bottom - top, theme.foreground);
                if let Some(layout) = r.layout_str(&text, &r.ui_format.clone()) {
                    unsafe {
                        let _ = layout.SetMaxHeight(bottom - top);
                        let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                    }
                    r.draw_layout(&layout, text_left, top, theme.foreground);
                    let cx = text_left + Renderer::x_of(&layout, byte_to_utf16(&text, cursor));
                    let cy = top + (bottom - top - r.ui_line_height) / 2.0;
                    r.fill(cx, cy, cx + 1.5, cy + r.ui_line_height, theme.caret);
                }
            }
            None => {
                r.draw_ui_text(&left, 12.0, top, bottom - top, theme.chrome_fg);
            }
        }
        r.pop_clip();
        r.draw_ui_text(&right, g.width - rw - 12.0, top, bottom - top, theme.chrome_fg);
    }
}

fn paint_text(r: &mut Renderer, tab: &mut Tab, g: &Geometry, theme: &Theme, ctx: &TextCtx) {
    let badge_x = ctx.badge_x;
    let lh = r.line_height;
    let cw = r.char_width;
    let sel = tab.doc.selection();
    let (caret_line, caret_col) = tab.doc.buffer().line_col(tab.doc.caret_pos());
    let len_lines = tab.doc.buffer().len_lines();
    let rows = ((g.text_bottom - g.text_top) / lh).ceil() as usize + 1;
    let origin_x = g.text_left - tab.scroll_x;
    let format = r.text_format.clone();
    let mut spans: Vec<StyleSpan> = Vec::new();

    if ctx.gutter {
        r.fill(0.0, g.text_top, g.gutter_w, g.text_bottom, theme.gutter_bg);
    }
    r.push_clip(g.gutter_w, g.text_top, g.width, g.text_bottom);
    for i in 0..rows {
        let line = tab.scroll_line + i;
        if line >= len_lines {
            break;
        }
        let y = g.text_top + i as f32 * lh;
        let line_start = tab.doc.buffer().line_to_char(line);
        let line_len = tab.doc.buffer().line_len(line);
        // 表示用の行 (装飾表示では詳細をアイコンにし、記法の記号を隠す)。
        // 文書上の桁 → 表示上の桁は `vcol` で変換する。
        let view = App::view_line(&tab.doc, line);
        let text = view.text.clone();
        let vcol = |col: usize| view.to_view(col, false);
        let caret_vcol = vcol(caret_col);
        // IME で変換中の文字列はキャレット位置に埋め込んで描く。
        // 表示用の行の位置 (UTF-16) → 変換中の文字列を含めた位置は `to_view` で変換する。
        let ime = ctx.ime.filter(|_| line == caret_line);
        let ime_at = char_to_utf16(&text, caret_vcol);
        let ime_len = ime.map_or(0, |c| c.text.encode_utf16().count());
        let to_view = |p16: usize| if p16 >= ime_at { p16 + ime_len } else { p16 };
        let view_text = match ime {
            Some(c) => {
                let at = text.char_indices().nth(caret_vcol).map_or(text.len(), |(b, _)| b);
                format!("{}{}{}", &text[..at], c.text, &text[at..])
            }
            None => text.clone(),
        };
        let u: Vec<u16> = view_text.encode_utf16().collect();
        let Some(layout) = r.layout(&u, &format) else { continue };
        unsafe {
            let _ = layout.SetMaxHeight(lh);
            let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
        // 太字などで文字幅が変わるので、位置計算より先に書式を適用する
        spans.clear();
        tab.doc.highlight_line(line, &view_text, &mut spans);
        spans.extend(link::find_urls(&view_text).into_iter().map(|range| StyleSpan { range, kind: TokenKind::Link }));
        // 装飾表示の書式 (リンク・コード・強調)。IME の文字列を挿入した場合は位置をずらす
        let ime_byte = ime.map(|_| text.char_indices().nth(caret_vcol).map_or(text.len(), |(b, _)| b));
        let ime_bytes = ime.map_or(0, |c| c.text.len());
        let shift = |b: usize| if ime_byte.is_some_and(|at| b >= at) { b + ime_bytes } else { b };
        spans.extend(view.styles.iter().map(|s| StyleSpan { range: shift(s.range.start)..shift(s.range.end), kind: s.kind }));
        apply_styles(r, &layout, &view_text, &spans, theme);
        if ime.is_some() {
            let range = DWRITE_TEXT_RANGE { startPosition: ime_at as u32, length: ime_len as u32 };
            unsafe {
                let _ = layout.SetUnderline(true, range);
            }
        }
        tab.max_width = tab.max_width.max(Renderer::layout_width(&layout));
        let x_at = |col: usize| origin_x + Renderer::x_of(&layout, to_view(char_to_utf16(&text, vcol(col))));

        if line == caret_line && sel.is_empty() {
            r.fill(g.gutter_w, y, g.width, y + lh, theme.current_line);
        }
        if let Some(re) = ctx.matches {
            for m in re.find_iter(&text).filter(|m| !m.is_empty()) {
                let x0 = origin_x + Renderer::x_of(&layout, to_view(byte_to_utf16(&text, m.start())));
                let x1 = origin_x + Renderer::x_of(&layout, to_view(byte_to_utf16(&text, m.end())));
                r.fill(x0, y, x1, y + lh, theme.search_match);
            }
        }
        if !sel.is_empty() && sel.start() <= line_start + line_len && sel.end() >= line_start {
            let c0 = sel.start().max(line_start) - line_start;
            let c1 = sel.end().min(line_start + line_len) - line_start;
            let mut x1 = x_at(c1);
            if sel.end() > line_start + line_len {
                x1 += cw * 0.6; // 改行も選択されていることを示す
            }
            r.fill(x_at(c0), y, x1, y + lh, theme.selection);
        }
        // 変換中のキャレット位置 (変換中の文字列内のカーソル)
        let caret_x = match ime {
            Some(c) => origin_x + Renderer::x_of(&layout, ime_at + c.cursor),
            None => x_at(caret_col),
        };
        if let Some(t) = ime.and_then(|c| c.target.clone()) {
            let x0 = origin_x + Renderer::x_of(&layout, ime_at + t.start);
            let x1 = origin_x + Renderer::x_of(&layout, ime_at + t.end);
            r.fill(x0, y, x1, y + lh, theme.selection);
        } else if line == caret_line && ctx.focused && ctx.caret == CaretStyle::Block {
            // ブロックキャレットは、キャレット位置の文字 (全角・アイコン) の幅に合わせる
            let p16 = char_to_utf16(&text, caret_vcol);
            let next = text.chars().nth(caret_vcol).map_or(p16, |c| p16 + c.len_utf16());
            let w = (Renderer::x_of(&layout, next) - Renderer::x_of(&layout, p16)).max(cw);
            r.fill(caret_x, y, caret_x + w, y + lh, theme.selection);
        }

        r.draw_layout(&layout, origin_x, y, theme.foreground);

        if line == caret_line && ctx.focused {
            let x = caret_x;
            let px = 1.0 / r.scale();
            match ctx.caret {
                CaretStyle::Bar => r.fill(x, y, x + (2.0 * px).max(1.5), y + lh, theme.caret),
                CaretStyle::Underline => r.fill(x, y + lh - 2.0, x + cw, y + lh, theme.caret),
                CaretStyle::Block => {}
            }
        }
    }
    r.pop_clip();

    // 行番号
    for i in (0..rows).filter(|_| ctx.gutter) {
        let line = tab.scroll_line + i;
        if line >= len_lines {
            break;
        }
        let y = g.text_top + i as f32 * lh;
        // 詳細のある行: 行番号の右隣にバッジ (角丸の地にメモの線 3 本)
        if let Some((bx0, bx1)) = badge_x.filter(|_| tab.doc.detail_at_line(line).is_some()) {
            let bh = (lh * 0.62).max(10.0);
            let by = y + (lh - bh) / 2.0;
            r.fill_round(bx0, by, bx1, by + bh, 3.0, theme.accent, 0.9);
            for (k, frac) in [(0.3, 1.0), (0.5, 1.0), (0.7, 0.6)] {
                let ly = by + bh * k;
                let lx1 = bx0 + 3.0 + (bx1 - bx0 - 6.0) * frac;
                r.fill(bx0 + 3.0, ly - 0.6, lx1, ly + 0.6, theme.background);
            }
        }
        let num = (line + 1).to_string();
        // 行番号は右寄せ。バッジの欄があればその左に寄せる
        let num_right = badge_x.map_or(g.gutter_w - cw, |(bx0, _)| bx0 - cw * 0.5);
        let x = num_right - cw * num.len() as f32;
        let color = if line == caret_line { theme.gutter_current_fg } else { theme.gutter_fg };
        if let Some(layout) = r.layout_str(&num, &format) {
            unsafe {
                let _ = layout.SetMaxHeight(lh);
                let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            }
            r.draw_layout(&layout, x, y, color);
        }
    }
}

/// 複数行の文字列を装飾表示にする (行ごとに `LineView`、URL も検出)。表示用の文字列と書式を返す。
fn decorate_lines(content: &str) -> (String, Vec<StyleSpan>) {
    let mut text = String::with_capacity(content.len());
    let mut spans = Vec::new();
    for (i, line) in content.split('\n').enumerate() {
        if i > 0 {
            text.push('\n');
        }
        let v = LineView::build(line, None);
        let off = text.len();
        spans.extend(v.styles.iter().map(|s| StyleSpan { range: s.range.start + off..s.range.end + off, kind: s.kind }));
        text.push_str(&v.text);
    }
    spans.extend(link::find_urls(&text).into_iter().map(|range| StyleSpan { range, kind: TokenKind::Link }));
    (text, spans)
}

/// ハイライト結果をテキストレイアウトの書式として適用する。
fn apply_styles(r: &mut Renderer, layout: &IDWriteTextLayout, text: &str, spans: &[StyleSpan], theme: &Theme) {
    for sp in spans {
        let start = byte_to_utf16(text, sp.range.start);
        let end = byte_to_utf16(text, sp.range.end);
        if end <= start {
            continue;
        }
        let range = DWRITE_TEXT_RANGE { startPosition: start as u32, length: (end - start) as u32 };
        let st = theme.style(sp.kind);
        unsafe {
            if st.fg != theme.foreground {
                if let Some(b) = r.brush(st.fg) {
                    let _ = layout.SetDrawingEffect(&b, range);
                }
            }
            if st.bold {
                let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range);
            }
            if st.italic {
                let _ = layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range);
            }
            if st.underline {
                let _ = layout.SetUnderline(true, range);
            }
        }
    }
}
