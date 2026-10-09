//! キー入力から文書の変化までを通しで確かめるテスト。

use super::*;
use crate::document::Document;
use crate::editor::{self, EditorConfig, ViewInfo};
use crate::keymap::{Keymap, Modifiers};
use crate::selection::Selection;

/// UI 層の代わり: コマンドを実行し、ヤンクした文字列を保持して p で貼り付ける。
struct Harness {
    mode: VimMode,
    doc: Document,
    clip: Option<Register>,
}

impl Harness {
    fn new(text: &str, caret: usize) -> Self {
        let mut doc = Document::from_text(text);
        doc.set_selection(Selection::caret(caret));
        Self { mode: VimMode::new(Keymap::windows_default()), doc, clip: None }
    }

    /// `<Esc>` `<CR>` `<BS>` `<C-x>` 以外の文字は文字入力として送る。
    fn keys(&mut self, s: &str) -> &mut Self {
        let mut rest = s;
        while !rest.is_empty() {
            let (resp, len) = if let Some(end) = rest.strip_prefix('<').and_then(|r| r.find('>')).filter(|_| rest.len() > 2) {
                let name = &rest[1..end + 1];
                let mut mods = Modifiers::default();
                let key = match name {
                    "Esc" => Key::Escape,
                    "CR" => Key::Enter,
                    "BS" => Key::Backspace,
                    "Tab" => Key::Tab,
                    n if n.starts_with("C-") => {
                        mods.ctrl = true;
                        Key::Char(n.chars().nth(2).unwrap())
                    }
                    _ => panic!("unknown key {name}"),
                };
                (self.mode.on_key(KeyChord::new(key, mods)), end + 2)
            } else {
                let c = rest.chars().next().unwrap();
                (self.mode.on_text(&c.to_string()), c.len_utf8())
            };
            self.run(resp);
            rest = &rest[len..];
        }
        self
    }

    fn run(&mut self, resp: ModeResponse) {
        let ModeResponse::Commands(cmds) = resp else { return };
        let cfg = EditorConfig::default();
        let view = ViewInfo { page_lines: 10, top_line: 0 };
        for cmd in cmds {
            let edit = match cmd {
                Command::Edit(e) => e,
                Command::App(AppCommand::VimPaste { before, count }) => match self.clip.clone() {
                    Some(r) => EditCommand::Vim(VimEdit::Paste { text: r.text, linewise: r.linewise, before, count }),
                    None => continue,
                },
                Command::App(_) => continue,
            };
            if let Some(r) = editor::execute(&mut self.doc, &edit, &cfg, &view) {
                self.clip = Some(r);
            }
        }
        if self.mode.normal_caret() {
            normalize_caret(&mut self.doc);
        }
    }

    fn text(&self) -> String {
        self.doc.buffer().to_string()
    }

    fn caret(&self) -> usize {
        self.doc.caret_pos()
    }

    fn check(&self, text: &str, caret: usize) {
        assert_eq!((self.text().as_str(), self.caret()), (text, caret));
    }
}

#[test]
fn motions() {
    let mut h = Harness::new("foo bar baz\n  qux\nend", 0);
    h.keys("w").check("foo bar baz\n  qux\nend", 4);
    h.keys("2w").check("foo bar baz\n  qux\nend", 14);
    h.keys("0").check("foo bar baz\n  qux\nend", 12);
    h.keys("^").check("foo bar baz\n  qux\nend", 14);
    h.keys("$").check("foo bar baz\n  qux\nend", 16);
    h.keys("gg").check("foo bar baz\n  qux\nend", 0);
    h.keys("G").check("foo bar baz\n  qux\nend", 18);
    h.keys("2G").check("foo bar baz\n  qux\nend", 14);
    h.keys("k$j").check("foo bar baz\n  qux\nend", 16); // $ の後の上下移動は行末を保つ
    h.keys("gg").keys("fa").check("foo bar baz\n  qux\nend", 5);
    h.keys(";").check("foo bar baz\n  qux\nend", 9);
    h.keys(",").check("foo bar baz\n  qux\nend", 5);
    h.keys("0ta").check("foo bar baz\n  qux\nend", 4);
    h.keys(";").check("foo bar baz\n  qux\nend", 8);
    h.keys("$l").check("foo bar baz\n  qux\nend", 10); // 行末より右へは行かない
}

#[test]
fn delete_and_put() {
    let mut h = Harness::new("foo bar baz", 0);
    h.keys("dw").check("bar baz", 0);
    h.keys("P").check("foo bar baz", 3);
    h.keys("$x").check("foo bar ba", 9);
    h.keys("0de").check(" bar ba", 0);
    h.keys("D").check("", 0);
    h.keys("u").check(" bar ba", 0);
    h.keys("u").check("foo bar ba", 0);
    h.keys("wdw").check("foo ba", 4);
    h.keys("$dw").check("foo b", 4); // 行の最後の単語では改行を消さない
}

