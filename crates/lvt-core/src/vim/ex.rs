//! `:` コマンドライン (Ex コマンド) の解析と、`:s` の置換。
//!
//! 対応: `:w` `:q` `:q!` `:wq` `:x` `:{行}` `:[範囲]s/pat/rep/[gi]` `:noh`。
//! パターンは Vim の magic 形式 (`\(` `\|` `\+` `\<` など) を Rust の regex 形式に変換する。
//! `\v` で始まるパターンは Rust の regex の構文としてそのまま使う。

use std::ops::Range;

use regex::Regex;

use crate::buffer::TextBuffer;

/// 行の指定 (0 始まりへの変換は `resolve` で行う)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineSpec {
    /// 1 始まりの行番号。
    Number(usize),
    Current,
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExRange {
    Current,
    All,
    Lines(LineSpec, LineSpec),
    /// `'<,'>` (ビジュアルモードで選択していた行)。
    Visual,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExCommand {
    Write,
    Quit { force: bool },
    WriteQuit,
    GotoLine(LineSpec),
    Substitute { range: ExRange, pattern: String, replacement: String, global: bool, ignore_case: bool },
    NoHighlight,
}

pub fn parse(input: &str) -> Result<ExCommand, String> {
    let s = input.trim().trim_start_matches(':').trim_start();
    let (range, rest) = parse_range(s)?;
    let rest = rest.trim_start();
    if rest.is_empty() {
        return match range {
            Some(ExRange::Lines(_, to)) => Ok(ExCommand::GotoLine(to)),
            None => Err(String::new()),
            _ => Err("行番号を指定してください".into()),
        };
    }
    let name_len = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
    let (name, args) = rest.split_at(name_len);
    let (bang, args) = match args.strip_prefix('!') {
        Some(a) => (true, a),
        None => (false, args),
    };
    let no_args = |cmd: ExCommand| if args.trim().is_empty() { Ok(cmd) } else { Err(format!("引数は指定できません: {input}")) };
    match name {
        "w" | "write" => no_args(ExCommand::Write),
        "q" | "quit" | "clo" | "close" => no_args(ExCommand::Quit { force: bang }),
        "wq" | "x" | "xit" | "exit" => no_args(ExCommand::WriteQuit),
        "noh" | "nohlsearch" => no_args(ExCommand::NoHighlight),
        "s" | "substitute" => {
            let mut chars = args.chars();
            let delim = chars.next().ok_or("使い方: :s/検索/置換/[g]")?;
            if delim.is_alphanumeric() || delim == '\\' || delim == ' ' {
                return Err("区切り文字が不正です".into());
            }
            let parts = split_delimited(chars.as_str(), delim);
            let pattern = parts.first().cloned().unwrap_or_default();
            if pattern.is_empty() {
                return Err("検索パターンが空です".into());
            }
            let replacement = parts.get(1).cloned().unwrap_or_default();
            let flags = parts.get(2).cloned().unwrap_or_default();
            if let Some(bad) = flags.chars().find(|c| !matches!(c, 'g' | 'i' | 'I' | 'c' | 'e')) {
                return Err(format!("未対応のフラグです: {bad}"));
            }
            Ok(ExCommand::Substitute {
                range: range.unwrap_or(ExRange::Current),
                pattern,
                replacement,
                global: flags.contains('g'),
                ignore_case: flags.contains('i'),
            })
        }
        _ => Err(format!("未対応のコマンドです: {name}")),
    }
}

fn parse_range(s: &str) -> Result<(Option<ExRange>, &str), String> {
    if let Some(rest) = s.strip_prefix('%') {
        return Ok((Some(ExRange::All), rest));
    }
    if let Some(rest) = s.strip_prefix("'<,'>") {
        return Ok((Some(ExRange::Visual), rest));
    }
    let Some((from, rest)) = parse_line(s) else { return Ok((None, s)) };
    match rest.strip_prefix(',') {
        Some(r) => {
            let (to, rest) = parse_line(r).ok_or("範囲の指定が不正です")?;
            Ok((Some(ExRange::Lines(from, to)), rest))
        }
        None => Ok((Some(ExRange::Lines(from, from)), rest)),
    }
}

fn parse_line(s: &str) -> Option<(LineSpec, &str)> {
    if let Some(r) = s.strip_prefix('.') {
        return Some((LineSpec::Current, r));
    }
    if let Some(r) = s.strip_prefix('$') {
        return Some((LineSpec::Last, r));
    }
    let n = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let num = s[..n].parse().ok()?;
    Some((LineSpec::Number(num), &s[n..]))
}

/// 区切り文字で分割する (`\` + 区切り文字は区切り文字そのもの)。
fn split_delimited(s: &str, delim: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(n) if n == delim => parts.last_mut().unwrap().push(n),
                Some(n) => {
                    let last = parts.last_mut().unwrap();
                    last.push('\\');
                    last.push(n);
                }
                None => parts.last_mut().unwrap().push('\\'),
            }
        } else if c == delim && parts.len() < 3 {
            parts.push(String::new());
        } else {
            parts.last_mut().unwrap().push(c);
        }
    }
    parts
}

impl LineSpec {
    /// 0 始まりの行番号。
    pub fn resolve(self, current: usize, len_lines: usize) -> usize {
        match self {
            LineSpec::Number(n) => n.max(1) - 1,
            LineSpec::Current => current,
            LineSpec::Last => len_lines - 1,
        }
        .min(len_lines - 1)
    }
}

