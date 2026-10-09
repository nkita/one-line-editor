//! 本文のスクロールバー (自前で描画する)。
//!
//! ウィンドウ標準のスクロールバー (WS_VSCROLL / WS_HSCROLL) はクライアント領域の外周に付き、
//! ステータスバーの横まで伸びてしまう。本文の領域の右と下にだけ置くため、自前で描画・操作する。
//! - 縦: 本文の右 (タブバーの下〜横のバーの上)。横: 本文の下 (入力欄・ステータスバーの上)。
//! - つまみのドラッグでスクロール、トラックのクリックで 1 ページ分スクロールする。
//! - マウスが乗ると (またはドラッグ中は) つまみを太く・濃くする。

use lvt_core::theme::Theme;

use super::app::{App, Geometry};

/// スクロールバーの欄の幅 (DIP)。
pub const SCROLLBAR_W: f32 = 12.0;

#[derive(Clone, Copy, Debug)]
pub struct ScrollBar {
    pub vertical: bool,
    /// 欄の領域。
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    /// つまみの開始位置と長さ (縦なら y、横なら x)。
    pub thumb_start: f32,
    pub thumb_len: f32,
    /// 値 (スクロール量) の最大。0 ならスクロールできない (つまみを描かない)。
    pub max: f32,
}

impl ScrollBar {
    /// `visible_ratio` は表示できる割合、`v` は現在の値 (0..=max)。
    fn new(vertical: bool, rect: (f32, f32, f32, f32), visible_ratio: f32, v: f32, max: f32) -> Self {
        let (left, top, right, bottom) = rect;
        let mut sb = Self { vertical, left, top, right, bottom, thumb_start: 0.0, thumb_len: 0.0, max: max.max(0.0) };
        let (t0, t1) = sb.track();
        let track = (t1 - t0).max(0.0);
        sb.thumb_len = (track * visible_ratio.clamp(0.0, 1.0)).max(24.0).min(track);
        let frac = if sb.max > 0.0 { (v / sb.max).clamp(0.0, 1.0) } else { 0.0 };
        sb.thumb_start = t0 + (track - sb.thumb_len) * frac;
        sb
    }

    /// つまみが動く範囲 (両端に少し余白を取る)。
    fn track(&self) -> (f32, f32) {
        if self.vertical {
            (self.top + 2.0, self.bottom - 2.0)
        } else {
            (self.left + 2.0, self.right - 2.0)
        }
    }

