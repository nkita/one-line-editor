//! 詳細パネル: キャレット行の詳細 (`*detail[…]`) の表示と、パネル内での編集。
//!
//! - 表示: キャレット行に詳細があれば、内容を折り返して表示する (paint.rs の `paint_detail_panel`)。
//! - 編集: パネルを大きくし、本体と同じ `Document` + 入力モード (Vim) + 描画で編集する。
//!   編集中は `App::tab()` / `App::geometry()` がパネル側を返すので、キー入力・IME・
//!   検索・Undo などの既存の処理がそのまま編集中の文書に働く。
//!   閉じるとき (Esc、パネルの外のクリック、`:wq`) に内容をエスケープして本体へ書き戻す。
//! - 位置: 見出しの帯をドラッグして動かせる。位置は本文の領域に対する割合で設定に保存する。

use lvt_core::{Document, Selection};

use super::app::{App, Geometry, Tab};

/// 編集中の詳細。
pub struct DetailEdit {
    pub tab: Tab,
    /// 書き戻し先のタブと行。
    pub idx: usize,
    pub line: usize,
    /// 編集前の内容 (変更がなければ書き戻さない)。
    pub original: String,
}

/// パネルの領域 (DIP)。
#[derive(Clone, Copy, Debug)]
pub struct PanelRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    /// 見出しの帯の下端。
    pub header_bottom: f32,
}

impl PanelRect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub fn in_header(&self, x: f32, y: f32) -> bool {
        self.contains(x, y) && y < self.header_bottom
    }
}

/// パネルのスクロールバー (DIP)。値はスクロール量 (表示中は DIP、編集中は行)。
#[derive(Clone, Copy, Debug)]
pub struct PanelScrollbar {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub thumb_top: f32,
    pub thumb_h: f32,
    pub max: f32,
}

impl PanelScrollbar {
    /// 幅の狭いバーなので、左右に少し広げて判定する。
    pub fn hit(&self, x: f32, y: f32) -> bool {
        x >= self.left - 4.0 && x < self.right + 3.0 && y >= self.top && y < self.bottom
    }

    /// つまみの上端が `thumb_top` のときの値。
    pub fn value_at(&self, thumb_top: f32) -> f32 {
        let range = (self.bottom - self.top - self.thumb_h).max(1.0);
        ((thumb_top - self.top) / range).clamp(0.0, 1.0) * self.max
    }

    /// 値 `v` (0..=max) のときのつまみ。トラックの縦の範囲と高さの割合から作る。
    pub fn new(left: f32, top: f32, bottom: f32, visible_ratio: f32, v: f32, max: f32) -> Self {
        let track = bottom - top;
        let thumb_h = (track * visible_ratio.clamp(0.0, 1.0)).max(18.0).min(track);
        let frac = if max > 0.0 { (v / max).clamp(0.0, 1.0) } else { 0.0 };
        Self { left, top, right: left + 4.0, bottom, thumb_top: top + (track - thumb_h) * frac, thumb_h, max }
    }
}

/// パネルの外側の余白と角の半径 (DIP)。
pub const PANEL_MARGIN: f32 = 10.0;
pub const PANEL_RADIUS: f32 = 8.0;

impl App {
    pub fn panel_header_h(&self) -> f32 {
        self.renderer.snap(self.renderer.ui_line_height + 14.0)
    }

    /// 編集中のパネルの大きさ。
    fn edit_panel_size(&self, g: &Geometry) -> (f32, f32) {
        let area_w = g.text_right - g.gutter_w - PANEL_MARGIN * 2.0;
        let area_h = g.text_bottom - g.text_top - PANEL_MARGIN * 2.0;
        let w = ((g.text_right - g.gutter_w) * 0.55).clamp(320.0, 760.0).min(area_w).max(120.0);
        let h = ((g.text_bottom - g.text_top) * 0.6).clamp(200.0, 560.0).min(area_h).max(80.0);
        (w, h)
    }

