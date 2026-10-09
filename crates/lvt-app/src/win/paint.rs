//! 画面の描画 (WM_PAINT)。見えている行だけを 1 行ずつレイアウトして描く。

use lvt_core::keymap::CaretStyle;
use lvt_core::syntax::StyleSpan;
use lvt_core::theme::Theme;
use regex::Regex;
use windows::Win32::Graphics::DirectWrite::{
    IDWriteTextLayout, DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_WEIGHT_BOLD, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_RANGE,
};

use super::app::{prompt_input_left, App, Geometry, Tab, TabHit};
use super::render::Renderer;
use super::util::{byte_to_utf16, char_to_utf16};

struct TextCtx<'a> {
    focused: bool,
    caret: CaretStyle,
    matches: Option<&'a Regex>,
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
        };
        let tab = &mut self.tabs[self.active];
        paint_text(&mut self.renderer, tab, &g, &theme, &ctx);

        self.paint_tabbar(&g, &theme);
        if self.prompt.is_some() {
            self.paint_prompt(&g, &theme);
        }
        self.paint_status(&g, &theme);
        self.renderer.end();
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
        let (line, col) = doc.buffer().line_col(sel.head);
        let left = self.status.clone().unwrap_or_else(|| {
            let mut s = format!("{} 行, {} 桁", line + 1, col + 1);
            if !sel.is_empty() {
                s.push_str(&format!("  ({} 文字選択)", sel.end() - sel.start()));
            }
            s
        });
        let mut right: Vec<&str> = Vec::new();
        if !self.mode.name().is_empty() {
            right.push(self.mode.name());
        }
        right.extend([doc.highlighter_name(), doc.encoding.name(), doc.line_ending.name()]);
        let right = right.join("    ");

        let r = &mut self.renderer;
        let (top, bottom) = (g.status_top, g.height);
        r.fill(0.0, top, g.width, bottom, theme.chrome_bg);
        r.fill(0.0, top, g.width, top + 1.0, theme.tab_inactive_bg);
        let rw = r.text_width(&right, &r.ui_format);
        r.push_clip(0.0, top, (g.width - rw - 24.0).max(0.0), bottom);
        r.draw_ui_text(&left, 12.0, top, bottom - top, theme.chrome_fg);
        r.pop_clip();
        r.draw_ui_text(&right, g.width - rw - 12.0, top, bottom - top, theme.chrome_fg);
    }
}

fn paint_text(r: &mut Renderer, tab: &mut Tab, g: &Geometry, theme: &Theme, ctx: &TextCtx) {
    let lh = r.line_height;
    let cw = r.char_width;
    let sel = tab.doc.selection();
    let (caret_line, caret_col) = tab.doc.buffer().line_col(sel.head);
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
        let text = App::line_text(&tab.doc, line);
        let u: Vec<u16> = text.encode_utf16().collect();
        let Some(layout) = r.layout(&u, &format) else { continue };
        unsafe {
            let _ = layout.SetMaxHeight(lh);
            let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
        // 太字などで文字幅が変わるので、位置計算より先に書式を適用する
        spans.clear();
        tab.doc.highlight_line(line, &text, &mut spans);
        apply_styles(r, &layout, &text, &spans, theme);
        tab.max_width = tab.max_width.max(Renderer::layout_width(&layout));
        let x_at = |col: usize| origin_x + Renderer::x_of(&layout, char_to_utf16(&text, col));

        if line == caret_line && sel.is_empty() {
            r.fill(g.gutter_w, y, g.width, y + lh, theme.current_line);
        }
        if let Some(re) = ctx.matches {
            for m in re.find_iter(&text).filter(|m| !m.is_empty()) {
                let x0 = origin_x + Renderer::x_of(&layout, byte_to_utf16(&text, m.start()));
                let x1 = origin_x + Renderer::x_of(&layout, byte_to_utf16(&text, m.end()));
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
        if line == caret_line && ctx.focused && ctx.caret == CaretStyle::Block {
            let x = x_at(caret_col);
            r.fill(x, y, x + cw, y + lh, theme.selection);
        }

        r.draw_layout(&layout, origin_x, y, theme.foreground);

        if line == caret_line && ctx.focused {
            let x = x_at(caret_col);
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
