//! Win32 の仮想キーコード → `lvt_core::keymap::Key` の変換。

use lvt_core::keymap::{Key, Modifiers};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, MapVirtualKeyW, MAPVK_VK_TO_CHAR, VK_CONTROL, VK_MENU, VK_SHIFT};

pub fn current_modifiers() -> Modifiers {
    let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
    Modifiers { ctrl: down(VK_CONTROL.0), shift: down(VK_SHIFT.0), alt: down(VK_MENU.0) }
}

pub fn key_from_vk(vk: u16) -> Option<Key> {
    Some(match vk {
        0x08 => Key::Backspace,
        0x09 => Key::Tab,
        0x0D => Key::Enter,
        0x1B => Key::Escape,
        0x20 => Key::Space,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        0x23 => Key::End,
        0x24 => Key::Home,
        0x25 => Key::Left,
        0x26 => Key::Up,
        0x27 => Key::Right,
        0x28 => Key::Down,
        0x2D => Key::Insert,
        0x2E => Key::Delete,
        0x30..=0x39 => Key::Char(vk as u8 as char),
        0x41..=0x5A => Key::Char((vk as u8 as char).to_ascii_lowercase()),
        0x60..=0x69 => Key::Char((b'0' + (vk - 0x60) as u8) as char),
        0x6A => Key::Char('*'),
        0x6B => Key::Char('+'),
        0x6D => Key::Char('-'),
        0x6E => Key::Char('.'),
        0x6F => Key::Char('/'),
        0x70..=0x87 => Key::F((vk - 0x70 + 1) as u8),
        // 記号キーは配列 (US/JIS) により異なるので、OS に無修飾時の文字を問い合わせる
        0xBA..=0xC0 | 0xDB..=0xDF | 0xE2 => {
            let c = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_CHAR) } & 0x7FFF;
            Key::Char(char::from_u32(c).filter(|c| !c.is_control())?.to_ascii_lowercase())
        }
        _ => return None,
    })
}

/// 文字を生むキー (修飾なしまたは Shift のみの場合は WM_CHAR 側で処理する)。
pub fn is_text_key(key: Key) -> bool {
    matches!(key, Key::Char(_) | Key::Space)
}
