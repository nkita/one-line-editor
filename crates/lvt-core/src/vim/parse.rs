//! ノーマル・ビジュアルモードの入力列の解析。
//!
//! 入力途中のキー列全体を毎回先頭から解析し、「まだ途中」「無効」「完成」のいずれかを返す。
//! 例: `2d3w` → 完成 (Operator{Delete, Motion(WordStart)}, count 6)、`d` → 途中。

use super::{InsertAt, OpTarget, Operator, TextObject, VimMotion};

/// 解析の単位。文字キーと、Vim が扱う Ctrl+文字。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    C(char),
    Ctrl(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Paste { before: bool },
    Undo,
    Redo,
    Join,
    Replace(char),
    Tilde,
    Repeat,
    Insert(InsertAt),
    Visual { linewise: bool },
    VisualSwap,
    CommandLine,
    Search { forward: bool },
    SearchNext { reverse: bool },
    SearchWord { forward: bool },
    Scroll(i32),
    /// ; と , (直前の f/t を繰り返す)
    RepeatFind { reverse: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Motion(VimMotion),
    Operator { op: Operator, target: OpTarget },
    /// ビジュアルモードでのテキストオブジェクト選択。
    Object { obj: TextObject, inner: bool },
    /// 演算子 + ; や , (FindChar に置き換えてから実行する)。
    OperatorRepeatFind { op: Operator, reverse: bool },
    Cmd(Cmd),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    pub count: Option<usize>,
    pub kind: Kind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Incomplete,
    Invalid,
    Done(Action),
}

enum Fail {
    Incomplete,
    Invalid,
}

struct P<'a> {
    toks: &'a [Tok],
    i: usize,
}

impl P<'_> {
    fn next(&mut self) -> Result<Tok, Fail> {
        let t = *self.toks.get(self.i).ok_or(Fail::Incomplete)?;
        self.i += 1;
        Ok(t)
    }

    fn char(&mut self) -> Result<char, Fail> {
        match self.next()? {
            Tok::C(c) => Ok(c),
            Tok::Ctrl(_) => Err(Fail::Invalid),
        }
    }

    fn count(&mut self) -> Option<usize> {
        let mut n: Option<usize> = None;
        while let Some(Tok::C(c @ '0'..='9')) = self.toks.get(self.i) {
            if *c == '0' && n.is_none() {
                break;
            }
            n = Some(n.unwrap_or(0).saturating_mul(10).saturating_add(c.to_digit(10).unwrap_or(0) as usize));
            self.i += 1;
        }
        n
    }
}

pub fn parse(toks: &[Tok], visual: bool) -> Parsed {
    let mut p = P { toks, i: 0 };
    match parse_inner(&mut p, visual) {
        Ok(a) => Parsed::Done(a),
        Err(Fail::Incomplete) => Parsed::Incomplete,
        Err(Fail::Invalid) => Parsed::Invalid,
    }
}

fn mul(a: Option<usize>, b: Option<usize>) -> Option<usize> {
    match (a, b) {
        (None, None) => None,
        _ => Some(a.unwrap_or(1).saturating_mul(b.unwrap_or(1))),
    }
}

