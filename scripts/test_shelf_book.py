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
    def test_live_reads_and_writes_use_explicit_http_without_changing_other_calls(self):
        endpoint = "http://127.0.0.1:50327/api/v0/graphql"
        connection = shelf.LIVE_GRAPHQL.set(endpoint)
        try:
            with patch.object(shelf.subprocess, "run", return_value=SimpleNamespace(stdout="{}")) as run:
                for command in [("query", "find"), ("document", "create"), ("request", "interrupt")]:
                    shelf.call(*command, "--home", "home")
                    self.assertEqual(run.call_args.args[0][-2:], ["--graphql", endpoint])
                shelf.call("pack", "build", "pack")
                self.assertNotIn("--graphql", run.call_args.args[0])
                shelf.call("query", "find", "--graphql", "http://explicit")
                self.assertEqual(run.call_args.args[0].count("--graphql"), 1)
        finally:
            shelf.LIVE_GRAPHQL.reset(connection)
        self.assertIsNone(shelf.LIVE_GRAPHQL.get())

    def test_shared_runtime_submits_all_books_and_waits_for_every_epub(self):
        self._run_books()

    def test_transcription_modes_bind_the_vision_profile_and_preserve_the_job_mode(self):
        for mode in ["refine", "transcribe"]:
            with self.subTest(mode=mode):
                self._run_books(remote_ocr=mode)

    def test_failed_book_does_not_stop_the_other_book(self):
        self._run_books(fail_first=True)

    def test_resume_cleans_up_failed_books_without_resubmitting_them(self):
        self._run_books(fail_first=True, resume=True)

    def test_resume_reuses_jobs_and_refuses_changed_sources(self):
        self._run_books(resume=True)

    def test_attach_reuses_runtime_and_jobs_without_managing_server(self):
        self._run_books(resume=True, attach=True)

    def test_attach_requires_existing_batch(self):
        with self.assertRaisesRegex(RuntimeError, "--attach requires --resume"):
            shelf.run(SimpleNamespace(attach=True, resume=False))

    def _run_books(self, fail_first=False, resume=False, attach=False, remote_ocr="off"):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in ["one.pdf", "two.pdf"]:
                (root / name).write_bytes(b"scan")
            manifest = root / "books.json"
            manifest.write_text(json.dumps([{"book_id": name, "sources": [name + ".pdf"]}
                                            for name in ["one", "two"]]))
            args = SimpleNamespace(manifest=manifest, sources=[], book_id=None,
                                   access="local_only", license="unknown", remote_ocr=remote_ocr,
                                   directory=root / "run", endpoint="http://model/v1", structure_endpoint=None,
                                   max_concurrent=3, epub=True, timeout=60)
            jobs, discoveries, prepares, grants, polls = {}, {}, {}, [], {}
            job_writes = []
            initializations = []
            servers = []
            interruptions = []
            cleanup_polls = []
            ocr_installs = []

            def call(*argv, **kwargs):
                if argv[0] == "init":
                    initializations.append(argv)
                    return {"inference_profile_id": "reader"}
                if argv[:3] == ("plugin", "dirs", "add"):
                    grants.append(argv)
                if argv[:2] == ("pack", "install") and Path(argv[2]).name == "ocr":
                    ocr_installs.append(argv)
                if argv[:2] == ("query", "find"):
                    filters = json.loads(argv[argv.index("--filter") + 1])
                    if filters["lifecycle_state"]["_in"] == ["pending", "claimed", "processing"]:
                        failed_id = next(run for run, job in jobs.items() if job["book_id"] == "one")
                        self.assertEqual(filters["caused_by_correlation"]["_in"], [failed_id])
                        cleanup_polls.append(failed_id)
                        return {"results": [{"request_id": "failed-book-agent", "interrupt_requested_at":
                                              "2026-10-10T00:00:00Z" if interruptions else None}]}
                    return {"results": []}
                if argv[:2] == ("request", "interrupt"):
                    interruptions.append(argv[2])
                if argv[:2] == ("document", "create"):
                    fields = json.loads(argv[-1])
                    if argv[2] == "ShelfJob":
                        self.assertEqual(fields["remote_ocr"], remote_ocr)
                        job_writes.append(fields["run_id"])
                        jobs[fields["run_id"]] = fields
                    elif argv[2] == "ShelfDiscoveryJob":
                        self.assertEqual(len(jobs), 2)
                        discoveries[fields["run_id"]] = fields
                    else:
                        self.assertIn(fields["run_id"].removesuffix("-readable"), discoveries)
                        self.assertEqual(len(jobs), 2)
                        prepares[fields["run_id"]] = fields
                return {}

            def query(home, collection, run_id, fields):
                if collection == "ShelfJob":
                    return [jobs[run_id]]
                if collection == "ShelfDiscoveryJob":
                    return [discoveries[run_id]] if run_id in discoveries else []
                if collection == "ShelfStructureFailure" and fail_first and jobs[run_id]["book_id"] == "one":
                    return [{"stage": "boundaries", "reason": "ambiguous heading"}]
                if collection == "ShelfSourceReady":
                    return [{"book_id": jobs[run_id]["book_id"]}]
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
                server = Mock(poll=lambda: None)
                servers.append(server)
                return server

            with patch.object(shelf, "call", call), patch.object(shelf, "query", query), \
                 patch.object(shelf, "export", export), patch.object(shelf, "source_capsule", side_effect=lambda home, run_id, fields, directory, endpoint, expected_hashes: {"run_id":run_id,"book_id":fields["book_id"]}), patch.object(shelf, "model", return_value="model"), \
                 patch.object(shelf.subprocess, "Popen", process), patch.object(shelf.time, "sleep", side_effect=RuntimeError("operator pause") if resume else None) as sleeping, \
                 contextlib.redirect_stdout(io.StringIO()):
                if fail_first and not resume:
                    with self.assertRaisesRegex(RuntimeError, "1 of 2 books failed"):
                        shelf.run(args)
                else:
                    if resume:
                        with self.assertRaisesRegex(RuntimeError, "operator pause"):
                            shelf.run(args)
                        args.resume = True
                        if attach:
                            args.attach = True
                            (args.directory / "home").mkdir()
                            runtime_path = args.directory / "home" / "runtime.json"
                            runtime = {"home": str(args.directory / "home"), "graphql": "http://127.0.0.1:50327/api/v0/graphql"}
                            runtime_path.write_text(json.dumps({**runtime, "home": str(root / "wrong")}))
                            with self.assertRaisesRegex(RuntimeError, "original home's local Gents runtime endpoint"):
                                shelf.run(args)
                            runtime_path.write_text(json.dumps({**runtime, "graphql": "http://other-host:50327/api/v0/graphql"}))
                            with self.assertRaisesRegex(RuntimeError, "original home's local Gents runtime endpoint"):
                                shelf.run(args)
                            runtime_path.write_text(json.dumps(runtime))
                        sleeping.side_effect = None
                        if fail_first:
                            with self.assertRaisesRegex(RuntimeError, "1 of 2 books failed"):
                                shelf.run(args)
                        else:
                            shelf.run(args)
                        self.assertEqual(len(servers), 1 if attach else 2)
                        for server in servers:
                            server.terminate.assert_called_once()
                        self.assertEqual(len(initializations), 1)
                        self.assertEqual(len(job_writes), 2)
                        (root / "one.pdf").write_bytes(b"changed source")
                        with self.assertRaisesRegex(RuntimeError, "changed book identities or source bytes"):
                            shelf.run(args)
                    else:
                        shelf.run(args)
            outcomes = json.loads((args.directory / "results.json").read_text())
            self.assertEqual(len(ocr_installs), 1)
            self.assertEqual("remote_ocr=reader" in ocr_installs[0], remote_ocr != "off")
            self.assertEqual([r["status"] for r in outcomes], ["failed", "completed"] if fail_first else ["completed", "completed"])
            if fail_first:
                self.assertIn("ambiguous heading", outcomes[0]["error"])
                self.assertEqual(interruptions, ["failed-book-agent"])
                self.assertEqual(len(cleanup_polls), 2)
            self.assertEqual(len(discoveries), 1 if fail_first else 2)
            self.assertEqual(len(prepares), 1 if fail_first else 2)
            self.assertEqual(list(polls.values()), [2] if fail_first else [2, 2])
            self.assertEqual(len({job["path"] for job in prepares.values()}), 1 if fail_first else 2)
            for job in prepares.values():
                grant = next(g for g in grants if str(g[3]) == job["path"])
                self.assertEqual(grant[grant.index("--access") + 1], "read_write")

    def test_failed_work_cleanup_pages_before_interrupting_and_keeps_existing_latches(self):
        requests = [{"request_id": f"r-{i}", "interrupt_requested_at": None} for i in range(103)]
        requests[7]["interrupt_requested_at"] = "2026-10-10T00:00:00Z"
        offsets, interrupted = [], []
        def call(*argv):
            if argv[:2] == ("query", "find"):
                filters = json.loads(argv[argv.index("--filter") + 1])
                self.assertEqual(filters["caused_by_correlation"]["_in"], ["book", "edition"])
                offset = int(argv[argv.index("--offset") + 1])
                offsets.append(offset)
                self.assertEqual(interrupted, [])
                return {"results": requests[offset:offset + 100]}
            self.assertEqual(argv[:2], ("request", "interrupt"))
            self.assertEqual(argv[-4:], ("--cause", "interrupted", "--output", "json"))
            interrupted.append(argv[2])
        with patch.object(shelf, "call", call):
            self.assertEqual(shelf.interrupt_failed_work("home", ["book", "edition"]), 102)
        self.assertEqual(offsets, [0, 100])
        self.assertNotIn("r-7", interrupted)

    def test_truncated_cleanup_inventory_does_not_interrupt_partial_results(self):
        with patch.object(shelf, "call", return_value={"truncated": True, "results": [{"request_id": "r"}]}) as call:
            with self.assertRaisesRegex(RuntimeError, "inventory was truncated"):
                shelf.interrupt_failed_work("home", ["book"])
        self.assertEqual(call.call_count, 1)

    def test_source_capsule_keeps_ordered_parts_and_refuses_missing_pages(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)
            for name in ["one.pdf","two.pdf"]:
                (root/name).write_bytes(name.encode())
            directory=root/"edition";directory.mkdir()
            fields={"path":str(root),"files":["one.pdf","two.pdf"]}
            rows=[{"source":name,"page":page,"markdown":f"{name} page {page}"}
                  for name in ["two.pdf","one.pdf"] for page in [2,1]]
            manifest=[{"source":"one.pdf","page_count":2},{"source":"two.pdf","page_count":2}]
            def query(home,collection,run_id,fields,*pagination,**kwargs):
                if collection=="ShelfSourceReady":
                    return [{"book_id":"book","sources_json":json.dumps(manifest),"access":"local_only","license":"unknown"}]
                return rows if pagination[1]==0 else []
            with patch.object(shelf,"query",query),patch.object(shelf,"FieldReader"):
                job=shelf.source_capsule("home","run",fields,directory,"http://runtime/mcp")
                raw=(directory/"source-book.json").read_bytes();capsule=json.loads(raw)
                self.assertEqual(shelf.hashlib.sha256(raw).hexdigest(),job["book_hash"])
                self.assertEqual([(p["scan_page"],p["source"],p["page"]) for p in capsule["pages"]],
                                 [(1,"one.pdf",1),(2,"one.pdf",2),(3,"two.pdf",1),(4,"two.pdf",2)])
                for item in capsule["sources"]:
                    self.assertEqual((directory/item["asset"]).read_bytes(),item["source"].encode())
                rows.pop()
                incomplete=root/"incomplete";incomplete.mkdir()
                with self.assertRaisesRegex(RuntimeError,"exactly cover"):
                    shelf.source_capsule("home","run",fields,incomplete,"http://runtime/mcp")
                self.assertEqual(list(incomplete.iterdir()),[])
                rows.append({"source":"one.pdf","page":1,"markdown":"Restored source page"})
                changed=root/"changed";changed.mkdir()
                with self.assertRaisesRegex(RuntimeError,"changed during OCR"):
                    shelf.source_capsule("home","run",fields,changed,"http://runtime/mcp",{"one.pdf":"wrong-source-hash"})

    def test_denied_native_stage_is_reported_without_waiting_for_timeout(self):
        def call(*args):
            filters = json.loads(args[args.index("--filter") + 1])
            self.assertEqual(filters["caused_by_correlation"]["_in"], ["book", "edition"])
            self.assertEqual(filters["lifecycle_state"]["_in"], ["denied", "failed"])
            return {"results": [{"callback_id": "shelf-prepare", "lifecycle_state": "denied",
                                 "error": "export folder is read-only"}]}
        with patch.object(shelf, "call", call):
            with self.assertRaisesRegex(RuntimeError, "export folder is read-only"):
                shelf.check_stage_failures("home", ["book", "edition"])

    def test_terminal_agent_failures_are_reported_with_canonical_states(self):
        for state in ["failed", "dead", "interrupted"]:
            with self.subTest(state=state):
                def call(*args):
                    filters = json.loads(args[args.index("--filter") + 1])
                    self.assertEqual(filters["caused_by_correlation"]["_in"], ["book", "edition"])
                    collection = args[args.index("--collection") + 1]
                    if collection == "CallbackInvocation":
                        self.assertEqual(filters["lifecycle_state"]["_in"], ["denied", "failed"])
                        return {"results": []}
                    self.assertEqual(filters["lifecycle_state"]["_in"], ["failed", "dead", "interrupted"])
                    return {"results": [{"behavior_id": "shelf-toc-find", "lifecycle_state": state,
                                         "failure_reason": "stopped discovery"}]}
                with patch.object(shelf, "call", call):
                    with self.assertRaisesRegex(RuntimeError, "stopped discovery"):
                        shelf.check_stage_failures("home", ["book", "edition"])

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
