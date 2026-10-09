//! 文字コード・改行コードの判定と変換、ファイルの読み書き。
//!
//! 読み込み時は内部表現 (UTF-8 / 改行 LF) に変換し、保存時に元の形式へ戻す。

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;

use encoding_rs::{Encoding, EncoderResult, EUC_JP, SHIFT_JIS, UTF_8};
use ropey::{Rope, RopeBuilder};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    ShiftJis,
    EucJp,
    /// chardetng が推定したその他の文字コード (windows-1252 など)。
    Other(&'static Encoding),
}

impl TextEncoding {
    /// メニューに並べる主要な文字コード。
    pub const COMMON: [TextEncoding; 6] = [
        TextEncoding::Utf8,
        TextEncoding::Utf8Bom,
        TextEncoding::Utf16Le,
        TextEncoding::Utf16Be,
        TextEncoding::ShiftJis,
        TextEncoding::EucJp,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::Utf8Bom => "UTF-8 (BOM)",
            TextEncoding::Utf16Le => "UTF-16 LE",
            TextEncoding::Utf16Be => "UTF-16 BE",
            TextEncoding::ShiftJis => "Shift_JIS",
            TextEncoding::EucJp => "EUC-JP",
            TextEncoding::Other(e) => e.name(),
        }
    }

    fn from_encoding(e: &'static Encoding) -> Self {
        if e == UTF_8 {
            TextEncoding::Utf8
        } else if e == SHIFT_JIS {
            TextEncoding::ShiftJis
        } else if e == EUC_JP {
            TextEncoding::EucJp
        } else {
            TextEncoding::Other(e)
        }
    }

    /// 読み込みに使う encoding_rs のエンコーディング。
    fn decoder_encoding(&self) -> &'static Encoding {
        match self {
            TextEncoding::Utf16Le => encoding_rs::UTF_16LE,
            TextEncoding::Utf16Be => encoding_rs::UTF_16BE,
            other => other.legacy(),
        }
    }

    fn legacy(&self) -> &'static Encoding {
        match self {
            TextEncoding::ShiftJis => SHIFT_JIS,
            TextEncoding::EucJp => EUC_JP,
            TextEncoding::Other(e) => e,
            _ => UTF_8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    CrLf,
    Lf,
    Cr,
}

impl LineEnding {
    pub const ALL: [LineEnding; 3] = [LineEnding::CrLf, LineEnding::Lf, LineEnding::Cr];

    pub fn as_str(&self) -> &'static str {
        match self {
            LineEnding::CrLf => "\r\n",
            LineEnding::Lf => "\n",
            LineEnding::Cr => "\r",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            LineEnding::CrLf => "CRLF",
            LineEnding::Lf => "LF",
            LineEnding::Cr => "CR",
        }
    }

    /// Windows 向けアプリなので新規ファイルは CRLF。
    pub fn platform_default() -> Self {
        LineEnding::CrLf
    }
}

pub struct DecodedText {
    pub rope: Rope,
    pub encoding: TextEncoding,
    pub line_ending: LineEnding,
    /// 変換できないバイト列があり置換文字 (U+FFFD) に置き換えた。
    pub had_errors: bool,
}

/// 読み込み・判定に使う先頭部分のサイズ。
const HEAD_BYTES: usize = 1 << 20;

/// バイト列を内部表現に変換する。`forced` が指定されていれば判定をせずその文字コードで読む。
pub fn decode(bytes: Vec<u8>, forced: Option<TextEncoding>) -> DecodedText {
    decode_reader(io::Cursor::new(bytes), forced).expect("reading from memory cannot fail")
}

/// ファイルを読み込む。全体を一度にメモリへ読まず、1MB ずつ変換しながら rope を構築する
/// (ピークメモリ ≒ 最終的な rope のサイズ)。
pub fn load_file(path: &Path, forced: Option<TextEncoding>) -> io::Result<DecodedText> {
    let decoded = decode_reader(File::open(path)?, forced)?;
    // 先頭 1MB だけを見て UTF-8 と推定したが、後半に UTF-8 として不正なバイト列があった場合は
    // ファイル全体で文字コードを推定し直す
    if forced.is_none() && decoded.had_errors && decoded.encoding == TextEncoding::Utf8 {
        let guessed = guess_stream(File::open(path)?)?;
        if guessed != TextEncoding::Utf8 {
            return decode_reader(File::open(path)?, Some(guessed));
        }
    }
    Ok(decoded)
}

