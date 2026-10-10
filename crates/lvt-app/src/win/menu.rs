//! メニューバー。各項目は `Command` に対応付けられ、ショートカット表示は
//! 現在の入力モードのキー割り当てから生成する。

use std::path::PathBuf;

use lvt_core::encoding::{LineEnding, TextEncoding};
use lvt_core::keymap::InputMode;
use lvt_core::{AppCommand, Command, Document, EditCommand};
use windows::core::{Result, PCWSTR};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CheckMenuItem, CreateMenu, CreatePopupMenu, DeleteMenu, EnableMenuItem, GetMenuItemCount, HMENU,
    MF_BYCOMMAND, MF_BYPOSITION, MF_CHECKED, MF_ENABLED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED,
};

use super::util::wide;

const FIRST_ID: usize = 1000;
/// 「最近使ったファイル」の項目の ID (`RECENT_ID + 番号`) と、一覧のクリア。
const RECENT_ID: usize = 9000;
const RECENT_CLEAR_ID: usize = 8999;

/// 「最近使ったファイル」のサブメニューの操作。
pub enum RecentAction {
    Open(usize),
    Clear,
}

pub struct Menus {
    pub bar: HMENU,
    items: Vec<Command>,
    /// 「最近使ったファイル」のサブメニュー (開く直前に作り直す)。
    recent: HMENU,
}

impl Menus {
    pub fn build(mode: &dyn InputMode) -> Result<Self> {
        use AppCommand as A;
        use EditCommand as E;
        let mut m = Menus { bar: unsafe { CreateMenu()? }, items: Vec::new(), recent: HMENU::default() };

        let file = m.popup(m.bar, "ファイル(&F)")?;
        m.item(file, "新規(&N)", A::NewTab, mode)?;
        m.item(file, "開く(&O)...", A::Open, mode)?;
        m.recent = m.popup(file, "最近使ったファイル(&R)")?;
        m.item(file, "上書き保存(&S)", A::Save, mode)?;
        m.item(file, "名前を付けて保存(&A)...", A::SaveAs, mode)?;
        m.separator(file)?;
        m.item(file, "タブを閉じる(&C)", A::CloseTab, mode)?;
        m.separator(file)?;
        m.item(file, "終了(&X)", A::Exit, mode)?;

        let edit = m.popup(m.bar, "編集(&E)")?;
        m.item(edit, "元に戻す(&U)", E::Undo, mode)?;
        m.item(edit, "やり直し(&R)", E::Redo, mode)?;
        m.separator(edit)?;
        m.item(edit, "切り取り(&T)", A::Cut, mode)?;
        m.item(edit, "コピー(&C)", A::Copy, mode)?;
        m.item(edit, "貼り付け(&P)", A::Paste, mode)?;
        m.separator(edit)?;
        m.item(edit, "すべて選択(&A)", E::SelectAll, mode)?;
        m.item(edit, "行/選択範囲の複製(&D)", E::DuplicateLine, mode)?;

        let search = m.popup(m.bar, "検索(&S)")?;
        m.item(search, "検索(&F)...", A::Find, mode)?;
        m.item(search, "次を検索(&N)", A::FindNext, mode)?;
        m.item(search, "前を検索(&P)", A::FindPrev, mode)?;
        m.item(search, "置換(&R)...", A::Replace, mode)?;
        m.separator(search)?;
        m.item(search, "指定行へ移動(&G)...", A::GoToLine, mode)?;

        let view = m.popup(m.bar, "表示(&V)")?;
        m.item(view, "拡大(&I)", A::ZoomIn, mode)?;
        m.item(view, "縮小(&O)", A::ZoomOut, mode)?;
        m.item(view, "既定のサイズ(&R)", A::ZoomReset, mode)?;
        m.separator(view)?;
        m.item(view, "ダークテーマ(&D)", A::ToggleTheme, mode)?;
        m.item(view, "Vim キーバインド(&V)", A::ToggleVim, mode)?;
        m.separator(view)?;
        m.item(view, "詳細を編集(&E)...", A::EditDetail, mode)?;
        m.item(view, ".txt で装飾表示を使う(&T)", A::ToggleTxtDetailView, mode)?;

        let enc = m.popup(m.bar, "文字コード(&C)")?;
        let reopen = m.popup(enc, "指定した文字コードで開き直す(&R)")?;
        for e in TextEncoding::COMMON {
            m.item(reopen, e.name(), A::ReopenWithEncoding(e), mode)?;
        }
        let save = m.popup(enc, "保存時の文字コード(&S)")?;
        for e in TextEncoding::COMMON {
            m.item(save, e.name(), A::SetSaveEncoding(e), mode)?;
        }
        let eol = m.popup(enc, "改行コード(&L)")?;
        for e in LineEnding::ALL {
            m.item(eol, e.name(), A::SetLineEnding(e), mode)?;
        }
        Ok(m)
    }

