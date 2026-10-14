#!/usr/bin/env python3
"""Literal-only Rhai meta parser and workflow index check (stdlib only).

  rhai_meta.py meta <file.rhai>       print the `let meta = #{...};` literal as JSON
  rhai_meta.py check-index <pack-dir> validate <pack-dir>/workflows/index.json
"""
import glob
import json
import os
import re
import sys

NAME = re.compile(r"[a-z0-9]+(-[a-z0-9]+)*\Z")
KEYS = {"name", "file", "description", "when_to_use"}

TOKEN = re.compile(r"""
    (?P<ws>\s+|//[^\n]*|/\*.*?\*/)
  | (?P<num>\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)
  | (?P<id>[A-Za-z_]\w*)
  | (?P<str>"(?:[^"\\]|\\.)*")
  | (?P<p>\#\{|[\[\]},:;=])
""", re.X | re.S)
ESCAPE = re.compile(r"\\(x[0-9a-fA-F]{2}|u[0-9a-fA-F]{4}|.)", re.S)
SIMPLE = {"n": "\n", "r": "\r", "t": "\t", "0": "\0", "\\": "\\", '"': '"', "'": "'"}


class Bad(Exception):
    pass


def unescape(s):
    def sub(m):
        e = m.group(1)
        if e[0] in "xu" and len(e) > 1:
            return chr(int(e[1:], 16))
        if e in SIMPLE:
            return SIMPLE[e]
        raise Bad("unknown escape \\%s" % e)
    return ESCAPE.sub(sub, s[1:-1])


class Lexer:
    def __init__(self, src, pos):
        self.src, self.pos = src, pos
        self.tok = None
        self.advance()

    def advance(self):
        """Move to the next non-whitespace token; self.tok = (kind, text, line)."""
        while True:
            line = self.src.count("\n", 0, self.pos) + 1
            m = TOKEN.match(self.src, self.pos)
            if self.pos >= len(self.src):
                self.tok = ("eof", "end of file", line)
                return
            if not m:
                raise Bad("unexpected %r at line %d" % (self.src[self.pos], line))
            self.pos = m.end()
            if m.lastgroup != "ws":
                self.tok = (m.lastgroup, m.group(), line)
                return

    def fail(self):
        _, text, line = self.tok
        raise Bad("unexpected %r at line %d" % (text, line))

    def take(self, text):
        if self.tok[1] != text or self.tok[0] in ("str", "id"):
            self.fail()
        self.advance()


def value(lx):
    kind, text, _ = lx.tok
    if kind == "num":
        lx.advance()
        return float(text) if re.search(r"[.eE]", text) else int(text)
    if kind == "str":
        lx.advance()
        return unescape(text)
    if kind == "id" and text in ("true", "false"):
        lx.advance()
        return text == "true"
    if kind == "p" and text == "[":
        lx.advance()
        out = []
        while lx.tok[1] != "]" or lx.tok[0] != "p":
            out.append(value(lx))
            if lx.tok[1] == ",":
                lx.advance()
            elif lx.tok[1] != "]":
                lx.fail()
        lx.advance()
        return out
    if kind == "p" and text == "#{":
        lx.advance()
        out = {}
        while lx.tok[1] != "}" or lx.tok[0] != "p":
            k_kind, k, _ = lx.tok
            if k_kind not in ("id", "str"):
                lx.fail()
            lx.advance()
            lx.take(":")
            out[unescape(k) if k_kind == "str" else k] = value(lx)
            if lx.tok[1] == ",":
                lx.advance()
            elif lx.tok[1] != "}":
                lx.fail()
        lx.advance()
        return out
    lx.fail()


def parse_meta(src):
    lx = Lexer(src, 0)
    for want in ("let", "meta", "=", "#{"):
        if lx.tok[1] != want or lx.tok[0] in ("str", "num"):
            raise Bad("the script must start with `let meta = #{`")
        if want != "#{":
            lx.advance()
    meta = value(lx)
    lx.take(";")
    return meta


def snake(name):
    return name.replace("-", "_")


def check_index(pack):
    index = os.path.join(pack, "workflows", "index.json")
    if not os.path.exists(index):
        return []
    try:
        with open(index) as f:
            entries = json.load(f)
        with open(os.path.join(pack, "manifest.json")) as f:
            assets = set(json.load(f).get("assets", []))
    except (OSError, ValueError) as e:
        return ["FAIL cannot read index.json or manifest.json: %s" % e]
    if not isinstance(entries, list):
        return ["FAIL index.json must be an array"]
    fails, seen, listed = [], set(), set()
    for i, e in enumerate(entries):
        if not isinstance(e, dict):
            fails.append("FAIL entry %d: must be an object" % i)
            continue
        listed.add(e.get("file"))
        missing, extra = KEYS - set(e), set(e) - KEYS
        if missing:
            fails.append("FAIL entry %d: missing key(s) %s" % (i, sorted(missing)))
        if extra:
            fails.append("FAIL entry %d: extra key(s) %s" % (i, sorted(extra)))
        if missing or extra:
            continue
        bad = [k for k in sorted(KEYS) if not isinstance(e[k], str) or not e[k]]
        if bad:
            fails.append("FAIL entry %d: %s must be non-empty strings" % (i, bad))
            continue
        name, file = e["name"], e["file"]
        if name in seen:
            fails.append("FAIL duplicate name %s" % name)
        seen.add(name)
        if not NAME.match(name):
            fails.append("FAIL %s: name must be lowercase words joined by hyphens" % name)
        if file != snake(name) + ".rhai":
            fails.append("FAIL %s: file must be %s.rhai" % (name, snake(name)))
        if "workflows/" + file not in assets:
            fails.append("FAIL %s: workflows/%s not in manifest assets" % (name, file))
        path = os.path.join(pack, "workflows", file)
        if not os.path.isfile(path):
            fails.append("FAIL %s: workflows/%s does not exist" % (name, file))
            continue
        try:
            with open(path) as f:
                got = parse_meta(f.read()).get("name")
        except Bad as err:
            fails.append("FAIL %s: meta: %s" % (name, err))
            continue
        if got != name:
            fails.append("FAIL %s: meta.name is %r" % (name, got))
    for path in sorted(glob.glob(os.path.join(pack, "workflows", "*.rhai"))):
        if os.path.basename(path) not in listed:
            fails.append("FAIL workflows/%s is not listed in index.json" % os.path.basename(path))
    return fails


def main(argv):
    if len(argv) == 3 and argv[1] == "meta":
        try:
            with open(argv[2]) as f:
                print(json.dumps(parse_meta(f.read())))
        except Bad as e:
            print("%s: %s" % (argv[2], e), file=sys.stderr)
            return 1
        return 0
    if len(argv) == 3 and argv[1] == "check-index":
        fails = check_index(argv[2])
        for line in fails:
            print(line)
        return 1 if fails else 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