fn decode_reader<R: Read>(mut r: R, forced: Option<TextEncoding>) -> io::Result<DecodedText> {
    let mut head = Vec::with_capacity(HEAD_BYTES);
    (&mut r).take(HEAD_BYTES as u64).read_to_end(&mut head)?;
    let is_whole = head.len() < HEAD_BYTES;

    let bom = if head.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some((TextEncoding::Utf8Bom, 3))
    } else if head.starts_with(&[0xFF, 0xFE]) {
        Some((TextEncoding::Utf16Le, 2))
    } else if head.starts_with(&[0xFE, 0xFF]) {
        Some((TextEncoding::Utf16Be, 2))
    } else {
        None
    };
    let (encoding, skip) = match (forced, bom) {
        (Some(f), Some((b, n))) if same_family(f, b) => (b, n),
        (Some(f), _) => (f, 0),
        (None, Some((b, n))) => (b, n),
        (None, None) => (guess(&head, is_whole), 0),
    };

    let mut decoder = encoding.decoder_encoding().new_decoder_without_bom_handling();
    let mut norm = Normalizer::default();
    let mut had_errors = false;
    let mut out = String::new();
    let mut feed = |src: &[u8], last: bool, norm: &mut Normalizer| {
        out.clear();
        out.reserve(decoder.max_utf8_buffer_length(src.len()).unwrap_or(src.len() * 3));
        let (_, _, err) = decoder.decode_to_string(src, &mut out, last);
        had_errors |= err;
        norm.push(&out);
    };
    feed(&head[skip..], is_whole, &mut norm);
    if !is_whole {
        drop(head);
        let mut buf = vec![0u8; HEAD_BYTES];
        loop {
            let n = read_full(&mut r, &mut buf)?;
            feed(&buf[..n], n == 0, &mut norm);
            if n == 0 {
                break;
            }
        }
    }
    let (rope, line_ending) = norm.finish();
    Ok(DecodedText { rope, encoding, line_ending, had_errors })
}

/// `buf` が埋まるか EOF まで読む。
fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

fn same_family(a: TextEncoding, b: TextEncoding) -> bool {
    use TextEncoding::*;
    matches!((a, b), (Utf8 | Utf8Bom, Utf8 | Utf8Bom)) || a == b
}

/// BOM がない場合の文字コード推定 (先頭部分のみを見る)。
fn guess(head: &[u8], is_whole: bool) -> TextEncoding {
    match std::str::from_utf8(head) {
        Ok(_) => return TextEncoding::Utf8,
        // 末尾で文字が途切れているだけなら有効な UTF-8 とみなす
        Err(e) if e.error_len().is_none() && !is_whole => return TextEncoding::Utf8,
        Err(_) => {}
    }
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    det.feed(head, is_whole);
    detector_guess(&det)
}

/// ファイル全体を使った推定 (先頭だけでは判定できなかった場合)。
fn guess_stream<R: Read>(mut r: R) -> io::Result<TextEncoding> {
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    let mut buf = vec![0u8; HEAD_BYTES];
    loop {
        let n = read_full(&mut r, &mut buf)?;
        det.feed(&buf[..n], n == 0);
        if n == 0 {
            return Ok(detector_guess(&det));
        }
    }
}

fn detector_guess(det: &chardetng::EncodingDetector) -> TextEncoding {
    // 日本語環境を想定し、TLD に "jp" を与えて日本語系の推定を優先させる
    TextEncoding::from_encoding(det.guess(Some(b"jp"), chardetng::Utf8Detection::Allow))
}

/// 改行コードを LF に正規化しながら rope を構築する。各改行コードの出現数も数える。
#[derive(Default)]
struct Normalizer {
    builder: RopeBuilder,
    buf: String,
    pending_cr: bool,
    crlf: usize,
    lf: usize,
    cr: usize,
}

impl Normalizer {
    const FLUSH_BYTES: usize = 64 * 1024;

