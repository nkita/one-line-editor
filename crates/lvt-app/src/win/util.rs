//! 文字列・座標変換などの小物。

use windows::Win32::Foundation::{LPARAM, WPARAM};

/// NUL 終端の UTF-16 文字列。
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// 行内の文字位置 → UTF-16 位置。
pub fn char_to_utf16(s: &str, col: usize) -> usize {
    s.chars().take(col).map(char::len_utf16).sum()
}

/// UTF-16 位置 → 行内の文字位置。
pub fn utf16_to_char(s: &str, pos: usize) -> usize {
    let mut u = 0;
    for (i, c) in s.chars().enumerate() {
        if u >= pos {
            return i;
        }
        u += c.len_utf16();
    }
    s.chars().count()
}

/// バイト位置 → UTF-16 位置 (`byte` は文字境界であること)。
pub fn byte_to_utf16(s: &str, byte: usize) -> usize {
    s[..byte.min(s.len())].encode_utf16().count()
}

pub fn loword(v: usize) -> u16 {
    (v & 0xFFFF) as u16
}

pub fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xFFFF) as u16
}

/// lParam のマウス座標 (符号付き)。
pub fn point_from_lparam(lp: LPARAM) -> (i32, i32) {
    let v = lp.0 as u32;
    ((v & 0xFFFF) as i16 as i32, (v >> 16) as i16 as i32)
}

pub fn wheel_delta(wp: WPARAM) -> i32 {
    hiword(wp.0) as i16 as i32
}