#[test]
fn line_operations() {
    let mut h = Harness::new("one\ntwo\nthree", 0);
    h.keys("dd").check("two\nthree", 0);
    h.keys("p").check("two\none\nthree", 4);
    h.keys("yyP").check("two\none\none\nthree", 4);
    h.keys("G2dd").check("two\none\none", 8); // 最終行を含む削除
    h.keys("ggdj").check("one", 0);
    h.keys("p").check("one\ntwo\none", 4);
    h.keys("ggJ").check("one two\none", 3);
    h.keys(">>").check("\tone two\none", 1);
    h.keys("<<").check("one two\none", 0);
}

#[test]
fn change_and_insert() {
    let mut h = Harness::new("foo bar", 0);
    h.keys("cwhey<Esc>").check("hey bar", 2);
    h.keys("u").check("foo bar", 0); // 削除と入力が 1 回で戻る
    h.keys("wciwX<Esc>").check("foo X", 4);
    h.keys("Ais<Esc>").check("foo Xis", 6);
    h.keys("onew<Esc>").check("foo Xis\nnew", 10);
    h.keys("Otop<Esc>").check("foo Xis\ntop\nnew", 10);
    h.keys("ggIa<Esc>").check("afoo Xis\ntop\nnew", 0);
    h.keys("ccline<Esc>").check("line\ntop\nnew", 3);
    h.keys("ji<CR>x<Esc>").check("line\nto\nxp\nnew", 8); // j で行末を越えた桁は最後の文字に寄る
}

#[test]
fn dot_repeat() {
    let mut h = Harness::new("a b c d e", 0);
    h.keys("x").check(" b c d e", 0);
    h.keys("w.").check(" b c d e".replacen('b', "", 1).as_str(), 1);
    let mut h = Harness::new("foo foo foo", 0);
    h.keys("ciwbar<Esc>").check("bar foo foo", 2);
    h.keys("w.").check("bar bar foo", 6);
    h.keys("w.").check("bar bar bar", 10);
    let mut h = Harness::new("1\n2\n3\n4", 0);
    h.keys("dd2.").check("4", 0);
    let mut h = Harness::new("x", 0);
    h.keys("Ay<Esc>3.").check("xyyyy", 4);
}

#[test]
fn text_objects() {
    let mut h = Harness::new("call(a, (b), \"str\")", 9);
    h.keys("di(").check("call(a, (), \"str\")", 9);
    h.keys("2di(").check("call()", 5);
    let mut h = Harness::new("say \"hello\" now", 6);
    h.keys("da\"").check("say now", 4);
    let mut h = Harness::new("snake_case word", 2);
    h.keys("gUiw").check("SNAKE_CASE word", 0);
    h.keys("~").check("sNAKE_CASE word", 1);
    h.keys("wrX").check("sNAKE_CASE Xord", 11);
}

#[test]
fn visual_mode() {
    let mut h = Harness::new("abc def\nghi\njkl", 0);
    h.keys("vlld").check(" def\nghi\njkl", 0);
    h.keys("Vjd").check("jkl", 0);
    h.keys("u").check(" def\nghi\njkl", 0);
    h.keys("veyP").check(" def def\nghi\njkl", 3); // 選択は終端の文字を含む
    let mut h = Harness::new("one two", 4);
    h.keys("viwy").check("one two", 4);
    assert_eq!(h.clip.as_ref().unwrap().text, "two");
    h.keys("0viwp").check("two two", 0);
    let mut h = Harness::new("a\nb\nc", 0);
    h.keys("VjJ").check("a b\nc", 1);
    h.keys("Vj>").check("\ta b\n\tc", 1);
    h.keys("vj<Esc>").check("\ta b\n\tc", 6);
}

#[test]
fn counts_and_undo() {
    let mut h = Harness::new("a b c d e", 0);
    h.keys("3x").check(" c d e", 0);
    h.keys("w2dw").check(" e", 1);
    h.keys("u").check(" c d e", 1);
    h.keys("2u").check("a b c d e", 0);
    h.keys("<C-r>").check(" c d e", 0);
}

#[test]
fn escape_cancels_pending() {
    let mut h = Harness::new("abc", 1);
    h.keys("d<Esc>x").check("ac", 1);
    assert_eq!(h.mode.name(), "NORMAL");
    h.keys("2d");
    assert_eq!(h.mode.name(), "NORMAL  2d");
    assert_eq!(h.mode.caret_style(), CaretStyle::Underline);
}

#[test]
fn text_in_normal_mode_after_insert_key() {
    // IME の確定文字列が「iあ」のように届いた場合、i 以降は挿入する
    let mut h = Harness::new("x", 0);
    h.keys("iあい<Esc>").check("あいx", 1);
}

#[test]
fn standard_keys_in_normal_mode() {
    let mut h = Harness::new("abc", 0);
    h.keys("<Tab>").check("abc", 0); // ノーマルモードの Tab は文字を挿入しない
    h.keys("i<Tab><Esc>").check("\tabc", 0); // 挿入モードでは挿入する
    let resp = h.mode.on_key(KeyChord::parse("Ctrl+S").unwrap());
    assert!(matches!(resp, ModeResponse::Commands(c) if c == vec![Command::App(AppCommand::Save)]));
}