    fn push(&mut self, text: &str) {
        let mut rest = text;
        if std::mem::take(&mut self.pending_cr) {
            if let Some(r) = rest.strip_prefix('\n') {
                self.crlf += 1;
                rest = r;
            } else {
                self.cr += 1;
            }
            self.buf.push('\n');
        }
        while let Some(i) = rest.find('\r') {
            self.push_plain(&rest[..i]);
            let after = &rest[i + 1..];
            if after.is_empty() {
                // 次の塊が LF で始まるかもしれないので保留
                self.pending_cr = true;
                return;
            }
            match after.strip_prefix('\n') {
                Some(a) => {
                    self.crlf += 1;
                    rest = a;
                }
                None => {
                    self.cr += 1;
                    rest = after;
                }
            }
            self.buf.push('\n');
        }
        self.push_plain(rest);
    }

    fn push_plain(&mut self, s: &str) {
        self.lf += s.bytes().filter(|&b| b == b'\n').count();
        self.buf.push_str(s);
        if self.buf.len() >= Self::FLUSH_BYTES {
            self.builder.append(&self.buf);
            self.buf.clear();
        }
    }

    fn finish(mut self) -> (Rope, LineEnding) {
        if self.pending_cr {
            self.cr += 1;
            self.buf.push('\n');
        }
        self.builder.append(&self.buf);
        let (crlf, lf, cr) = (self.crlf, self.lf, self.cr);
        let eol = if crlf == 0 && lf == 0 && cr == 0 {
            LineEnding::platform_default()
        } else if crlf >= lf && crlf >= cr {
            LineEnding::CrLf
        } else if lf >= cr {
            LineEnding::Lf
        } else {
            LineEnding::Cr
        };
        (self.builder.finish(), eol)
    }
}

/// 最も多く使われている改行コードを返す。改行がなければ既定値。
pub fn detect_line_ending(text: &str) -> LineEnding {
    let mut n = Normalizer::default();
    n.push(text);
    n.finish().1
}

/// CRLF / CR を LF に正規化した rope を作る。
pub fn build_rope_normalized(text: &str) -> Rope {
    let mut n = Normalizer::default();
    n.push(text);
    n.finish().0
}

#[derive(Debug)]
pub enum SaveError {
    Io(io::Error),
    /// 指定の文字コードで表現できない文字がある。
    Unmappable(char),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Io(e) => write!(f, "{e}"),
            SaveError::Unmappable(c) => write!(f, "文字 '{c}' (U+{:04X}) はこの文字コードで保存できません", *c as u32),
        }
    }
}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        SaveError::Io(e)
    }
}

/// rope を指定の文字コード・改行コードで書き出す。
pub fn encode_to<W: Write>(rope: &Rope, encoding: TextEncoding, eol: LineEnding, w: &mut W) -> Result<(), SaveError> {
    match encoding {
        TextEncoding::Utf8Bom => w.write_all(&[0xEF, 0xBB, 0xBF])?,
        TextEncoding::Utf16Le => w.write_all(&[0xFF, 0xFE])?,
        TextEncoding::Utf16Be => w.write_all(&[0xFE, 0xFF])?,
        _ => {}
    }
    let mut encoder = match encoding {
        TextEncoding::ShiftJis | TextEncoding::EucJp | TextEncoding::Other(_) => Some(encoding.legacy().new_encoder()),
        _ => None,
    };
    let mut converted = String::new();
    let mut out = vec![0u8; 64 * 1024];
    for chunk in rope.chunks() {
        let chunk: &str = if eol == LineEnding::Lf {
            chunk
        } else {
            converted.clear();
            converted.push_str(&chunk.replace('\n', eol.as_str()));
            &converted
        };
        match encoding {
            TextEncoding::Utf8 | TextEncoding::Utf8Bom => w.write_all(chunk.as_bytes())?,
            TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
                let le = encoding == TextEncoding::Utf16Le;
                let mut buf = Vec::with_capacity(chunk.len() * 2);
                for u in chunk.encode_utf16() {
                    buf.extend_from_slice(&if le { u.to_le_bytes() } else { u.to_be_bytes() });
                }
                w.write_all(&buf)?;
            }
            _ => encode_legacy(encoder.as_mut().unwrap(), chunk, false, &mut out, w)?,
        }
    }
    if let Some(enc) = encoder.as_mut() {
        encode_legacy(enc, "", true, &mut out, w)?;
    }
    Ok(())
}