fn parse_inner(p: &mut P, visual: bool) -> Result<Action, Fail> {
    let count = p.count();
    let t = p.next()?;
    let done = |kind| Ok(Action { count, kind });
    let cmd = |c| Ok(Action { count, kind: Kind::Cmd(c) });
    let op_on = |op, target| Ok(Action { count, kind: Kind::Operator { op, target } });

    if let Tok::Ctrl(c) = t {
        return match c {
            'r' => cmd(Cmd::Redo),
            'e' => cmd(Cmd::Scroll(1)),
            'y' => cmd(Cmd::Scroll(-1)),
            _ => match ctrl_motion(c) {
                Some(m) => done(Kind::Motion(m)),
                None => Err(Fail::Invalid),
            },
        };
    }
    let Tok::C(c) = t else { unreachable!() };

    if visual {
        let v = OpTarget::Visual;
        match c {
            'd' | 'x' => return op_on(Operator::Delete, v),
            'c' | 's' => return op_on(Operator::Change, v),
            'y' => return op_on(Operator::Yank, v),
            '>' => return op_on(Operator::Indent, v),
            '<' => return op_on(Operator::Outdent, v),
            '~' => return op_on(Operator::ToggleCase, v),
            'u' => return op_on(Operator::Lower, v),
            'U' => return op_on(Operator::Upper, v),
            // 行単位にしてから実行する (VimMode 側で Visual{linewise} を先に送る)
            'D' | 'X' => return op_on(Operator::Delete, OpTarget::VisualLines),
            'C' | 'S' | 'R' => return op_on(Operator::Change, OpTarget::VisualLines),
            'Y' => return op_on(Operator::Yank, OpTarget::VisualLines),
            'J' => return cmd(Cmd::Join),
            'p' | 'P' => return cmd(Cmd::Paste { before: true }),
            'o' | 'O' => return cmd(Cmd::VisualSwap),
            'v' => return cmd(Cmd::Visual { linewise: false }),
            'V' => return cmd(Cmd::Visual { linewise: true }),
            ':' => return cmd(Cmd::CommandLine),
            'i' | 'a' => {
                let obj = text_object(p.char()?).ok_or(Fail::Invalid)?;
                return done(Kind::Object { obj, inner: c == 'i' });
            }
            'g' => {
                return match p.char()? {
                    'g' => done(Kind::Motion(VimMotion::GotoLine { last: false })),
                    '~' => op_on(Operator::ToggleCase, v),
                    'u' => op_on(Operator::Lower, v),
                    'U' => op_on(Operator::Upper, v),
                    _ => Err(Fail::Invalid),
                }
            }
            _ => {}
        }
    } else {
        let op = match c {
            'd' => Some(Operator::Delete),
            'c' => Some(Operator::Change),
            'y' => Some(Operator::Yank),
            '>' => Some(Operator::Indent),
            '<' => Some(Operator::Outdent),
            'g' => {
                // gg はモーション、g~ / gu / gU は演算子
                match p.char()? {
                    'g' => return done(Kind::Motion(VimMotion::GotoLine { last: false })),
                    '~' => Some(Operator::ToggleCase),
                    'u' => Some(Operator::Lower),
                    'U' => Some(Operator::Upper),
                    _ => return Err(Fail::Invalid),
                }
            }
            _ => None,
        };
        if let Some(op) = op {
            return operator_target(p, op, c, count);
        }
        let simple = match c {
            'x' => Some((Operator::Delete, OpTarget::Motion(VimMotion::Right))),
            'X' => Some((Operator::Delete, OpTarget::Motion(VimMotion::Left))),
            's' => Some((Operator::Change, OpTarget::Motion(VimMotion::Right))),
            'S' => Some((Operator::Change, OpTarget::Lines)),
            'D' => Some((Operator::Delete, OpTarget::Motion(VimMotion::LineEnd))),
            'C' => Some((Operator::Change, OpTarget::Motion(VimMotion::LineEnd))),
            'Y' => Some((Operator::Yank, OpTarget::Lines)),
            _ => None,
        };
        if let Some((op, target)) = simple {
            return op_on(op, target);
        }
        let c2 = match c {
            'p' => Some(Cmd::Paste { before: false }),
            'P' => Some(Cmd::Paste { before: true }),
            'u' => Some(Cmd::Undo),
            'J' => Some(Cmd::Join),
            'r' => Some(Cmd::Replace(p.char()?)),
            '~' => Some(Cmd::Tilde),
            '.' => Some(Cmd::Repeat),
            'i' => Some(Cmd::Insert(InsertAt::Before)),
            'a' => Some(Cmd::Insert(InsertAt::After)),
            'I' => Some(Cmd::Insert(InsertAt::LineStart)),
            'A' => Some(Cmd::Insert(InsertAt::LineEnd)),
            'o' => Some(Cmd::Insert(InsertAt::LineBelow)),
            'O' => Some(Cmd::Insert(InsertAt::LineAbove)),
            'v' => Some(Cmd::Visual { linewise: false }),
            'V' => Some(Cmd::Visual { linewise: true }),
            ':' => Some(Cmd::CommandLine),
            '/' => Some(Cmd::Search { forward: true }),
            '?' => Some(Cmd::Search { forward: false }),
            'n' => Some(Cmd::SearchNext { reverse: false }),
            'N' => Some(Cmd::SearchNext { reverse: true }),
            '*' => Some(Cmd::SearchWord { forward: true }),
            '#' => Some(Cmd::SearchWord { forward: false }),
            _ => None,
        };
        if let Some(c2) = c2 {
            return cmd(c2);
        }
    }
    match c {
        ';' => cmd(Cmd::RepeatFind { reverse: false }),
        ',' => cmd(Cmd::RepeatFind { reverse: true }),
        _ => match motion(p, c)? {
            Some(m) => done(Kind::Motion(m)),
            None => Err(Fail::Invalid),
        },
    }
}

