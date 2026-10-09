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
    def test_shared_runtime_submits_all_books_and_waits_for_every_epub(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in ["one.pdf", "two.pdf"]:
                (root / name).write_bytes(b"scan")
            manifest = root / "books.json"
            manifest.write_text(json.dumps([{"book_id": name, "sources": [name + ".pdf"]}
                                            for name in ["one", "two"]]))
            args = SimpleNamespace(manifest=manifest, sources=[], book_id=None,
                                   access="local_only", license="unknown", remote_ocr="off",
                                   directory=root / "run", endpoint="http://model/v1", structure_endpoint=None,
                                   max_concurrent=3, epub=True, timeout=60)
            jobs, prepares, grants, polls = {}, {}, [], {}

            def call(*argv, **kwargs):
                if argv[0] == "init":
                    return {"inference_profile_id": "reader"}
                if argv[:3] == ("plugin", "dirs", "add"):
                    grants.append(argv)
                if argv[:2] == ("query", "find"):
                    return {"results": []}
                if argv[:2] == ("document", "create"):
                    fields = json.loads(argv[-1])
                    if argv[2] == "ShelfJob":
                        jobs[fields["run_id"]] = fields
                    else:
                        self.assertEqual(len(jobs), 2)
                        prepares[fields["run_id"]] = fields
                return {}

            def query(home, collection, run_id, fields):
                if collection == "ShelfStructureReport":
                    return [{"book_id": jobs[run_id]["book_id"]}]
                if collection == "ShelfLibraryEdition":
                    polls[run_id] = polls.get(run_id, 0) + 1
                    return [{"edition_id": run_id}] if polls[run_id] > 1 else []
                return []

            def export(home, run_id, path, endpoint):
                path.write_text(json.dumps({"book_id": jobs[run_id]["book_id"]}))

            def process(*argv, **kwargs):
                kwargs["stdout"].write("gents server is running\n")
                kwargs["stdout"].flush()
                from unittest.mock import Mock
                return Mock(poll=lambda: None)

            with patch.object(shelf, "call", call), patch.object(shelf, "query", query), \
                 patch.object(shelf, "export", export), patch.object(shelf, "model", return_value="model"), \
                 patch.object(shelf.subprocess, "Popen", process), patch.object(shelf.time, "sleep"), \
                 contextlib.redirect_stdout(io.StringIO()):
                shelf.run(args)
            self.assertEqual(len(prepares), 2)
            self.assertEqual(list(polls.values()), [2, 2])
            self.assertEqual(len({job["path"] for job in prepares.values()}), 2)
            for job in prepares.values():
                grant = next(g for g in grants if str(g[3]) == job["path"])
                self.assertEqual(grant[grant.index("--access") + 1], "read_write")

    def test_denied_native_stage_is_reported_without_waiting_for_timeout(self):
        def call(*args):
            filters = json.loads(args[args.index("--filter") + 1])
            self.assertEqual(filters["caused_by_correlation"]["_in"], ["book", "edition"])
            self.assertEqual(filters["lifecycle_state"]["_in"], ["denied", "failed"])
            return {"results": [{"callback_id": "shelf-prepare", "lifecycle_state": "denied",
                                 "error": "export folder is read-only"}]}
        with patch.object(shelf, "call", call):
            with self.assertRaisesRegex(RuntimeError, "export folder is read-only"):
                shelf.check_callback_failures("home", ["book", "edition"])

    def test_search_catalog_retries_truncated_pages_without_losing_editions(self):
        rows = [dict(book_id=str(i), edition_id="e-" + str(i), modified="1970-01-01T00:00:00Z", status="source_text",
                     access="open", language="en") for i in range(205)]
        offsets = []
        def call(*args):
            if args[:2] == ("query", "find"):
                limit = int(args[args.index("--limit") + 1])
                offset = int(args[args.index("--offset") + 1])
                offsets.append((offset, limit))
                return {"truncated": limit > 50, "results": rows[offset:offset + min(limit, 50)]}
            filters = json.loads(args[args.index("--filter") + 1])
            self.assertEqual(set(filters["edition_id"]["_in"]), {r["edition_id"] for r in rows})
            return {"results": []}
        args = SimpleNamespace(home="unused", text="grain", limit=5, book_id=None,
                               edition_id=None, language=None, access="open")
        with patch.object(shelf, "call", call):
            shelf.search_library(args)
        self.assertEqual(offsets, [(0, 100), (0, 50), (50, 50), (100, 50), (150, 50), (200, 50)])

    def test_ambiguous_book_does_not_disable_other_books_and_reviewed_wins(self):
        args=SimpleNamespace(home="unused",text="grain",limit=5,book_id=None,edition_id=None,language=None,access="all")
        rows=[]
        for book,edition,status,stamp in [('ambiguous','a','source_text','2026-01-01T00:00:00Z'),('ambiguous','b','source_text','2026-01-01T00:00:00Z'),('usable','raw-a','source_text','2026-01-01T00:00:00Z'),('usable','raw-b','source_text','2026-01-01T00:00:00Z'),('usable','reviewed','reviewed','2025-01-01T00:00:00Z')]:
            rows.append(dict(book_id=book,edition_id=edition,status=status,modified=stamp,access="open",language="en"))
        def call(*argv):
            if argv[1]=='find':return {"results":rows}
            self.assertEqual(json.loads(argv[argv.index('--filter')+1])['edition_id']['_in'],['reviewed'])
            return {"results":[{"book_id":"usable"}]}
        with patch.object(shelf,'call',call):
            result=shelf.search_library(args)
            self.assertEqual(result['results'][0]['book_id'],'usable')
            self.assertEqual(result['warnings'][0]['book_id'],'ambiguous')

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
            args = SimpleNamespace(home=root / "home", manifest=manifest, remote_ocr="auto")
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
                self.assertTrue(all(rows[0]["remote_ocr"] == "auto" for rows in stored.values()))
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
