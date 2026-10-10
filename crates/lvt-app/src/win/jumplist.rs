//! タスクバーのジャンプリスト (右クリックの一覧) に「最近使ったファイル」を出す。
//!
//! 拡張子の関連付けがなくても使えるように、既定の「最近」の分類 (SHAddToRecentDocs) ではなく
//! 独自の分類に、`lvt.exe "パス"` を起動するショートカット (IShellLinkW) を並べる。
//! 起動した lvt は既存のウィンドウにパスを渡して終わる (mod.rs の単一ウィンドウの処理)。
//! 失敗しても編集には影響しないので、エラーは無視する。

use std::path::PathBuf;

use windows::core::{w, Interface, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW, SHStrDupW,
    SetCurrentProcessExplicitAppUserModelID, ShellLink,
};

/// タスクバーでウィンドウとジャンプリストをまとめる ID。
const APP_ID: PCWSTR = w!("lvt.editor");

/// `PKEY_Title` (ジャンプリストに表示する名前)。
const PKEY_TITLE: PROPERTYKEY =
    PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9), pid: 2 };

/// プロセスの AppUserModelID を設定する (ウィンドウを作る前に呼ぶ)。
pub fn set_app_id() {
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(APP_ID);
    }
}

/// ジャンプリストを `paths` (新しい順) で作り直す。
pub fn update(paths: &[PathBuf]) {
    let _ = try_update(paths);
}

fn try_update(paths: &[PathBuf]) -> Result<()> {
    let exe = std::env::current_exe().map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;
    unsafe {
        let list: ICustomDestinationList = CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
        list.SetAppID(APP_ID)?;
        let mut max_slots = 0u32;
        // ユーザーがジャンプリストから削除した項目は、追加すると失敗するので除く
        let removed: IObjectArray = list.BeginList(&mut max_slots)?;
        let removed_args = link_arguments(&removed);
        let items: IObjectCollection = CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
        let mut count = 0;
        for path in paths {
            let args = format!("\"{}\"", path.display());
            if removed_args.contains(&args) {
                continue;
            }
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&HSTRING::from(exe.as_os_str()))?;
            link.SetArguments(&HSTRING::from(args.as_str()))?;
            link.SetDescription(&HSTRING::from(path.display().to_string()))?;
            let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
            set_title(&link, &name)?;
            items.AddObject(&link)?;
            count += 1;
        }
        if count > 0 {
            list.AppendCategory(w!("最近使ったファイル"), &items.cast::<IObjectArray>()?)?;
        }
        list.CommitList()?;
    }
    Ok(())
}

/// ショートカットの表示名 (PKEY_Title)。独自の分類の項目には必須。
unsafe fn set_title(link: &IShellLinkW, title: &str) -> Result<()> {
    let store: IPropertyStore = link.cast()?;
    // 注意: windows crate の PROPVARIANT は Drop で PropVariantClear (= CoTaskMemFree) を呼ぶ。
    // 文字列は必ず CoTaskMemAlloc で確保したもの (SHStrDupW) を渡し、解放は Drop に任せる。
    // Rust の Vec などを指すと、Drop で解放されたうえに Vec の解放で二重解放になり、ヒープが壊れる。
    let value = PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_LPWSTR,
                Anonymous: PROPVARIANT_0_0_0 { pwszVal: SHStrDupW(&HSTRING::from(title))? },
                ..Default::default()
            }),
        },
    };
    store.SetValue(&PKEY_TITLE, &value)?;
    store.Commit()
}

/// 削除された項目 (ショートカット) の引数の一覧。
unsafe fn link_arguments(array: &IObjectArray) -> Vec<String> {
    let n = array.GetCount().unwrap_or(0);
    (0..n)
        .filter_map(|i| {
            let link: IShellLinkW = array.GetAt(i).ok()?;
            let mut buf = [0u16; 1024];
            link.GetArguments(&mut buf).ok()?;
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..len]))
        })
        .collect()
}
