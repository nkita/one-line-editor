//! アプリが記憶する設定値 (入力モードなど) の読み書き。
//!
//! 形式は TOML のサブセット (`キー = "文字列"` / 真偽値 / 整数、`#` コメント) で、
//! 将来の設定ファイル (docs/design/extending.md §G) で本物の TOML パーサに置き換えても
//! そのまま読めるようにしてある。知らないキーは保存時にもそのまま残す。
//! ファイルの場所は UI 層が決める (Windows では `%APPDATA%\lvt\settings.toml`)。

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    values: BTreeMap<String, String>,
}

impl Settings {
    /// ファイルを読む。存在しない・読めない場合は空の設定を返す (起動を止めない)。
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).map(|s| Self::parse(&s)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_text())
    }

    pub fn parse(text: &str) -> Self {
        let mut values = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            let value = match value.strip_prefix('"').and_then(|v| v.split_once('"')) {
                Some((s, _)) => s.replace("\\\\", "\\"),
                None => value.split('#').next().unwrap_or_default().trim().to_string(),
            };
            values.insert(key.trim().to_string(), value);
        }
        Self { values }
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("# lvt が自動で保存する設定です。\n");
        for (k, v) in &self.values {
            let quoted = v.parse::<i64>().is_err() && v != "true" && v != "false";
            if quoted {
                s.push_str(&format!("{k} = \"{}\"\n", v.replace('\\', "\\\\").replace('"', "")));
            } else {
                s.push_str(&format!("{k} = {v}\n"));
            }
        }
        s
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.values.insert(key.to_string(), value.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut s = Settings::parse("# comment\ninput_mode = \"vim\"\nsize = 15 # 文字サイズ\nunknown = true\n");
        assert_eq!(s.get("input_mode"), Some("vim"));
        assert_eq!(s.get("size"), Some("15"));
        s.set("input_mode", "standard");
        let again = Settings::parse(&s.to_text());
        assert_eq!(again, s);
        assert_eq!(again.get("unknown"), Some("true"));
        assert_eq!(again.get("input_mode"), Some("standard"));
    }

    #[test]
    fn broken_lines_are_ignored() {
        let s = Settings::parse("garbage\n[section]\n= x\nk = \"unterminated\n");
        assert_eq!(s.get("k"), Some("\"unterminated"));
    }

    #[test]
    fn missing_file_is_empty() {
        assert_eq!(Settings::load(Path::new("/nonexistent/lvt/settings.toml")), Settings::default());
    }
}