/// 演算子の後ろ: 回数、同じ演算子 (dd)、テキストオブジェクト、モーション。
fn operator_target(p: &mut P, op: Operator, op_char: char, count1: Option<usize>) -> Result<Action, Fail> {
    let count2 = p.count();
    let count = mul(count1, count2);
    let done = |target| Ok(Action { count, kind: Kind::Operator { op, target } });
    let t = p.next()?;
    let c = match t {
        Tok::Ctrl(c) => return ctrl_motion(c).map_or(Err(Fail::Invalid), |m| done(OpTarget::Motion(m))),
        Tok::C(c) => c,
    };
    // dd, cc, yy, >>, <<, g~~ / g~g~, guu / gugu, gUU / gUgU
    let doubled = if op_char == 'g' {
        let second = match op {
            Operator::ToggleCase => '~',
            Operator::Lower => 'u',
            _ => 'U',
        };
        c == second || (c == 'g' && p.char()? == second)
    } else {
        c == op_char
    };
    if doubled {
        return done(OpTarget::Lines);
    }
    match c {
        'i' | 'a' => {
            let obj = text_object(p.char()?).ok_or(Fail::Invalid)?;
            done(OpTarget::Object { obj, inner: c == 'i' })
        }
        ';' | ',' => Ok(Action { count, kind: Kind::OperatorRepeatFind { op, reverse: c == ',' } }),
        'g' => match p.char()? {
            'g' => done(OpTarget::Motion(VimMotion::GotoLine { last: false })),
            _ => Err(Fail::Invalid),
        },
        _ => match motion(p, c)? {
            Some(m) => done(OpTarget::Motion(m)),
            None => Err(Fail::Invalid),
        },
    }
}

fn ctrl_motion(c: char) -> Option<VimMotion> {
    Some(match c {
        'd' => VimMotion::HalfPageDown,
        'u' => VimMotion::HalfPageUp,
        'f' => VimMotion::PageDown,
        'b' => VimMotion::PageUp,
        _ => return None,
    })
}