    /// マウスの位置のうち、バーの向きの成分。
    pub fn axis(&self, x: f32, y: f32) -> f32 {
        if self.vertical {
            y
        } else {
            x
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub fn in_thumb(&self, x: f32, y: f32) -> bool {
        let a = self.axis(x, y);
        self.hit(x, y) && a >= self.thumb_start && a < self.thumb_start + self.thumb_len
    }

    /// つまみの開始位置が `start` のときの値。
    pub fn value_at(&self, start: f32) -> f32 {
        let (t0, t1) = self.track();
        let range = (t1 - t0 - self.thumb_len).max(1.0);
        ((start - t0) / range).clamp(0.0, 1.0) * self.max
    }
}

impl App {
    /// 本体の縦・横のスクロールバー (詳細の編集中でも本体を表す)。
    pub fn main_scrollbars(&self) -> (ScrollBar, ScrollBar) {
        let g = self.main_geometry();
        let tab = self.main_tab();
        let lh = self.renderer.line_height;
        let lines = tab.doc.buffer().len_lines() as f32;
        let vis = ((g.text_bottom - g.text_top) / lh).floor().max(1.0);
        let vmax = (lines - vis).max(0.0);
        let v = ScrollBar::new(true, (g.text_right, g.text_top, g.width, g.text_bottom), vis / lines.max(1.0), tab.scroll_line as f32, vmax);
        let area_w = (g.text_right - g.text_left).max(1.0);
        let content_w = tab.max_width + self.renderer.char_width * 4.0;
        let hmax = (content_w - area_w).max(0.0);
        let h = ScrollBar::new(false, (g.gutter_w, g.text_bottom, g.text_right, g.prompt_top), area_w / content_w.max(1.0), tab.scroll_x, hmax);
        (v, h)
    }

    /// 座標にある本体のスクロールバー。
    pub fn main_scrollbar_at(&self, x: f32, y: f32) -> Option<ScrollBar> {
        let (v, h) = self.main_scrollbars();
        [v, h].into_iter().find(|sb| sb.hit(x, y))
    }

    /// 本体のスクロール量を設定する (縦は行、横は DIP)。
    pub fn set_main_scroll(&mut self, vertical: bool, value: f32) {
        let tab = &mut self.tabs[self.active];
        if vertical {
            tab.scroll_line = value.round().max(0.0) as usize;
        } else {
            tab.scroll_x = value.max(0.0);
        }
        self.refresh();
    }

    /// スクロールバーのクリック: つまみならドラッグを始める (掴んだ位置を返す)。
    /// トラックならクリックした側へ 1 ページスクロールする (None を返す)。
    pub fn scrollbar_click(&mut self, sb: &ScrollBar, x: f32, y: f32) -> Option<f32> {
        let a = sb.axis(x, y);
        if sb.in_thumb(x, y) {
            return Some(a - sb.thumb_start);
        }
        if sb.max <= 0.0 {
            return None;
        }
        let dir = if a < sb.thumb_start { -1.0 } else { 1.0 };
        let tab = &self.tabs[self.active];
        let g = self.main_geometry();
        let (cur, page) = if sb.vertical {
            let vis = ((g.text_bottom - g.text_top) / self.renderer.line_height).floor().max(1.0);
            (tab.scroll_line as f32, vis - 1.0)
        } else {
            (tab.scroll_x, (g.text_right - g.text_left) * 0.8)
        };
        self.set_main_scroll(sb.vertical, (cur + dir * page.max(1.0)).clamp(0.0, sb.max));
        None
    }

    /// つまみのドラッグ中 (`grab` はつまみの開始位置からのマウスの位置)。
    pub fn drag_scrollbar(&mut self, vertical: bool, grab: f32) {
        let (v, h) = self.main_scrollbars();
        let sb = if vertical { v } else { h };
        let (mx, my) = self.mouse_pos();
        self.set_main_scroll(vertical, sb.value_at(sb.axis(mx, my) - grab));
    }

    pub fn paint_scrollbars(&mut self, g: &Geometry, theme: &Theme) {
        let (v, h) = self.main_scrollbars();
        let active = self.scrollbar_active();
        let r = &mut self.renderer;
        // 欄の地 (横の欄の行番号の部分と、右下の角も本文と同じ地にする)
        r.fill(v.left, v.top, v.right, g.prompt_top, theme.background);
        r.fill(h.left, h.top, h.right, h.bottom, theme.background);
        r.fill(0.0, h.top, g.gutter_w, h.bottom, theme.gutter_bg);
        for sb in [v, h] {
            if sb.max <= 0.0 {
                continue;
            }
            let hot = active == Some(sb.vertical);
            let thick = if hot { 8.0 } else { 6.0 };
            let alpha = if hot { 0.75 } else { 0.45 };
            let (t0, t1) = (sb.thumb_start, sb.thumb_start + sb.thumb_len);
            if sb.vertical {
                let cx = (sb.left + sb.right) / 2.0;
                r.fill_round(cx - thick / 2.0, t0, cx + thick / 2.0, t1, thick / 2.0, theme.gutter_fg, alpha);
            } else {
                let cy = (sb.top + sb.bottom) / 2.0;
                r.fill_round(t0, cy - thick / 2.0, t1, cy + thick / 2.0, thick / 2.0, theme.gutter_fg, alpha);
            }
        }
    }
}
