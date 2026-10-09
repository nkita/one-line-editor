//! 行内の URL の検出 (自動リンク)。
//!
//! 表示行ごとに呼ばれるので正規表現は使わず、1 回の走査で済ませる。
//! URL は ASCII の空白以外の文字が続く範囲とし、非 ASCII 文字 (日本語など) で終わる。
//! 日本語の文章では URL の直後に「。」などが空白なしで続くことが多いため。

use std::ops::Range;

const SCHEMES: [&str; 3] = ["https://", "http://", "www."];

/// `line` に含まれる URL のバイト範囲を先頭から順に返す。
pub fn find_urls(line: &str) -> Vec<Range<usize>> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let Some(scheme) = SCHEMES.iter().find(|s| starts_with_ignore_case(&bytes[i..], s.as_bytes())) else {
            i += 1;
            continue;
        };
        // 単語の途中 (例: "xhttp://") からは始めない
        if i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'.') {
            i += 1;
            continue;
        }
        let end = i + bytes[i..].iter().take_while(|&&b| is_url_byte(b)).count();
        let end = trim_trailing(&line[i..end]) + i;
        if end > i + scheme.len() {
            out.push(i..end);
            i = end;
        } else {
            i += scheme.len();
        }
    }
    out
}

/// URL を開くときの文字列 (`www.` で始まるものには `https://` を補う)。
pub fn normalize(url: &str) -> String {
    if url.len() >= 4 && url[..4].eq_ignore_ascii_case("www.") {
        format!("https://{url}")
    } else {
        url.to_string()
    }
}

/// 行内のバイト位置 `byte` を含む URL。
pub fn url_at(line: &str, byte: usize) -> Option<Range<usize>> {
    find_urls(line).into_iter().find(|r| r.contains(&byte))
}

fn starts_with_ignore_case(s: &[u8], prefix: &[u8]) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn is_url_byte(b: u8) -> bool {
    b.is_ascii_graphic() && !matches!(b, b'<' | b'>' | b'"' | b'`' | b'{' | b'}' | b'|' | b'\\' | b'^')
}

/// 末尾の句読点と、対応しない閉じ括弧を除いた長さ。
fn trim_trailing(s: &str) -> usize {
    let b = s.as_bytes();
    let mut end = b.len();
    while end > 0 {
        let c = b[end - 1];
        let unbalanced = |open: u8, close: u8| {
            c == close && b[..end].iter().filter(|&&x| x == close).count() > b[..end].iter().filter(|&&x| x == open).count()
        };
        if matches!(c, b'.' | b',' | b';' | b':' | b'!' | b'?' | b'\'' | b'*' | b'_')
            || unbalanced(b'(', b')')
            || unbalanced(b'[', b']')
        {
            end -= 1;
        } else {
            break;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(s: &str) -> Vec<&str> {
        find_urls(s).into_iter().map(|r| &s[r]).collect()
    }

    #[test]
    fn basic() {
        assert_eq!(urls("see https://example.com/a?b=1#c now"), ["https://example.com/a?b=1#c"]);
        assert_eq!(urls("http://a.jp and www.b.com"), ["http://a.jp", "www.b.com"]);
        assert_eq!(urls("HTTPS://X.COM"), ["HTTPS://X.COM"]);
        assert!(urls("no links here").is_empty());
        assert!(urls("https://").is_empty());
        assert!(urls("xhttp://a.com").is_empty());
    }

    #[test]
    fn trailing_punctuation() {
        assert_eq!(urls("Go to https://a.com/x."), ["https://a.com/x"]);
        assert_eq!(urls("(https://a.com/x)"), ["https://a.com/x"]);
        assert_eq!(urls("https://en.wikipedia.org/wiki/Rust_(language)"), ["https://en.wikipedia.org/wiki/Rust_(language)"]);
        assert_eq!(urls("[link](https://a.com)"), ["https://a.com"]);
        assert_eq!(urls("<https://a.com>"), ["https://a.com"]);
    }

    #[test]
    fn japanese() {
        let s = "詳しくはhttps://example.com/docを参照。";
        assert_eq!(urls(s), ["https://example.com/doc"]);
        assert_eq!(urls("「https://a.com」"), ["https://a.com"]);
    }

    #[test]
    fn helpers() {
        assert_eq!(normalize("www.a.com"), "https://www.a.com");
        assert_eq!(normalize("http://a.com"), "http://a.com");
        let s = "a https://b.com c";
        assert_eq!(url_at(s, 5), Some(2..15));
        assert_eq!(url_at(s, 0), None);
    }
}
