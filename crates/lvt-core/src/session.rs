//! 起動をまたいで記憶する、開いていたタブ (セッション) と最近使ったファイルの一覧。
//!
//! どちらも 1 行 1 項目のテキストファイル (UTF-8) に保存する。ファイルの場所は UI 層が決める
//! (Windows では `%APPDATA%\lvt\session.txt` / `recent.txt`)。読めない行は無視する。
//! パスにタブ文字は使えない (Windows) ので、項目の区切りにタブを使う。

use std::path::{Path, PathBuf};

/// 最近使ったファイルの最大件数。
pub const RECENT_MAX: usize = 10;

/// 最近使ったファイル (新しい順)。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentFiles {
    paths: Vec<PathBuf>,
}

impl RecentFiles {
    pub fn load(path: &Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut r = Self::default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()).rev() {
            r.push(Path::new(line));
        }
        r
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text: String = self.paths.iter().map(|p| format!("{}\n", p.display())).collect();
        write_file(path, &text)
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// 先頭に追加する (既にあれば先頭へ移す)。最大件数を超えた古いものは捨てる。
    pub fn push(&mut self, path: &Path) {
        self.remove(path);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(RECENT_MAX);
    }

    pub fn remove(&mut self, path: &Path) {
        self.paths.retain(|p| !same_path(p, path));
    }

    pub fn clear(&mut self) {
        self.paths.clear();
    }
}

/// セッションの 1 タブ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTab {
    pub path: PathBuf,
    /// キャレットの文字位置。
    pub caret: usize,
    /// 先頭に表示している行。
    pub scroll_line: usize,
}

/// 終了時に開いていたタブ (ファイルのあるものだけ) と、アクティブなタブの位置。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub tabs: Vec<SessionTab>,
    pub active: usize,
}

impl Session {
    pub fn load(path: &Path) -> Self {
        Self::parse(&std::fs::read_to_string(path).unwrap_or_default())
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        write_file(path, &self.to_text())
    }

    /// 1 行目は `active<TAB>番号`、続く各行は `パス<TAB>キャレット<TAB>先頭の行`。
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let mut f = line.split('\t');
            let first = f.next().unwrap_or_default().trim();
            if first.is_empty() {
                continue;
            }
            let mut num = || f.next().and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
            if first == "active" {
                s.active = num();
            } else {
                let (caret, scroll_line) = (num(), num());
                s.tabs.push(SessionTab { path: PathBuf::from(first), caret, scroll_line });
            }
        }
        s.active = s.active.min(s.tabs.len().saturating_sub(1));
        s
    }

    pub fn to_text(&self) -> String {
        let mut s = format!("active\t{}\n", self.active);
        for t in &self.tabs {
            s.push_str(&format!("{}\t{}\t{}\n", t.path.display(), t.caret, t.scroll_line));
        }
        s
    }
}

/// 同じファイルか (Windows のパスは大文字・小文字を区別しない)。
pub fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

fn write_file(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_order_and_limit() {
        let mut r = RecentFiles::default();
        for i in 0..12 {
            r.push(Path::new(&format!("C:\\f{i}.txt")));
        }
        assert_eq!(r.paths().len(), RECENT_MAX);
        assert_eq!(r.paths()[0], Path::new("C:\\f11.txt"));
        // 既にあるもの (大文字・小文字の違いは同じとみなす) は先頭へ移す
        r.push(Path::new("c:\\F5.TXT"));
        assert_eq!(r.paths()[0], Path::new("c:\\F5.TXT"));
        assert_eq!(r.paths().len(), RECENT_MAX);
        assert_eq!(r.paths().iter().filter(|p| same_path(p, Path::new("C:\\f5.txt"))).count(), 1);
        r.remove(Path::new("C:\\f5.txt"));
        assert_eq!(r.paths()[0], Path::new("C:\\f11.txt"));
    }

    #[test]
    fn recent_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("lvt-recent-{}", std::process::id()));
        let file = dir.join("recent.txt");
        let mut r = RecentFiles::default();
        r.push(Path::new("C:\\a b\\x.txt"));
        r.push(Path::new("D:\\y.lvt"));
        r.save(&file).unwrap();
        assert_eq!(RecentFiles::load(&file), r);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_round_trip() {
        let s = Session {
            tabs: vec![
                SessionTab { path: PathBuf::from("C:\\a b\\x.txt"), caret: 12, scroll_line: 3 },
                SessionTab { path: PathBuf::from("D:\\日本語.lvt"), caret: 0, scroll_line: 0 },
            ],
            active: 1,
        };
        assert_eq!(Session::parse(&s.to_text()), s);
        // 壊れた行・範囲外の active
        let s = Session::parse("active\t9\nC:\\x.txt\tabc\n\n");
        assert_eq!(s.tabs, vec![SessionTab { path: PathBuf::from("C:\\x.txt"), caret: 0, scroll_line: 0 }]);
        assert_eq!(s.active, 0);
        assert_eq!(Session::parse(""), Session::default());
    }
}
