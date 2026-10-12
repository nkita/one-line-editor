//! Windows 向けビルドで、アプリケーション マニフェスト (res/lvt.manifest)・アイコン (res/*.ico)・
//! バージョン情報 (Cargo.toml の version から生成する version.rc) を exe に埋め込む。
//! MSVC ではマニフェストだけを埋め込む (アイコンとバージョン情報は GNU ターゲットのみ)。

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=res/lvt.manifest");
    println!("cargo:rerun-if-changed=res/lvt.rc");
    println!("cargo:rerun-if-changed=res/lvt.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let res = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("res");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest = res.join("lvt.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
        return;
    }
    // GNU: windres でリソースをオブジェクトファイルにしてリンクする
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    std::fs::write(out_dir.join("version.rc"), version_rc()).expect("version.rc を書けません");
    let out = out_dir.join("lvt-res.o");
    let windres = if cfg!(windows) { "windres" } else { "x86_64-w64-mingw32-windres" };
    let status = Command::new(windres)
        .current_dir(&res)
        .arg("lvt.rc")
        .arg("-I")
        .arg(&out_dir)
        .args(["-O", "coff", "-o"])
        .arg(&out)
        .status()
        .unwrap_or_else(|e| panic!("{windres} を実行できません ({e})。mingw-w64 をインストールしてください"));
    assert!(status.success(), "{windres} が失敗しました");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

/// exe のプロパティ (詳細) に出るバージョン情報。lvt.rc から `#include "version.rc"` で読む。
fn version_rc() -> String {
    let ver = env::var("CARGO_PKG_VERSION").unwrap();
    let nums: Vec<u16> = ver.split(['.', '-']).take(3).map(|n| n.parse().unwrap_or(0)).collect();
    let (a, b, c) = (nums[0], nums.get(1).copied().unwrap_or(0), nums.get(2).copied().unwrap_or(0));
    format!(
        r#"1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "041104B0"
    BEGIN
      VALUE "CompanyName", "nkita"
      VALUE "FileDescription", "lvt - lightweight text editor"
      VALUE "FileVersion", "{ver}"
      VALUE "InternalName", "lvt"
      VALUE "LegalCopyright", "MIT License"
      VALUE "OriginalFilename", "lvt.exe"
      VALUE "ProductName", "lvt"
      VALUE "ProductVersion", "{ver}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x411, 1200
  END
END
"#
    )
}