    /// パネルの位置。`avoid` (キャレット行の上下端) と重なる場合は、位置を動かしていなければ下へ寄せる。
    pub fn panel_rect(&self, g: &Geometry, w: f32, h: f32, avoid: Option<(f32, f32)>) -> PanelRect {
        let x0 = g.gutter_w + PANEL_MARGIN;
        let x1 = (g.text_right - w - PANEL_MARGIN).max(x0);
        let y0 = g.text_top + PANEL_MARGIN;
        let y1 = (g.text_bottom - h - PANEL_MARGIN).max(y0);
        let (fx, fy) = self.panel_pos.unwrap_or((1.0, 0.0));
        let x = x0 + (x1 - x0) * fx;
        let mut y = y0 + (y1 - y0) * fy;
        if let (None, Some((c0, c1))) = (self.panel_pos, avoid) {
            if c0 < y + h && c1 > y {
                y = y1;
            }
        }
        PanelRect { left: x, top: y, right: x + w, bottom: y + h, header_bottom: y + self.panel_header_h() }
    }

    /// 編集中のパネルの領域。
    pub fn edit_panel_rect(&self, g: &Geometry) -> PanelRect {
        let (w, h) = self.edit_panel_size(g);
        self.panel_rect(g, w, h, None)
    }

    /// 編集中の本文の配置 (パネルの内側)。チップやステータスバーの位置は本体のまま。
    pub fn edit_geometry(&self, g: Geometry) -> Geometry {
        let p = self.edit_panel_rect(&g);
        Geometry {
            text_right: p.right - 14.0, // 右端はスクロールバー
            gutter_w: p.left + 6.0, // 現在行の帯が左の枠線に重ならないよう内側から始める
            text_left: p.left + 14.0,
            text_top: p.header_bottom + 6.0,
            text_bottom: p.bottom - 10.0,
            ..g
        }
    }