    fn popup(&self, parent: HMENU, label: &str) -> Result<HMENU> {
        let sub = unsafe { CreatePopupMenu()? };
        let w = wide(label);
        unsafe { AppendMenuW(parent, MF_POPUP, sub.0 as usize, PCWSTR(w.as_ptr()))? };
        Ok(sub)
    }

    fn separator(&self, menu: HMENU) -> Result<()> {
        unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) }
    }

    fn item(&mut self, menu: HMENU, label: &str, cmd: impl Into<Command>, mode: &dyn InputMode) -> Result<()> {
        let cmd = cmd.into();
        let text = match mode.shortcut_for(&cmd) {
            Some(k) => format!("{label}\t{}", k.label()),
            None => label.to_string(),
        };
        let id = FIRST_ID + self.items.len();
        self.items.push(cmd);
        let w = wide(&text);
        unsafe { AppendMenuW(menu, MF_STRING, id, PCWSTR(w.as_ptr())) }
    }

    /// 「最近使ったファイル」のサブメニューを作り直す (番号 1〜9, 0 をアクセスキーにする)。
    pub fn update_recent(&self, paths: &[PathBuf]) {
        unsafe {
            while GetMenuItemCount(Some(self.recent)) > 0 {
                if DeleteMenu(self.recent, 0, MF_BYPOSITION).is_err() {
                    break;
                }
            }
            if paths.is_empty() {
                let w = wide("(なし)");
                let _ = AppendMenuW(self.recent, MF_STRING | MF_GRAYED, RECENT_ID, PCWSTR(w.as_ptr()));
                return;
            }
            for (i, p) in paths.iter().enumerate() {
                let key = if i < 9 { format!("&{}", i + 1) } else { "1&0".to_string() };
                // `&` はアクセスキーの記号なので、パスの中の `&` は `&&` にする
                let w = wide(&format!("{key}  {}", p.display().to_string().replace('&', "&&")));
                let _ = AppendMenuW(self.recent, MF_STRING, RECENT_ID + i, PCWSTR(w.as_ptr()));
            }
            let _ = AppendMenuW(self.recent, MF_SEPARATOR, 0, PCWSTR::null());
            let w = wide("一覧をクリア(&C)");
            let _ = AppendMenuW(self.recent, MF_STRING, RECENT_CLEAR_ID, PCWSTR(w.as_ptr()));
        }
    }

    pub fn recent_action(id: u16) -> Option<RecentAction> {
        match id as usize {
            RECENT_CLEAR_ID => Some(RecentAction::Clear),
            i if (RECENT_ID..RECENT_ID + lvt_core::session::RECENT_MAX).contains(&i) => Some(RecentAction::Open(i - RECENT_ID)),
            _ => None,
        }
    }

    pub fn command(&self, id: u16) -> Option<Command> {
        (id as usize).checked_sub(FIRST_ID).and_then(|i| self.items.get(i)).cloned()
    }

    /// メニューを開く直前にチェック状態・有効状態を更新する。
    pub fn update(&self, doc: &Document, dark: bool, vim: bool, txt_detail: bool) {
        for (i, cmd) in self.items.iter().enumerate() {
            let id = (FIRST_ID + i) as u32;
            let checked = match cmd {
                Command::App(AppCommand::SetSaveEncoding(e)) => Some(*e == doc.encoding),
                Command::App(AppCommand::SetLineEnding(e)) => Some(*e == doc.line_ending),
                Command::App(AppCommand::ToggleTheme) => Some(dark),
                Command::App(AppCommand::ToggleVim) => Some(vim),
                Command::App(AppCommand::ToggleTxtDetailView) => Some(txt_detail),
                _ => None,
            };
            let enabled = match cmd {
                Command::Edit(EditCommand::Undo) => Some(doc.can_undo()),
                Command::Edit(EditCommand::Redo) => Some(doc.can_redo()),
                Command::App(AppCommand::Cut | AppCommand::Copy) => Some(!doc.selection().is_empty()),
                Command::App(AppCommand::ReopenWithEncoding(_)) => Some(doc.path().is_some()),
                Command::App(AppCommand::EditDetail) => Some(doc.detail_view()),
                _ => None,
            };
            unsafe {
                if let Some(c) = checked {
                    CheckMenuItem(self.bar, id, (MF_BYCOMMAND | if c { MF_CHECKED } else { MF_UNCHECKED }).0);
                }
                if let Some(e) = enabled {
                    let _ = EnableMenuItem(self.bar, id, MF_BYCOMMAND | if e { MF_ENABLED } else { MF_GRAYED });
                }
            }
        }
    }
}
