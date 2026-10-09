//! Direct2D / DirectWrite による描画の基本部品。
//!
//! 何をどこに描くかは paint.rs (App 側) が決め、このモジュールは
//! デバイス資源 (レンダーターゲット・ブラシ・テキスト書式) の管理と
//! 描画プリミティブだけを提供する。

use std::collections::HashMap;

use lvt_core::theme::Color;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_UNKNOWN, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_ROUNDED_RECT,
    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_HIT_TEST_METRICS,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows_numerics::Vector2;

use super::util::wide;

pub const DEFAULT_FONT_SIZE: f32 = 15.0;
const EDITOR_FONT: &str = "Consolas";
const UI_FONT: &str = "Segoe UI";
const UI_FONT_SIZE: f32 = 12.5;
const LOCALE: &str = "ja-jp";

pub struct Renderer {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    target: Option<ID2D1HwndRenderTarget>,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
    pub text_format: IDWriteTextFormat,
    pub ui_format: IDWriteTextFormat,
    pub font_size: f32,
    pub tab_width: usize,
    /// 1 行の高さ (DIP)。デバイスピクセルに揃えてある。
    pub line_height: f32,
    /// 半角 1 文字の幅 (DIP)。
    pub char_width: f32,
    pub ui_line_height: f32,
    pub dpi: f32,
}