    /// パネルをドラッグで動かす (`left` / `top` はパネルの左上の希望位置)。
    pub fn move_panel_to(&mut self, left: f32, top: f32) {
        let g = self.main_geometry();
        let Some(p) = self.detail_panel else { return };
        let (w, h) = (p.right - p.left, p.bottom - p.top);
        let x0 = g.gutter_w + PANEL_MARGIN;
        let x1 = g.text_right - w - PANEL_MARGIN;
        let y0 = g.text_top + PANEL_MARGIN;
        let y1 = g.text_bottom - h - PANEL_MARGIN;
        let frac = |v: f32, a: f32, b: f32| if b > a { ((v - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
        self.panel_pos = Some((frac(left, x0, x1), frac(top, y0, y1)));
        self.invalidate();
    }

    /// スクロールバーのつまみをマウスの位置 (`grab` はつまみの中の掴んだ位置) に合わせる。
    pub fn scroll_panel_to_mouse(&mut self, y: f32, grab: f32) {
        let Some(sb) = self.panel_scrollbar else { return };
        let v = sb.value_at(y - grab);
        match &mut self.detail_edit {
            Some(e) => e.tab.scroll_line = v.round() as usize,
            None => self.panel_scroll = v,
        }
        self.invalidate();
    }

    /// 行番号の欄の、詳細のバッジの左右。行番号の右隣 (本文との間) に置く。
    /// 装飾表示でなければ None (バッジの欄がない)。
    pub fn badge_x(&self, g: &Geometry) -> Option<(f32, f32)> {
        if !self.main_tab().doc.detail_view() {
            return None;
        }
        let cw = self.renderer.char_width;
        let w = (cw * 1.3).max(11.0);
        let right = g.gutter_w - cw * 0.4;
        Some((right - w, right))
    }

    /// 画面座標が行番号の欄の詳細のバッジの上なら、その行。
    pub fn badge_at(&self, g: &Geometry, x: f32, y: f32) -> Option<usize> {
        let (x0, x1) = self.badge_x(g)?;
        if self.detail_edit.is_some() || x < x0 - 3.0 || x >= x1 + 3.0 || y < g.text_top || y >= g.text_bottom {
            return None;
        }
        let tab = self.main_tab();
        let line = tab.scroll_line + ((y - g.text_top) / self.renderer.line_height) as usize;
        (line < tab.doc.buffer().len_lines() && tab.doc.detail_at_line(line).is_some()).then_some(line)
    }

    /// 本体の `line` 行の詳細を削除する (行が空白だけになったら行ごと)。1 回の Undo で戻せる。
    pub fn delete_detail(&mut self, line: usize) {
        self.finish_detail_edit(true);
        if self.tabs[self.active].doc.delete_detail(line) {
            let undo = if self.vim_enabled { "u" } else { "Ctrl+Z" };
            self.status = Some(format!("詳細を削除しました ({undo} で元に戻せます)"));
            self.ensure_caret_visible();
        }
        self.refresh();
    }

    /// ドラッグで動かした位置を設定に保存する。
    pub fn save_panel_pos(&mut self) {
        let Some((fx, fy)) = self.panel_pos else { return };
        self.settings.set("detail_panel_pos", format!("{fx:.4},{fy:.4}"));
        self.save_settings();
    }

    // ---- 編集 ----

    /// キャレット行 (`line`) の詳細をパネルで編集する。詳細がなければ空の内容から始め、確定したら行末に作る。
    pub fn start_detail_edit(&mut self, line: usize) {
        if self.detail_edit.is_some() {
            return;
        }
        if !self.main_tab().doc.detail_view() {
            self.status = Some(if self.txt_detail_view {
                "詳細 (*detail[…]) は .txt / .lvt ファイルで使えます"
            } else {
                "詳細 (*detail[…]) は .lvt ファイルで使えます (.txt は表示メニューで有効にできます)"
            }
            .into());
            return;
        }
        let Some(text) = self.detail_text(self.active, line) else { return };
        let mut doc = Document::from_text(&text);
        doc.set_detail_view(true); // パネルの中でもリンク・強調などを装飾する
        doc.set_selection(Selection::caret(0));
        let original = doc.buffer().to_string();
        self.mode.reset();
        self.ime = None;
        self.highlight_matches = false;
        self.detail_edit = Some(DetailEdit { tab: Tab::new(doc), idx: self.active, line, original });
        self.status = Some(if self.vim_enabled {
            "詳細を編集中: ノーマルモードの Esc で反映して閉じる (:q! で破棄)".into()
        } else {
            "詳細を編集中: Esc で反映して閉じる".into()
        });
        self.ensure_caret_visible();
        self.refresh();
    }

    /// 詳細の編集を終える。`apply` なら本体へ書き戻す (変更がなければ何もしない)。
    pub fn finish_detail_edit(&mut self, apply: bool) {
        let Some(e) = self.detail_edit.take() else { return };
        self.mode.reset();
        self.ime = None;
        self.prompt = None;
        self.highlight_matches = false;
        let text = e.tab.doc.buffer().to_string();
        if apply && text != e.original {
            self.write_detail(e.idx, e.line, &text);
        } else {
            self.status = Some(if apply { "詳細は変更されていません" } else { "詳細の変更を破棄しました" }.into());
        }
        self.refresh();
    }

    /// 編集を続けたまま本体へ書き戻す (`:w`)。
    pub fn write_detail_edit(&mut self) {
        let Some(e) = &self.detail_edit else { return };
        let (idx, line, text) = (e.idx, e.line, e.tab.doc.buffer().to_string());
        if text != e.original {
            self.write_detail(idx, line, &text);
        }
        if let Some(e) = &mut self.detail_edit {
            e.original = text;
        }
        self.status = Some("詳細を反映しました".into());
    }
}
