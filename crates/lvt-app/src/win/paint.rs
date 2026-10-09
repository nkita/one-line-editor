//! 画面の描画 (WM_PAINT)。見えている行だけを 1 行ずつレイアウトして描く。

use lvt_core::keymap::CaretStyle;
use lvt_core::detail;
use lvt_core::link;
use lvt_core::syntax::{StyleSpan, TokenKind};
use lvt_core::theme::Theme;
use regex::Regex;
use windows::Win32::Graphics::DirectWrite::{
    IDWriteTextLayout, DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_WEIGHT_BOLD, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE, DWRITE_WORD_WRAPPING_WRAP,
};

use super::app::{prompt_input_left, status_prompt_text_left, App, Geometry, ImeComposition, Tab, TabHit};
use super::render::Renderer;
use super::util::{byte_to_utf16, char_to_utf16};

struct TextCtx<'a> {
    focused: bool,
    caret: CaretStyle,
    matches: Option<&'a Regex>,
    ime: Option<&'a ImeComposition>,
}

impl App {
    pub fn paint(&mut self) {
        if !self.renderer.begin(self.hwnd) {
            return;
        }
        let g = self.geometry();
        let theme = self.theme.clone();
        self.renderer.clear(theme.background);

        let ctx = TextCtx {
            focused: self.focused,
            caret: self.mode.caret_style(),
            matches: if self.highlight_matches { self.search_re.as_ref() } else { None },
            ime: self.ime.as_ref(),
        };
        let tab = &mut self.tabs[self.active];
        paint_text(&mut self.renderer, tab, &g, &theme, &ctx);
        self.paint_detail_panel(&g, &theme);

        self.paint_tabbar(&g, &theme);
        if self.prompt.as_ref().is_some_and(|p| !p.in_status_bar()) {
            self.paint_prompt(&g, &theme);
        }
        self.paint_status(&g, &theme);
        self.renderer.end();
    }

    /// キャレット行の詳細 (`*[…]`) の内容を、本文の右上 (キャレット行と重なるなら右下) に表示する。
    fn paint_detail_panel(&mut self, g: &Geometry, theme: &Theme) {
        self.detail_panel = None;
        let doc = &self.tabs[self.active].doc;
        let caret_line = doc.buffer().char_to_line(doc.caret_pos());
        let Some(token) = doc.detail_at_line(caret_line) else { return };
        let ls = doc.buffer().line_to_char(caret_line);
        let c = token.content();
        let content = detail::unescape(&doc.buffer().slice_to_string(ls + c.start..ls + c.end));
        let scroll_line = self.tabs[self.active].scroll_line;

        let r = &mut self.renderer;
        let text_w = g.width - g.text_left;
        let text_h = g.text_bottom - g.text_top;
        if text_w < 260.0 || text_h < 120.0 {
            return;
        }
        let pad = 10.0;
        let w = (text_w * 0.4).clamp(240.0, 520.0);
        let inner_w = w - pad * 2.0;
        let header_h = r.ui_line_height + 6.0;
        let empty = content.is_empty();
        let shown = if empty { "(空です。F2 で編集)".to_string() } else { content };
        let Some(layout) = r.layout_str(&shown, &r.text_format.clone()) else { return };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            let _ = layout.SetMaxWidth(inner_w);
            let _ = layout.GetMetrics(&mut m);
        }
        let max_body = (text_h * 0.5 - header_h - pad * 2.0).max(r.line_height);
        let body_h = m.height.min(max_body);
        let h = pad + header_h + body_h + pad;
        let x = g.width - w - 12.0;
        // キャレット行と重なるなら下へ寄せる
        let caret_y = g.text_top + (caret_line as f32 - scroll_line as f32) * r.line_height;
        let mut y = g.text_top + 8.0;
        if caret_y < y + h && caret_y + r.line_height > y {
            y = g.text_bottom - h - 8.0;
        }

        r.fill(x - 1.0, y - 1.0, x + w + 1.0, y + h + 1.0, theme.accent);
        r.fill(x, y, x + w, y + h, theme.chrome_bg);
        let title = format!("{} 詳細  (F2 / クリックで編集)", detail::ICON);
        r.push_clip(x, y, x + w, y + h);
        r.draw_ui_text(&title, x + pad, y + pad * 0.5, header_h, theme.gutter_fg);
        let body_top = y + pad + header_h;
        r.push_clip(x + pad, body_top, x + w - pad, body_top + body_h);
        r.draw_layout(&layout, x + pad, body_top, if empty { theme.gutter_fg } else { theme.foreground });
        r.pop_clip();
        if m.height > body_h {
            r.draw_ui_text("…", x + w - pad - 12.0, y + h - pad - r.ui_line_height, r.ui_line_height, theme.gutter_fg);
        }
        r.pop_clip();
        self.detail_panel = Some((x, y, x + w, y + h));
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
        let doc = &self.tabs[self.active].doc;
        let sel = doc.selection();
        let (line, col) = doc.buffer().line_col(doc.caret_pos());
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
    let lh = r.line_height;
    let cw = r.char_width;
    let sel = tab.doc.selection();
    let (caret_line, caret_col) = tab.doc.buffer().line_col(tab.doc.caret_pos());
    let len_lines = tab.doc.buffer().len_lines();
    let rows = ((g.text_bottom - g.text_top) / lh).ceil() as usize + 1;
    let origin_x = g.text_left - tab.scroll_x;
    let format = r.text_format.clone();
    let mut spans: Vec<StyleSpan> = Vec::new();

    r.fill(0.0, g.text_top, g.gutter_w, g.text_bottom, theme.gutter_bg);
    r.push_clip(g.gutter_w, g.text_top, g.width, g.text_bottom);
    for i in 0..rows {
        let line = tab.scroll_line + i;
        if line >= len_lines {
            break;
        }
        let y = g.text_top + i as f32 * lh;
        let line_start = tab.doc.buffer().line_to_char(line);
        let line_len = tab.doc.buffer().line_len(line);
        // 表示用の行 (行頭の詳細 `*[…]` はアイコン 1 文字に折りたたむ)。
        // 文書上の桁 → 表示上の桁は `vcol` で変換する。
        let (text, token) = App::view_line(&tab.doc, line);
        let vcol = |col: usize| token.map_or(col, |t| t.to_view(col, false));
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
    for i in 0..rows {
        let line = tab.scroll_line + i;
        if line >= len_lines {
            break;
        }
        let y = g.text_top + i as f32 * lh;
        let num = (line + 1).to_string();
        let x = g.gutter_w - cw * (num.len() as f32 + 1.0);
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