impl Renderer {
    pub fn new(hwnd: HWND, tab_width: usize) -> Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let dpi = match GetDpiForWindow(hwnd) {
                0 => 96.0,
                d => d as f32,
            };
            let text_format = create_format(&dw, EDITOR_FONT, DEFAULT_FONT_SIZE)?;
            let ui_format = create_format(&dw, UI_FONT, UI_FONT_SIZE)?;
            let mut r = Self {
                d2d,
                dw,
                target: None,
                brushes: HashMap::new(),
                text_format,
                ui_format,
                font_size: DEFAULT_FONT_SIZE,
                tab_width,
                line_height: 0.0,
                char_width: 0.0,
                ui_line_height: 0.0,
                dpi,
            };
            r.rebuild_formats()?;
            Ok(r)
        }
    }

    pub fn scale(&self) -> f32 {
        self.dpi / 96.0
    }

    /// DIP をデバイスピクセル境界に丸める。
    pub fn snap(&self, v: f32) -> f32 {
        (v * self.scale()).round() / self.scale()
    }

    pub fn set_dpi(&mut self, dpi: f32) -> Result<()> {
        self.dpi = dpi;
        if let Some(t) = &self.target {
            unsafe { t.SetDpi(dpi, dpi) };
        }
        self.rebuild_formats()
    }

    pub fn set_font_size(&mut self, size: f32) -> Result<()> {
        self.font_size = size.clamp(6.0, 72.0);
        self.text_format = create_format(&self.dw, EDITOR_FONT, self.font_size)?;
        self.rebuild_formats()
    }

    fn rebuild_formats(&mut self) -> Result<()> {
        let m = self.measure("M", &self.text_format.clone());
        self.char_width = m.width.max(1.0);
        self.line_height = ((m.height * 1.15 * self.scale()).ceil() / self.scale()).max(1.0);
        unsafe { self.text_format.SetIncrementalTabStop(self.char_width * self.tab_width.max(1) as f32)? };
        let u = self.measure("Mg", &self.ui_format.clone());
        self.ui_line_height = (u.height * self.scale()).ceil() / self.scale();
        Ok(())
    }

    pub fn layout(&self, text: &[u16], format: &IDWriteTextFormat) -> Option<IDWriteTextLayout> {
        unsafe { self.dw.CreateTextLayout(text, format, 1.0e6, 1.0e4).ok() }
    }

    pub fn layout_str(&self, text: &str, format: &IDWriteTextFormat) -> Option<IDWriteTextLayout> {
        let w: Vec<u16> = text.encode_utf16().collect();
        self.layout(&w, format)
    }

    pub fn measure(&self, text: &str, format: &IDWriteTextFormat) -> DWRITE_TEXT_METRICS {
        let mut m = DWRITE_TEXT_METRICS::default();
        if let Some(l) = self.layout_str(text, format) {
            unsafe {
                let _ = l.GetMetrics(&mut m);
            }
        }
        m
    }

    pub fn text_width(&self, text: &str, format: &IDWriteTextFormat) -> f32 {
        self.measure(text, format).widthIncludingTrailingWhitespace
    }

    /// レイアウト内の UTF-16 位置の x 座標。
    pub fn x_of(layout: &IDWriteTextLayout, pos16: usize) -> f32 {
        let (mut x, mut y) = (0.0, 0.0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = layout.HitTestTextPosition(pos16 as u32, false, &mut x, &mut y, &mut m);
        }
        x
    }

    /// x 座標に最も近い UTF-16 位置 (文字の右半分なら次の位置)。
    pub fn pos_at(layout: &IDWriteTextLayout, x: f32) -> usize {
        let mut trailing = Default::default();
        let mut inside = Default::default();
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = layout.HitTestPoint(x, 1.0, &mut trailing, &mut inside, &mut m);
        }
        m.textPosition as usize + if trailing.as_bool() { m.length as usize } else { 0 }
    }

    pub fn layout_width(layout: &IDWriteTextLayout) -> f32 {
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.GetMetrics(&mut m);
        }
        m.widthIncludingTrailingWhitespace
    }

    // ---- 描画 ----

    /// 描画開始。レンダーターゲットがなければ作り、サイズが変わっていれば合わせる。
    pub fn begin(&mut self, hwnd: HWND) -> bool {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        let size = D2D_SIZE_U { width: (rc.right - rc.left).max(1) as u32, height: (rc.bottom - rc.top).max(1) as u32 };
        if self.target.is_none() {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_UNKNOWN, alphaMode: D2D1_ALPHA_MODE_UNKNOWN },
                dpiX: self.dpi,
                dpiY: self.dpi,
                ..Default::default()
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd, pixelSize: size, presentOptions: D2D1_PRESENT_OPTIONS_NONE };
            match unsafe { self.d2d.CreateHwndRenderTarget(&props, &hprops) } {
                Ok(t) => self.target = Some(t),
                Err(_) => return false,
            }
            self.brushes.clear();
        }
        let t = self.target.as_ref().unwrap();
        unsafe {
            let cur = t.GetPixelSize();
            if cur.width != size.width || cur.height != size.height {
                let _ = t.Resize(&size);
            }
            t.BeginDraw();
        }
        true
    }

    pub fn end(&mut self) {
        if let Some(t) = &self.target {
            if let Err(e) = unsafe { t.EndDraw(None, None) } {
                if e.code() == D2DERR_RECREATE_TARGET {
                    self.target = None;
                    self.brushes.clear();
                }
            }
        }
    }

    pub fn brush(&mut self, c: Color) -> Option<ID2D1SolidColorBrush> {
        if let Some(b) = self.brushes.get(&c.0) {
            return Some(b.clone());
        }
        let (r, g, b) = c.rgb_f32();
        let color = D2D1_COLOR_F { r, g, b, a: 1.0 };
        let brush = unsafe { self.target.as_ref()?.CreateSolidColorBrush(&color, None).ok()? };
        self.brushes.insert(c.0, brush.clone());
        Some(brush)
    }

    /// 半透明のブラシ (`alpha` は 0.0〜1.0)。
    fn brush_alpha(&mut self, c: Color, alpha: f32) -> Option<ID2D1SolidColorBrush> {
        let a = (alpha.clamp(0.0, 1.0) * 255.0) as u32;
        if a == 255 {
            return self.brush(c);
        }
        // 不透明のブラシ (キー = 色) と区別するため、上位 8 ビットに alpha + 1 を入れる
        let key = c.0 | ((a + 1) << 24);
        if let Some(b) = self.brushes.get(&key) {
            return Some(b.clone());
        }
        let (r, g, b) = c.rgb_f32();
        let color = D2D1_COLOR_F { r, g, b, a: a as f32 / 255.0 };
        let brush = unsafe { self.target.as_ref()?.CreateSolidColorBrush(&color, None).ok()? };
        self.brushes.insert(key, brush.clone());
        Some(brush)
    }

    /// 角丸の矩形を塗る。
    #[allow(clippy::too_many_arguments)]
    pub fn fill_round(&mut self, left: f32, top: f32, right: f32, bottom: f32, radius: f32, c: Color, alpha: f32) {
        let Some(b) = self.brush_alpha(c, alpha) else { return };
        let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left, top, right, bottom }, radiusX: radius, radiusY: radius };
        if let Some(t) = &self.target {
            unsafe { t.FillRoundedRectangle(&rr, &b) };
        }
    }

    /// 角丸の矩形の枠線を描く。
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_round(&mut self, left: f32, top: f32, right: f32, bottom: f32, radius: f32, c: Color, width: f32) {
        let Some(b) = self.brush(c) else { return };
        let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left, top, right, bottom }, radiusX: radius, radiusY: radius };
        if let Some(t) = &self.target {
            unsafe { t.DrawRoundedRectangle(&rr, &b, width, None) };
        }
    }

    pub fn clear(&mut self, c: Color) {
        let (r, g, b) = c.rgb_f32();
        if let Some(t) = &self.target {
            unsafe { t.Clear(Some(&D2D1_COLOR_F { r, g, b, a: 1.0 })) };
        }
    }

    pub fn fill(&mut self, left: f32, top: f32, right: f32, bottom: f32, c: Color) {
        let Some(b) = self.brush(c) else { return };
        if let Some(t) = &self.target {
            unsafe { t.FillRectangle(&D2D_RECT_F { left, top, right, bottom }, &b) };
        }
    }

    pub fn draw_layout(&mut self, layout: &IDWriteTextLayout, x: f32, y: f32, c: Color) {
        let Some(b) = self.brush(c) else { return };
        if let Some(t) = &self.target {
            unsafe { t.DrawTextLayout(Vector2 { X: x, Y: y }, layout, &b, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT) };
        }
    }

    /// UI 文字列を描き、その幅を返す。`height` の中で縦中央に置く。
    pub fn draw_ui_text(&mut self, text: &str, x: f32, y: f32, height: f32, c: Color) -> f32 {
        let Some(layout) = self.layout_str(text, &self.ui_format.clone()) else { return 0.0 };
        unsafe {
            let _ = layout.SetMaxHeight(height);
            let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
        self.draw_layout(&layout, x, y, c);
        Self::layout_width(&layout)
    }

    pub fn push_clip(&mut self, left: f32, top: f32, right: f32, bottom: f32) {
        if let Some(t) = &self.target {
            unsafe { t.PushAxisAlignedClip(&D2D_RECT_F { left, top, right, bottom }, D2D1_ANTIALIAS_MODE_ALIASED) };
        }
    }

    pub fn pop_clip(&mut self) {
        if let Some(t) = &self.target {
            unsafe { t.PopAxisAlignedClip() };
        }
    }
}

fn create_format(dw: &IDWriteFactory, family: &str, size: f32) -> Result<IDWriteTextFormat> {
    let fam = wide(family);
    let loc = wide(LOCALE);
    unsafe {
        let f = dw.CreateTextFormat(
            PCWSTR(fam.as_ptr()),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            PCWSTR(loc.as_ptr()),
        )?;
        f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(f)
    }
}
