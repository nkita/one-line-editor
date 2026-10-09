//! 配色。ハイライタは `TokenKind` (意味) だけを返し、色はテーマが決める。

use crate::syntax::TokenKind;

/// 0xRRGGBB 形式の色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub fn rgb_f32(&self) -> (f32, f32, f32) {
        let c = self.0;
        (((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextStyle {
    pub fg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl TextStyle {
    const fn fg(c: u32) -> Self {
        Self { fg: Color(c), bold: false, italic: false, underline: false }
    }
    const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    const fn italic(mut self) -> Self {
        self.italic = true;
        self
    }
    const fn underline(mut self) -> Self {
        self.underline = true;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: &'static str,
    pub background: Color,
    pub foreground: Color,
    pub current_line: Color,
    pub selection: Color,
    pub caret: Color,
    pub gutter_bg: Color,
    pub gutter_fg: Color,
    pub gutter_current_fg: Color,
    pub chrome_bg: Color,
    pub chrome_fg: Color,
    pub tab_active_bg: Color,
    pub tab_inactive_bg: Color,
    pub accent: Color,
    pub search_match: Color,
    pub whitespace: Color,
    tokens: [TextStyle; TokenKind::COUNT],
}

impl Theme {
    pub fn style(&self, kind: TokenKind) -> TextStyle {
        self.tokens[kind as usize]
    }

    pub fn light() -> Self {
        Self {
            name: "light",
            background: Color(0xFFFFFF),
            foreground: Color(0x1F1F1F),
            current_line: Color(0xF3F6FA),
            selection: Color(0xB4D5FE),
            caret: Color(0x000000),
            gutter_bg: Color(0xF7F7F7),
            gutter_fg: Color(0x9A9A9A),
            gutter_current_fg: Color(0x333333),
            chrome_bg: Color(0xEDEDED),
            chrome_fg: Color(0x333333),
            tab_active_bg: Color(0xFFFFFF),
            tab_inactive_bg: Color(0xE0E0E0),
            accent: Color(0x2F6FDB),
            search_match: Color(0xFFE48A),
            whitespace: Color(0xC8C8C8),
            tokens: [
                TextStyle::fg(0x0033B3).bold(),  // Keyword
                TextStyle::fg(0x067D17),         // String
                TextStyle::fg(0x8C8C8C).italic(), // Comment
                TextStyle::fg(0x1750EB),         // Number
                TextStyle::fg(0x0B4F9E).bold(),  // Heading
                TextStyle::fg(0x1F1F1F).italic(), // Emphasis
                TextStyle::fg(0x1F1F1F).bold(),  // Strong
                TextStyle::fg(0xA31515),         // Code
                TextStyle::fg(0x2F6FDB).underline(), // Link
                TextStyle::fg(0x6A737D).italic(), // Quote
                TextStyle::fg(0xD9730D).bold(),  // ListMarker
                TextStyle::fg(0x8C8C8C),         // Punctuation
            ],
        }
    }

    pub fn dark() -> Self {
        Self {
            name: "dark",
            background: Color(0x1E1F22),
            foreground: Color(0xD4D4D4),
            current_line: Color(0x26282E),
            selection: Color(0x214283),
            caret: Color(0xE8E8E8),
            gutter_bg: Color(0x1E1F22),
            gutter_fg: Color(0x5F6368),
            gutter_current_fg: Color(0xB0B0B0),
            chrome_bg: Color(0x2B2D30),
            chrome_fg: Color(0xCFCFCF),
            tab_active_bg: Color(0x1E1F22),
            tab_inactive_bg: Color(0x2B2D30),
            accent: Color(0x3574F0),
            search_match: Color(0x5C4B12),
            whitespace: Color(0x45484D),
            tokens: [
                TextStyle::fg(0xCF8E6D).bold(),
                TextStyle::fg(0x6AAB73),
                TextStyle::fg(0x7A7E85).italic(),
                TextStyle::fg(0x2AACB8),
                TextStyle::fg(0x56A8F5).bold(),
                TextStyle::fg(0xD4D4D4).italic(),
                TextStyle::fg(0xE8E8E8).bold(),
                TextStyle::fg(0xCE9178),
                TextStyle::fg(0x56A8F5).underline(),
                TextStyle::fg(0x8C8C8C).italic(),
                TextStyle::fg(0xE0A050).bold(),
                TextStyle::fg(0x7A7E85),
            ],
        }
    }
}
