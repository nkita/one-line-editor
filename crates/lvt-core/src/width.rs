//! 表示幅 (桁) の計算。全角文字は 2 桁、タブは次のタブ位置まで。
//!
//! 上下移動の「希望桁」の保持に使う。描画上の正確な x 座標は UI 層が
//! DirectWrite で求めるが、論理的な桁はこのモジュールで OS 非依存に扱う。

use unicode_width::UnicodeWidthChar;

pub fn char_width(c: char, col: usize, tab_width: usize) -> usize {
    if c == '\t' {
        let tw = tab_width.max(1);
        tw - col % tw
    } else {
        c.width().unwrap_or(0)
    }
}

/// 行頭から `char_col` 文字目までの表示桁。
pub fn display_col(chars: impl Iterator<Item = char>, char_col: usize, tab_width: usize) -> usize {
    let mut col = 0;
    for c in chars.take(char_col) {
        col += char_width(c, col, tab_width);
    }
    col
}

/// 表示桁 `target` に最も近い (超えない) 文字位置。
pub fn char_col_at_display(chars: impl Iterator<Item = char>, target: usize, tab_width: usize) -> usize {
    let mut col = 0;
    let mut idx = 0;
    for c in chars {
        let w = char_width(c, col, tab_width);
        if col + w > target {
            // 文字の中央より右なら次の文字へ
            if target - col > w / 2 && w > 1 {
                idx += 1;
            }
            return idx;
        }
        col += w;
        idx += 1;
    }
    idx
}

/// 単語移動用の文字種。同じ種類が続く範囲を 1 単語とみなす。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CharClass {
    Space,
    Newline,
    Word,
    Hiragana,
    Katakana,
    Kanji,
    Punct,
}

pub fn char_class(c: char) -> CharClass {
    match c {
        '\n' => CharClass::Newline,
        c if c.is_whitespace() => CharClass::Space,
        c if c.is_alphanumeric() && (c.is_ascii() || is_fullwidth_alnum(c)) || c == '_' => CharClass::Word,
        '\u{3040}'..='\u{309F}' => CharClass::Hiragana,
        '\u{30A0}'..='\u{30FF}' | '\u{FF66}'..='\u{FF9F}' => CharClass::Katakana,
        '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}' | '々' | '〆' => CharClass::Kanji,
        c if c.is_alphanumeric() => CharClass::Word,
        _ => CharClass::Punct,
    }
}

fn is_fullwidth_alnum(c: char) -> bool {
    matches!(c, '\u{FF10}'..='\u{FF19}' | '\u{FF21}'..='\u{FF3A}' | '\u{FF41}'..='\u{FF5A}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_columns() {
        assert_eq!(display_col("a\tb".chars(), 2, 4), 4);
        assert_eq!(display_col("あいう".chars(), 2, 4), 4);
        assert_eq!(char_col_at_display("あいう".chars(), 4, 4), 2);
        assert_eq!(char_col_at_display("ab".chars(), 10, 4), 2);
    }
}