fn motion(p: &mut P, c: char) -> Result<Option<VimMotion>, Fail> {
    use VimMotion as M;
    Ok(Some(match c {
        'h' => M::Left,
        'l' | ' ' => M::Right,
        'j' => M::Down,
        'k' => M::Up,
        'w' => M::WordStart { big: false },
        'W' => M::WordStart { big: true },
        'b' => M::WordBack { big: false },
        'B' => M::WordBack { big: true },
        'e' => M::WordEnd { big: false },
        'E' => M::WordEnd { big: true },
        '0' => M::LineStart,
        '^' | '_' => M::FirstNonBlank,
        '$' => M::LineEnd,
        'G' => M::GotoLine { last: true },
        '%' => M::MatchPair,
        '}' => M::ParagraphForward,
        '{' => M::ParagraphBack,
        'H' => M::ScreenTop,
        'M' => M::ScreenMiddle,
        'L' => M::ScreenBottom,
        '+' => M::NextLineStart,
        '-' => M::PrevLineStart,
        'f' | 'F' | 't' | 'T' => {
            let ch = p.char()?;
            M::FindChar { c: ch, forward: c == 'f' || c == 't', till: c == 't' || c == 'T', repeat: false }
        }
        _ => return Ok(None),
    }))
}

fn text_object(c: char) -> Option<TextObject> {
    Some(match c {
        'w' => TextObject::Word { big: false },
        'W' => TextObject::Word { big: true },
        '"' | '\'' | '`' => TextObject::Quote(c),
        '(' | ')' | 'b' => TextObject::Bracket('(', ')'),
        '[' | ']' => TextObject::Bracket('[', ']'),
        '{' | '}' | 'B' => TextObject::Bracket('{', '}'),
        '<' | '>' => TextObject::Bracket('<', '>'),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        s.chars().map(Tok::C).collect()
    }

    fn done(s: &str) -> Action {
        match parse(&toks(s), false) {
            Parsed::Done(a) => a,
            other => panic!("{s}: {other:?}"),
        }
    }

    #[test]
    fn counts_and_operators() {
        let a = done("2d3w");
        assert_eq!(a.count, Some(6));
        assert_eq!(a.kind, Kind::Operator { op: Operator::Delete, target: OpTarget::Motion(VimMotion::WordStart { big: false }) });
        assert_eq!(done("dd").kind, Kind::Operator { op: Operator::Delete, target: OpTarget::Lines });
        assert_eq!(done("guu").kind, Kind::Operator { op: Operator::Lower, target: OpTarget::Lines });
        assert_eq!(done("gUgU").kind, Kind::Operator { op: Operator::Upper, target: OpTarget::Lines });
        assert_eq!(
            done("ci(").kind,
            Kind::Operator { op: Operator::Change, target: OpTarget::Object { obj: TextObject::Bracket('(', ')'), inner: true } }
        );
        assert_eq!(done("10G").count, Some(10));
        assert_eq!(done("0").kind, Kind::Motion(VimMotion::LineStart));
        assert_eq!(done("10j").count, Some(10));
    }

    #[test]
    fn incomplete_and_invalid() {
        for s in ["d", "2", "g", "f", "di", "c2", "r"] {
            assert_eq!(parse(&toks(s), false), Parsed::Incomplete, "{s}");
        }
        for s in ["Q", "dQ", "gq", "diq"] {
            assert_eq!(parse(&toks(s), false), Parsed::Invalid, "{s}");
        }
    }

    #[test]
    fn find_and_replace() {
        assert_eq!(
            done("dtx").kind,
            Kind::Operator {
                op: Operator::Delete,
                target: OpTarget::Motion(VimMotion::FindChar { c: 'x', forward: true, till: true, repeat: false })
            }
        );
        assert_eq!(done("rあ").kind, Kind::Cmd(Cmd::Replace('あ')));
    }

    #[test]
    fn visual() {
        let p = |s: &str| parse(&toks(s), true);
        assert_eq!(p("d"), Parsed::Done(Action { count: None, kind: Kind::Operator { op: Operator::Delete, target: OpTarget::Visual } }));
        assert_eq!(p("iw"), Parsed::Done(Action { count: None, kind: Kind::Object { obj: TextObject::Word { big: false }, inner: true } }));
        assert!(matches!(p("3j"), Parsed::Done(Action { count: Some(3), kind: Kind::Motion(VimMotion::Down) })));
    }
}
