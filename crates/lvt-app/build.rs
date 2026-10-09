//! Windows 向けビルドで、アプリケーション マニフェスト (res/lvt.manifest) を exe に埋め込む。

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=res/lvt.manifest");
    println!("cargo:rerun-if-changed=res/lvt.rc");
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
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("lvt-res.o");
    let windres = if cfg!(windows) { "windres" } else { "x86_64-w64-mingw32-windres" };
    let status = Command::new(windres)
        .current_dir(&res)
        .arg("lvt.rc")
        .args(["-O", "coff", "-o"])
        .arg(&out)
        .status()
        .unwrap_or_else(|e| panic!("{windres} を実行できません ({e})。mingw-w64 をインストールしてください"));
    assert!(status.success(), "{windres} が失敗しました");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}
