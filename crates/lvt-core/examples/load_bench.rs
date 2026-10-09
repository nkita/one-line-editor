//! ファイル読み込み・検索・保存の速度を測る簡易ベンチマーク。
//!
//! cargo run -p lvt-core --release --example load_bench -- <ファイル> [検索文字列]

use std::time::Instant;

use lvt_core::search::{self, SearchQuery};
use lvt_core::Document;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: load_bench <file> [pattern]");
    let pattern = args.next().unwrap_or_else(|| "zzz_not_found".into());

    let t = Instant::now();
    let (doc, _) = Document::open(path.as_ref(), None).expect("open failed");
    println!(
        "load:   {:>8.3}s  {} lines, {} bytes, {}",
        t.elapsed().as_secs_f64(),
        doc.buffer().len_lines(),
        doc.buffer().len_bytes(),
        doc.encoding.name()
    );

    let re = SearchQuery { pattern, regex: false, case_sensitive: true }.compile().unwrap();
    let t = Instant::now();
    let found = search::find_next(doc.buffer(), &re, 0, true);
    println!("search: {:>8.3}s  {:?}", t.elapsed().as_secs_f64(), found);

    #[cfg(target_os = "linux")]
    if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
        for l in s.lines().filter(|l| l.starts_with("VmHWM") || l.starts_with("VmRSS")) {
            println!("{l}");
        }
    }
}
