#!/usr/bin/env python3
"""動作検証用のテストファイルを生成する。

使い方: python3 scripts/gen_testdata.py <出力先ディレクトリ> [大容量ファイルのMB数 (既定 300)]
"""
import os
import sys

JA = "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。"


def write(path, data: bytes):
    with open(path, "wb") as f:
        f.write(data)
    print(f"  {os.path.basename(path)}  ({len(data):,} bytes)")


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)
    out = sys.argv[1]
    big_mb = int(sys.argv[2]) if len(sys.argv) > 2 else 300
    os.makedirs(out, exist_ok=True)
    print(f"出力先: {out}")

    text = "\r\n".join(f"{i:05d}: {JA} The quick brown fox." for i in range(1, 201)) + "\r\n"
    write(os.path.join(out, "utf8_crlf.txt"), text.encode("utf-8"))
    write(os.path.join(out, "utf8_bom.txt"), b"\xef\xbb\xbf" + text.encode("utf-8"))
    write(os.path.join(out, "utf8_lf.txt"), text.replace("\r\n", "\n").encode("utf-8"))
    write(os.path.join(out, "sjis.txt"), text.encode("shift_jis"))
    write(os.path.join(out, "eucjp.txt"), text.encode("euc_jp"))
    write(os.path.join(out, "utf16le.txt"), b"\xff\xfe" + text.encode("utf-16-le"))
    write(os.path.join(out, "utf16be.txt"), b"\xfe\xff" + text.encode("utf-16-be"))
    write(os.path.join(out, "emoji_surrogate.txt"), "絵文字 😀🎉 𠮷野家 (サロゲートペア)\r\n".encode("utf-8"))
    write(os.path.join(out, "long_line.txt"), ("abcdefghij" * 100_000 + "\r\nnext line\r\n").encode("utf-8"))

    md = """# 見出し1

本文に **太字** と *斜体* と `インラインコード` と [リンク](https://example.com)。

## 見出し2

- 箇条書き1
- 箇条書き2
  1. 番号付き

> 引用文です。

```rust
fn main() {
    # これは見出しではなくコード
    println!("hello");
}
```
"""
    write(os.path.join(out, "sample.md"), md.replace("\n", "\r\n").encode("utf-8"))

    # 大容量ファイル (1 行約 100 バイト)
    path = os.path.join(out, f"large_{big_mb}mb.txt")
    line_tmpl = "{:09d}\t" + JA + " lorem ipsum dolor sit amet\r\n"
    target = big_mb * 1024 * 1024
    written = 0
    i = 0
    with open(path, "wb") as f:
        buf = []
        while written < target:
            b = line_tmpl.format(i).encode("utf-8")
            buf.append(b)
            written += len(b)
            i += 1
            if len(buf) >= 10000:
                f.write(b"".join(buf))
                buf.clear()
        f.write(b"".join(buf))
    print(f"  {os.path.basename(path)}  ({written:,} bytes, {i:,} 行)")


if __name__ == "__main__":
    main()
