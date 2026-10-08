import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("shelf_book", Path(__file__).with_name("shelf-book.py"))
shelf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shelf)


class BatchSubmission(unittest.TestCase):
    def test_resume_after_partial_submission_and_refuse_conflicting_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ["one.pdf", "two.pdf", "three.pdf"]:
                (root / name).write_bytes(b"fixture")
            manifest = root / "books.json"
            manifest.write_text(json.dumps([
                {"run_id": name, "sources": [name + ".pdf"]}
                for name in ["one", "two", "three"]
            ]))
            args = SimpleNamespace(home=root / "home", manifest=manifest)
            stored, writes = {}, []

            def query(home, collection, run_id, fields):
                return stored.get(run_id, [])

            def create(*argv):
                self.assertEqual(argv[:3], ("document", "create", "ShelfJob"))
                fields = json.loads(argv[-1])
                run_id = fields.pop("run_id")
                if run_id == "two" and not writes.count("two"):
                    writes.append("two")
                    raise OSError("interrupted between documents")
                writes.append(run_id)
                stored[run_id] = [dict(fields, files=None)]

            with patch.object(shelf, "query", query), patch.object(shelf, "call", create), contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaises(OSError):
                    shelf.submit_batch(args)
                shelf.submit_batch(args)
                self.assertEqual(set(stored), {"one", "two", "three"})
                self.assertEqual(writes.count("one"), 1)
                before = list(writes)
                shelf.submit_batch(args)
                self.assertEqual(writes, before)
                stored["three"][0]["path"] = "different-source"
                del stored["one"]
                with self.assertRaisesRegex(RuntimeError, "different job"):
                    shelf.submit_batch(args)
                self.assertEqual(writes, before)


if __name__ == "__main__":
    unittest.main()
