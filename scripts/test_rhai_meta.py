#!/usr/bin/env python3
"""Tests for rhai_meta.py. Run: python3 scripts/test_rhai_meta.py"""
import json
import os
import subprocess
import sys
import tempfile
import unittest

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), "rhai_meta.py")


def run(*args):
    return subprocess.run([sys.executable, TOOL, *args], capture_output=True, text=True)


def rhai(name):
    return 'let meta = #{ name: "%s", description: "d" };\n' % name


ENTRY = {"name": "code-review", "file": "code_review.rhai", "description": "d", "when_to_use": "w"}


class Base(unittest.TestCase):
    def setUp(self):
        self._td = tempfile.TemporaryDirectory()
        self.addCleanup(self._td.cleanup)
        self.root = self._td.name

    def write(self, rel, text):
        path = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)
        return path

    def pack(self, entries=None, files=None, assets=None):
        """A valid one-workflow pack unless overridden."""
        entries = [ENTRY] if entries is None else entries
        files = {"code_review.rhai": rhai("code-review")} if files is None else files
        assets = ["workflows/code_review.rhai"] if assets is None else assets
        self.write("manifest.json", json.dumps({"kind": "documents", "assets": assets}))
        self.write("workflows/index.json", json.dumps(entries))
        for name, text in files.items():
            self.write("workflows/" + name, text)
        return self.root

    def fails(self, **kw):
        r = run("check-index", self.pack(**kw))
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        lines = [l for l in r.stdout.splitlines() if l.startswith("FAIL ")]
        self.assertEqual(len(lines), 1, r.stdout)
        return lines[0]


class MetaTests(Base):
    def meta(self, src):
        return run("meta", self.write("w.rhai", src))

    def test_meta_valid_literal(self):
        src = r'''
// leading comment
let meta = #{
    name: "wf",            /* block */
    "quoted-key": "a\n\"b\" \u0041 \x41",
    count: 3, ratio: 1.5, on: true, off: false,
    nested: #{ a: [1, 2, #{ b: [] },], },
    phases: ["recon", "scan",],
};
fn later() { 1 + 2 }
'''
        r = self.meta(src)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(json.loads(r.stdout), {
            "name": "wf",
            "quoted-key": 'a\n"b" A A',
            "count": 3, "ratio": 1.5, "on": True, "off": False,
            "nested": {"a": [1, 2, {"b": []}]},
            "phases": ["recon", "scan"],
        })

    def refused(self, src, token, line):
        r = self.meta(src)
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn(token, r.stderr)
        self.assertIn("line %d" % line, r.stderr)

    def test_meta_refuses_identifier(self):
        self.refused('let meta = #{\n  name: some_var,\n};', "some_var", 2)

    def test_meta_refuses_call(self):
        self.refused('let meta = #{\n  name: "a",\n  d: make(1),\n};', "make", 3)

    def test_meta_refuses_operator(self):
        self.refused('let meta = #{\n  n: 1 + 2,\n};', "+", 2)

    def test_meta_refuses_backtick_interpolation(self):
        self.refused('let meta = #{\n  n: `a${b}`,\n};', "`", 2)


class IndexTests(Base):
    def test_index_absent_is_ok(self):
        self.write("manifest.json", "{}")
        r = run("check-index", self.root)
        self.assertEqual((r.returncode, r.stdout, r.stderr), (0, "", ""))

    def test_index_valid_pack_passes(self):
        r = run("check-index", self.pack())
        self.assertEqual((r.returncode, r.stdout, r.stderr), (0, "", ""))

    def test_index_wrong_file_name(self):
        e = dict(ENTRY, file="code-review.rhai")
        line = self.fails(entries=[e], files={"code-review.rhai": rhai("code-review")},
                          assets=["workflows/code-review.rhai"])
        self.assertIn("file must be code_review.rhai", line)

    def test_index_missing_file(self):
        self.assertIn("does not exist", self.fails(files={}))

    def test_index_file_not_in_manifest_assets(self):
        self.assertIn("not in manifest assets", self.fails(assets=[]))

    def test_index_meta_name_mismatch(self):
        self.assertIn("meta.name", self.fails(files={"code_review.rhai": rhai("other")}))

    def test_index_duplicate_name(self):
        self.assertIn("duplicate name", self.fails(entries=[ENTRY, ENTRY]))

    def test_index_unlisted_rhai(self):
        line = self.fails(files={"code_review.rhai": rhai("code-review"), "extra.rhai": rhai("extra")})
        self.assertIn("not listed", line)

    def test_index_missing_key(self):
        e = {k: v for k, v in ENTRY.items() if k != "when_to_use"}
        self.assertIn("missing key", self.fails(entries=[e]))

    def test_index_extra_key(self):
        self.assertIn("extra key", self.fails(entries=[dict(ENTRY, extra="x")]))


if __name__ == "__main__":
    unittest.main()