fn encode_legacy<W: Write>(
    enc: &mut encoding_rs::Encoder,
    src: &str,
    last: bool,
    out: &mut [u8],
    w: &mut W,
) -> Result<(), SaveError> {
    let mut pos = 0;
    loop {
        let (res, read, written) = enc.encode_from_utf8_without_replacement(&src[pos..], out, last);
        w.write_all(&out[..written])?;
        pos += read;
        match res {
            EncoderResult::InputEmpty => return Ok(()),
            EncoderResult::OutputFull => continue,
            EncoderResult::Unmappable(c) => return Err(SaveError::Unmappable(c)),
        }
    }
}

/// 一時ファイルに書き出してから置き換える (書き込み途中で失敗しても元ファイルを壊さない)。
pub fn save_file(path: &Path, rope: &Rope, encoding: TextEncoding, eol: LineEnding) -> Result<(), SaveError> {
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{file_name}.lvt-tmp"));
    let result = (|| {
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&tmp)?);
        encode_to(rope, encoding, eol, &mut w)?;
        w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(text: &str, enc: TextEncoding, eol: LineEnding) {
        let rope = build_rope_normalized(text);
        let mut bytes = Vec::new();
        encode_to(&rope, enc, eol, &mut bytes).unwrap();
        let d = decode(bytes, None);
        assert_eq!(d.rope.to_string(), rope.to_string());
        assert_eq!(d.line_ending, eol);
    }

    #[test]
    fn normalizes_line_endings() {
        assert_eq!(build_rope_normalized("a\r\nb\rc\nd").to_string(), "a\nb\nc\nd");
        assert_eq!(detect_line_ending("a\r\nb\r\nc\n"), LineEnding::CrLf);
        assert_eq!(detect_line_ending("a\nb\n"), LineEnding::Lf);
    }

    #[test]
    fn roundtrips() {
        roundtrip("hello\nworld\n", TextEncoding::Utf8, LineEnding::CrLf);
        roundtrip("日本語\nテキスト", TextEncoding::Utf8Bom, LineEnding::Lf);
        roundtrip("日本語\nテキスト", TextEncoding::Utf16Le, LineEnding::CrLf);
        roundtrip("日本語\nテキスト", TextEncoding::Utf16Be, LineEnding::Lf);
    }

    #[test]
    fn detects_shift_jis() {
        let (bytes, _, _) = SHIFT_JIS.encode("これはシフトJISで書かれた日本語の文章です。\r\n漢字とかなを含みます。");
        let d = decode(bytes.into_owned(), None);
        assert_eq!(d.encoding, TextEncoding::ShiftJis);
        assert!(d.rope.to_string().starts_with("これは"));
    }

    #[test]
    fn crlf_split_across_chunks() {
        let mut n = Normalizer::default();
        n.push("a\r");
        n.push("\nb\r");
        n.push("c\r");
        let (rope, eol) = n.finish();
        assert_eq!(rope.to_string(), "a\nb\nc\n");
        assert_eq!(eol, LineEnding::Cr);
    }

    #[test]
    fn large_input_streams_correctly() {
        // HEAD_BYTES を超える入力で、塊の境界をまたぐマルチバイト文字と CRLF を正しく扱う
        let line = "あいうえお\r\n";
        let text = line.repeat(HEAD_BYTES / line.len() * 2 + 7);
        let d = decode(text.clone().into_bytes(), None);
        assert_eq!(d.encoding, TextEncoding::Utf8);
        assert!(!d.had_errors);
        assert_eq!(d.line_ending, LineEnding::CrLf);
        assert_eq!(d.rope.to_string(), text.replace("\r\n", "\n"));

        let (sjis, _, _) = SHIFT_JIS.encode(&text);
        let d = decode(sjis.into_owned(), None);
        assert_eq!(d.encoding, TextEncoding::ShiftJis);
        assert_eq!(d.rope.len_chars(), text.replace("\r\n", "\n").chars().count());
    }

    #[test]
    fn unmappable_is_error() {
        let rope = Rope::from_str("😀");
        let r = encode_to(&rope, TextEncoding::ShiftJis, LineEnding::Lf, &mut Vec::new());
        assert!(matches!(r, Err(SaveError::Unmappable('😀'))));
    }
}