/// Vim の magic 形式のパターン → Rust の regex。
pub fn translate_pattern(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("\\v") {
        return rest.to_string();
    }
    let mut out = String::new();
    let mut chars = p.chars();
    let mut in_brace = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(n @ ('(' | ')' | '|' | '+' | '?')) => out.push(n),
                Some('=') => out.push('?'),
                Some('{') => {
                    out.push('{');
                    in_brace = true;
                }
                Some('}') if in_brace => {
                    out.push('}');
                    in_brace = false;
                }
                Some('<' | '>') => out.push_str("\\b"),
                Some('/') => out.push('/'),
                Some(n) => {
                    out.push('\\');
                    out.push(n);
                }
                None => out.push_str("\\\\"),
            },
            '}' if in_brace => {
                out.push('}');
                in_brace = false;
            }
            '(' | ')' | '|' | '+' | '?' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Vim の置換文字列 (`&`, `\1`, `\n`) → Rust の regex の置換文字列。
pub fn translate_replacement(r: &str) -> String {
    let mut out = String::new();
    let mut chars = r.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(d @ '0'..='9') => out.push_str(&format!("${{{d}}}")),
                Some('n' | 'r') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('$') => out.push_str("$$"),
                Some(n) => out.push(n),
                None => out.push('\\'),
            },
            '&' => out.push_str("${0}"),
            '$' => out.push_str("$$"),
            _ => out.push(c),
        }
    }
    out
}

pub fn compile(pattern: &str, ignore_case: bool) -> Result<Regex, String> {
    regex::RegexBuilder::new(&translate_pattern(pattern))
        .case_insensitive(ignore_case)
        .build()
        .map_err(|e| format!("正規表現エラー: {}", e.to_string().lines().last().unwrap_or_default()))
}

/// `lines` (0 始まり、終端を含まない) の各行で置換する編集の一覧。`global` でなければ各行の最初の一致だけ。
pub fn substitute(buf: &TextBuffer, lines: Range<usize>, re: &Regex, replacement: &str, global: bool) -> Vec<(Range<usize>, String)> {
    let mut edits = Vec::new();
    let mut text = String::new();
    for l in lines {
        text.clear();
        for chunk in buf.line(l).chunks() {
            text.push_str(chunk);
        }
        let base = buf.line_to_char(l);
        let mut last_byte = 0;
        let mut last_char = 0;
        for caps in re.captures_iter(&text) {
            let m = caps.get(0).expect("group 0");
            let start = last_char + text[last_byte..m.start()].chars().count();
            let end = start + m.as_str().chars().count();
            last_byte = m.end();
            last_char = end;
            let mut dst = String::new();
            caps.expand(replacement, &mut dst);
            edits.push((base + start..base + end, dst));
            if !global {
                break;
            }
        }
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        assert_eq!(parse(":w"), Ok(ExCommand::Write));
        assert_eq!(parse("q!"), Ok(ExCommand::Quit { force: true }));
        assert_eq!(parse("wq"), Ok(ExCommand::WriteQuit));
        assert_eq!(parse("x"), Ok(ExCommand::WriteQuit));
        assert_eq!(parse("42"), Ok(ExCommand::GotoLine(LineSpec::Number(42))));
        assert_eq!(parse("$"), Ok(ExCommand::GotoLine(LineSpec::Last)));
        assert_eq!(parse("noh"), Ok(ExCommand::NoHighlight));
        assert!(parse("foo").is_err());
        assert!(parse("w file.txt").is_err());
    }

    #[test]
    fn substitute_commands() {
        assert_eq!(
            parse("%s/a\\/b/c/g"),
            Ok(ExCommand::Substitute {
                range: ExRange::All,
                pattern: "a/b".into(),
                replacement: "c".into(),
                global: true,
                ignore_case: false
            })
        );
        assert_eq!(
            parse("'<,'>s#x#y#i"),
            Ok(ExCommand::Substitute {
                range: ExRange::Visual,
                pattern: "x".into(),
                replacement: "y".into(),
                global: false,
                ignore_case: true
            })
        );
        assert!(matches!(parse("2,$s/a/b"), Ok(ExCommand::Substitute { range: ExRange::Lines(LineSpec::Number(2), LineSpec::Last), .. })));
        assert!(matches!(parse("s/a/"), Ok(ExCommand::Substitute { range: ExRange::Current, .. })));
        assert!(parse("s//b/").is_err());
    }

    #[test]
    fn patterns() {
        assert_eq!(translate_pattern(r"\(foo\|bar\)\+"), "(foo|bar)+");
        assert_eq!(translate_pattern(r"a(b)c+"), r"a\(b\)c\+");
        assert_eq!(translate_pattern(r"\<word\>"), r"\bword\b");
        assert_eq!(translate_pattern(r"a\{2,3}"), "a{2,3}");
        assert_eq!(translate_pattern(r"\d\+\.\d"), r"\d+\.\d");
        assert_eq!(translate_pattern(r"\v(a|b)+"), "(a|b)+");
        assert_eq!(translate_replacement(r"[\1]&$"), "[${1}]${0}$$");
    }

    #[test]
    fn substitute_lines() {
        let buf = TextBuffer::from_str("aa あa\naa\naa");
        let re = compile("a", false).unwrap();
        let edits = substitute(&buf, 0..2, &re, &translate_replacement("<&>"), false);
        assert_eq!(edits, vec![(0..1, "<a>".to_string()), (6..7, "<a>".to_string())]);
        let edits = substitute(&buf, 0..1, &re, "b", true);
        assert_eq!(edits.iter().map(|e| e.0.clone()).collect::<Vec<_>>(), vec![0..1, 1..2, 4..5]);
    }
}
