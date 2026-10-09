#!/usr/bin/env python3
"""Run Shelf through Gents' document owners or export a persisted structured book."""
import argparse
import datetime
import functools
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
GENTS = os.environ.get("GENTS", "gents")


def call(*args, json_output=True):
    result = subprocess.run([GENTS, *map(str, args)], check=True, text=True, stdout=subprocess.PIPE)
    return json.loads(result.stdout) if json_output else result.stdout


def source_fields(sources, base=Path.cwd()):
    inputs = [(base / Path(p).expanduser()).resolve(strict=True) for p in sources]
    if not inputs or any(not p.is_file() for p in inputs) or len(set(inputs)) != len(inputs):
        raise RuntimeError("sources must be distinct existing files")
    if len({p.parent for p in inputs}) != 1:
        raise RuntimeError("multi-part sources must be in the same folder; list them in reading order")
    fields = {"path": str(inputs[0] if len(inputs) == 1 else inputs[0].parent),
              "ocr": "auto", "remote_ocr": "off", "figure_images": False}
    if len(inputs) > 1:
        fields["files"] = [p.name for p in inputs]
    return fields


def submit_batch(args):
    """Submit independent jobs to one existing runtime; triggers own all scheduling."""
    manifest = args.manifest.expanduser().resolve(strict=True)
    items = json.loads(manifest.read_text())
    if not isinstance(items, list) or not items:
        raise RuntimeError("batch manifest must be a nonempty array of {run_id, sources} objects")
    jobs = {}
    for item in items:
        if (not isinstance(item, dict) or not {"run_id", "sources"} <= set(item) or set(item) - {"run_id", "sources", "book_id", "access", "license"}
                or not isinstance(item["run_id"], str) or not item["run_id"].strip()
                or ("book_id" in item and (not isinstance(item["book_id"], str) or not item["book_id"].strip()))
                or not isinstance(item["sources"], list)
                or not all(isinstance(p, str) and p for p in item["sources"])):
            raise RuntimeError("each batch item needs a nonempty run_id, sources array and optional nonempty book_id")
        run_id = item["run_id"]
        if run_id in jobs:
            raise RuntimeError(f"duplicate batch run_id: {run_id}")
        jobs[run_id] = dict(run_id=run_id, **source_fields(item["sources"], manifest.parent))
        for key in ["book_id","access","license"]:
            if item.get(key): jobs[run_id][key] = item[key]
        jobs[run_id]["remote_ocr"] = getattr(args, "remote_ocr", "off")
    pending = []
    # Inspect the whole batch before submitting, so a conflicting ID cannot partially enqueue it.
    for run_id, fields in jobs.items():
        existing = query(args.home, "ShelfJob", run_id, ["path", "files", "ocr", "remote_ocr", "figure_images", "book_id", "access", "license"])
        expected = {k: v for k, v in fields.items() if k != "run_id"}
        if existing:
            actual = {k: v for k, v in existing[0].items() if not (k in {"files", "book_id", "access", "license"} and not v)}
            if len(existing) != 1 or actual != expected:
                raise RuntimeError(f"run_id {run_id} already belongs to a different job; use a new ID")
        else:
            pending.append(fields)
    for fields in pending:
        call("document", "create", "ShelfJob", "--home", args.home, "--json", json.dumps(fields))
        print(json.dumps({"run_id": fields["run_id"], "status": "submitted"}), flush=True)
    print(json.dumps({"submitted": len(pending), "already_submitted": len(jobs) - len(pending)}), flush=True)


PASSAGE_FIELDS = ["record_id", "book_id", "edition_id", "passage_id", "chapter_title",
                  "title", "author", "language", "access", "license", "status", "source_type", "revision",
                  "source_hash", "source_hash_scope", "text_hash", "text", "preview",
                  "locator_summary", "source_spans_json", "epub_href"]
SEARCH_FIELDS = ["book_id", "edition_id", "passage_id", "title", "author", "chapter_title",
                 "language", "access", "status", "source_type", "preview", "locator_summary"]


def search_library(args):
    if not args.text.strip() or not 1 <= args.limit <= 100:
        raise RuntimeError("search needs nonempty terms and a limit from 1 to 100")
    conditions = {"status": {"_in": ["reviewed", "source_text"]}}
    for key in ["book_id", "language"]:
        if getattr(args, key):
            conditions[key] = {"_eq": getattr(args, key)}
    if args.access != "all":
        conditions["access"] = {"_eq": args.access}
    editions = []
    offset = 0
    page_size = 100
    catalog_filter = {"status": {"_in": ["reviewed", "source_text"]}}
    if args.book_id:
        catalog_filter["book_id"] = {"_eq": args.book_id}
    while True:
        result = call("query", "find", "--home", args.home, "--collection", "ShelfLibraryEdition",
                      "--filter", json.dumps(catalog_filter), "--field", "book_id", "--field", "edition_id",
                      "--field", "modified", "--field", "status", "--field", "language", "--field", "access", "--limit", page_size, "--offset", offset)
        if result.get("truncated"):
            if page_size == 1:
                raise RuntimeError("one edition exceeds the catalog output limit; narrow the book filter")
            page_size = max(1, page_size // 2)
            continue  # reread this offset, so a truncated tail is never lost
        editions.extend(result["results"])
        if len(result["results"]) < page_size:
            break
        offset += page_size
    warnings = []
    if args.edition_id:
        candidates = [e for e in editions if e["edition_id"] == args.edition_id]
    else:
        latest = {}
        def rank(edition):
            stamp = edition["modified"]
            try:
                parsed = datetime.datetime.strptime(stamp, "%Y-%m-%dT%H:%M:%SZ")
            except (ValueError, TypeError) as error:
                raise RuntimeError("invalid edition timestamp; select an explicit edition") from error
            return (edition["status"] == "reviewed", parsed)
        grouped = {}
        for edition in editions:
            grouped.setdefault(edition["book_id"], []).append(edition)
        for book, choices in grouped.items():
            best = max(map(rank, choices))
            winners = [edition for edition in choices if rank(edition) == best]
            if len({edition["edition_id"] for edition in winners}) != 1:
                if args.book_id:
                    raise RuntimeError(f"ambiguous current edition for {book}; select --edition-id or supply distinct revision timestamps")
                warnings.append({"book_id":book,"reason":"ambiguous current edition; select --edition-id","edition_ids":[e["edition_id"] for e in winners]})
                continue
            latest[book] = winners[0]
        candidates = list(latest.values())
    selected = [e["edition_id"] for e in candidates
                if (args.access == "all" or e["access"] == args.access)
                and (not args.language or e["language"] == args.language)]
    if not selected:
        return {"ranking": "bm25", "results": [], "returned_count": 0, "warnings": warnings}
    conditions.update(edition_id={"_in": selected}, _alias={"_score": {"_gt": 0}})
    command = ["query", "search", "--home", args.home, "--collection", "ShelfLibraryPassage",
               "--text", args.text, "--search-field", "text", "--filter", json.dumps(conditions), "--limit", args.limit]
    for name in SEARCH_FIELDS:
        command += ["--field", name]
    result = call(*command)
    if warnings: result["warnings"] = warnings
    if result.get("truncated"):
        result["next"] = "Narrow the query; open-passage retrieves exact full text after selecting a citation."
    return result


def open_passage(args):
    filters = {name: {"_eq": getattr(args, name)} for name in ["book_id", "edition_id", "passage_id"]}
    command = ["query", "find", "--home", args.home, "--collection", "ShelfLibraryPassage",
               "--filter", json.dumps(filters), "--limit", 2, "--field", "_docID"]
    for name in PASSAGE_FIELDS:
        command += ["--field", name]
    result = call(*command)
    if len(result["results"]) != 1:
        raise RuntimeError("citation does not identify exactly one stored passage")
    passage = result["results"][0]
    if result.get("truncated"):
        if not args.mcp_endpoint or not result.get("field_recovery"):
            raise RuntimeError("passage is truncated; supply --mcp-endpoint to read complete text")
        reader = FieldReader(args.mcp_endpoint)
        for recovery in result["field_recovery"]:
            passage[recovery["field"]] = reader.read("ShelfLibraryPassage", args.edition_id, recovery)
    import hashlib
    if hashlib.sha256(passage["text"].encode()).hexdigest() != passage["text_hash"]:
        raise RuntimeError("stored passage text no longer matches its citation hash")
    passage.pop("_docID", None)
    return passage


class FieldReader:
    """Recover long strings through Gents' canonical query MCP surface."""

    def __init__(self, endpoint):
        self.endpoint = endpoint
        self.session = None
        self.sequence = 0
        self.rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                               "clientInfo": {"name": "shelf-export", "version": "1"}})
        self.rpc("notifications/initialized", {}, notification=True)

    def rpc(self, method, params, notification=False):
        self.sequence += 1
        body = {"jsonrpc": "2.0", "method": method, "params": params}
        if not notification:
            body["id"] = self.sequence
        headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
        if self.session:
            headers["Mcp-Session-Id"] = self.session
        request = urllib.request.Request(self.endpoint, json.dumps(body).encode(), headers)
        with urllib.request.urlopen(request, timeout=60) as response:
            self.session = response.headers.get("Mcp-Session-Id", self.session)
            raw = response.read().decode()
        if notification:
            return None
        messages = [json.loads(line[5:].strip()) for line in raw.splitlines() if line.startswith("data:") and line[5:].strip()] if raw.startswith("event:") or raw.startswith("data:") else [json.loads(raw)]
        result = next(m for m in messages if m.get("id") == self.sequence)
        if "error" in result:
            raise RuntimeError(str(result["error"]))
        return result["result"]

    def read(self, collection, run_id, recovery):
        page = dict(recovery["field_page"])
        parts = []
        while True:
            result = self.rpc("tools/call", {"name": "query", "arguments": {
                "argv": ["find"], "collection": collection, "options": {
                    "fields": [page["field"]], "filter": {"run_id": {"_eq": run_id}}, "field_page": page}}})
            if result.get("isError"):
                raise RuntimeError(str(result))
            value = json.loads("".join(c["text"] for c in result["content"] if c["type"] == "text"))["field_page"]
            parts.append(value["text"])
            if value["complete"]:
                return "".join(parts)
            page.update(offset_bytes=value["next_offset_bytes"], expected_hash=value["value_hash"])


def query(home, collection, run_id, fields, limit=1000, offset=0, reader=None):
    args = ["query", "find", "--home", home, "--collection", collection,
            "--filter", json.dumps({"run_id": {"_eq": run_id}}), "--limit", limit, "--offset", offset]
    for field in ["_docID", *fields]:
        args += ["--field", field]
    value = call(*args)
    if value.get("truncated"):
        recoveries = value.get("field_recovery", [])
        if not reader or not recoveries:
            raise RuntimeError(f"{collection} query was truncated; export needs --mcp-endpoint for field recovery")
        rows = {row["_docID"]: row for row in value["results"]}
        for recovery in recoveries:
            rows[recovery["doc_id"]][recovery["field"]] = reader.read(collection, run_id, recovery)
    for row in value["results"]:
        row.pop("_docID", None)
    return value["results"]


def source_capsule(home, run_id, fields, directory, endpoint, expected_hashes=None):
    read = functools.partial(query, reader=FieldReader(endpoint))
    receipts = read(home, "ShelfSourceReady", run_id, ["book_id", "sources_json", "access", "license"])
    if len(receipts) != 1:
        raise RuntimeError("expected one complete source readiness receipt")
    receipt = receipts[0]
    manifest = json.loads(receipt["sources_json"])
    pages, offset = {}, 0
    while True:
        rows = read(home, "ShelfPage", run_id, ["source", "page", "markdown"], 10, offset)
        if not rows:
            break
        for row in rows:
            key = (row["source"], row["page"])
            if key in pages:
                raise RuntimeError(f"duplicate source page: {key}")
            pages[key] = row
        offset += len(rows)
    expected = {(item["source"], n) for item in manifest for n in range(1, item["page_count"] + 1)}
    if not manifest or len({item["source"] for item in manifest}) != len(manifest) or set(pages) != expected:
        raise RuntimeError("source capsule pages do not exactly cover the OCR manifest")
    input_path = Path(fields["path"])
    folder = input_path if "files" in fields else input_path.parent
    allowed = set(fields.get("files", [input_path.name]))
    sources, ordered = [], []
    for item in manifest:
        name = item["source"]
        if name not in allowed or Path(name).name != name or item["page_count"] <= 0:
            raise RuntimeError("source manifest is outside the submitted PDF inputs")
        original = folder / name
        asset = f"source-{len(sources):04}.pdf"
        target = directory / asset
        with original.open("rb") as src, target.open("xb") as dst:
            digest = hashlib.sha256()
            while data := src.read(1024 * 1024):
                digest.update(data)
                dst.write(data)
        if expected_hashes is not None and expected_hashes.get(name) != digest.hexdigest():
            raise RuntimeError(f"source PDF changed during OCR: {name}; start a new book run")
        sources.append(dict(item, asset=asset, sha256=digest.hexdigest()))
        for physical in range(1, item["page_count"] + 1):
            ordered.append(dict(pages[(name, physical)], scan_page=len(ordered) + 1))
    capsule = {"run_id": run_id, "book_id": receipt["book_id"], "sources": sources,
               "pages": ordered, "access": receipt["access"], "license": receipt["license"]}
    raw = json.dumps(capsule, ensure_ascii=False).encode()
    with (directory / "source-book.json").open("xb") as stream:
        stream.write(raw)
    return {"run_id": run_id, "book_id": receipt["book_id"], "path": str(directory),
            "book_file": "source-book.json", "book_hash": hashlib.sha256(raw).hexdigest(),
            "stage": "start", "stage_job_id": run_id + ":start", "access": receipt["access"], "license": receipt["license"]}


def export(home, run_id, output, mcp_endpoint=None):
    read = functools.partial(query, reader=FieldReader(mcp_endpoint) if mcp_endpoint else None)
    books = read(home, "ShelfBook", run_id, ["book_id", "title", "author", "language", "access", "license", "metadata_json", "source_book_file", "source_book_hash", "source_manifest", "page_count", "chapter_count", "review_notes"])
    if len(books) != 1:
        raise RuntimeError(f"expected one assembled book for {run_id}, found {len(books)}")
    book = books[0]
    chunks = read(home, "ShelfChunk", run_id, ["chunk", "source", "pages"])
    extracts = read(home, "ShelfExtract", run_id, ["chunk", "source", "complete", "error", "cursor", "warnings"])
    chunk_ids = {(row["chunk"], row["source"]) for row in chunks}
    extraction_keys = {(row["chunk"], row["source"]) for row in extracts}
    if (not chunks or len(chunk_ids) != len(chunks) or len(extraction_keys) != len(extracts)
            or chunk_ids != extraction_keys
            or any(not row["complete"] or row.get("error") or row.get("cursor") for row in extracts)):
        raise RuntimeError("source extraction is incomplete or inconsistent; inspect ShelfExtract")
    reports = read(home, "ShelfStructureReport", run_id, ["covered_pages", "page_count", "chapter_count"])
    if len(reports) != 1 or reports[0]["covered_pages"] != book["page_count"]:
        raise RuntimeError("missing or inconsistent structure receipt")
    chapters = read(home, "ShelfChapter", run_id, ["chapter_key", "toc_entry_id", "toc_title", "entry_number", "level_name", "parent_key", "audio_include", "audio_include_reasoning", "owned_end_page", "sequence", "title", "level", "matter_type", "content_type", "start_page", "end_page", "source_ranges_json", "review_notes"])
    if len(chapters) != book["chapter_count"]:
        raise RuntimeError("chapter collection does not match the book receipt")
    chapters.sort(key=lambda c: c["sequence"])
    sources = json.loads(book.pop("source_manifest"))
    expected = {(s["source"], n) for s in sources for n in range(1, s["page_count"] + 1)}
    pages = {}
    offset = 0
    while True:
        rows = read(home, "ShelfPage", run_id, ["source", "page", "markdown"], 10, offset)
        if not rows:
            break
        for row in rows:
            key = (row["source"], row["page"])
            if key in pages:
                raise RuntimeError(f"duplicate source page: {key}")
            pages[key] = row
        offset += len(rows)
    if set(pages) != expected or len(expected) != book["page_count"]:
        raise RuntimeError("persisted pages do not exactly cover the source manifest")
    used = set()
    for chapter in chapters:
        chapter["source_ranges"] = json.loads(chapter.pop("source_ranges_json"))
        chapter["pages"] = []
        for span in chapter["source_ranges"]:
            for n in range(span["start_page"], span["end_page"] + 1):
                key = (span["source"], n)
                if key in used or key not in pages:
                    raise RuntimeError(f"overlapping or missing chapter page: {key}")
                used.add(key)
                chapter["pages"].append(pages[key])
        if len(chapter["pages"]) != max(0, chapter["owned_end_page"] - chapter["start_page"] + 1):
            raise RuntimeError("chapter source ranges disagree with cumulative scan positions")
    if used != expected:
        raise RuntimeError("chapters leave source pages unassigned")
    book["metadata"] = json.loads(book.pop("metadata_json"))
    book["source_evidence"] = {"run_id":run_id,"book_file":book.pop("source_book_file"),"book_hash":book.pop("source_book_hash")}
    book.update(run_id=run_id, sources=sources, chapters=chapters, extraction_warnings=[{ "source": row["source"], "chunk": row["chunk"], "warnings": row["warnings"] } for row in extracts if row.get("warnings")])
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x") as stream:
        json.dump(book, stream, indent=2, ensure_ascii=False)
        stream.write("\n")
    print(json.dumps({"path": str(output), "title": book["title"], "pages": len(pages), "chapters": len(chapters)}))


def model(endpoint):
    with urllib.request.urlopen(endpoint.rstrip("/") + "/models", timeout=15) as response:
        models = json.load(response)["data"]
    if len(models) != 1:
        raise RuntimeError(f"{endpoint} must expose one model; found {len(models)}")
    return models[0]["id"]



def check_stage_failures(home, correlations):
    for collection, identity, error in [("CallbackInvocation", "callback_id", "error"),
                                         ("AgentRequest", "behavior_id", "failure_reason")]:
        result = call("query", "find", "--home", home, "--collection", collection,
                      "--filter", json.dumps({"caused_by_correlation": {"_in": correlations},
                                               "lifecycle_state": {"_in": ["denied", "failed"]}}),
                      "--field", identity, "--field", "lifecycle_state", "--field", error, "--limit", 10)
        if result["results"]:
            raise RuntimeError(f"{collection} stage failed; persisted diagnostics: {result['results']}")


def input_books(args):
    if args.manifest:
        if args.sources or args.book_id:
            raise RuntimeError("--manifest supplies sources and book IDs; do not also pass them on the command line")
        manifest = args.manifest.expanduser().resolve(strict=True)
        items = json.loads(manifest.read_text())
        base = manifest.parent
        if not isinstance(items, list) or not items:
            raise RuntimeError("manifest must be a nonempty array of {book_id, sources}")
    else:
        items = [{"book_id": args.book_id, "sources": args.sources}]
        base = Path.cwd()
    books, seen = [], set()
    for item in items:
        if not isinstance(item, dict) or set(item) - {"book_id", "sources", "access", "license"}:
            raise RuntimeError("manifest items contain book_id, sources and optional access/license")
        book_id = item.get("book_id")
        if args.manifest and (not isinstance(book_id, str) or not book_id.strip() or book_id in seen):
            raise RuntimeError("manifest book IDs must be nonempty and distinct")
        seen.add(book_id)
        sources = item.get("sources")
        if not isinstance(sources, list) or not sources:
            raise RuntimeError("each book needs an ordered, nonempty sources list")
        fields = source_fields(sources, base)
        fields.update(remote_ocr=args.remote_ocr, access=item.get("access", args.access),
                      license=item.get("license", args.license))
        if book_id:
            fields["book_id"] = book_id
        input_path = Path(fields["path"])
        folder = input_path if "files" in fields else input_path.parent
        input_hashes = {}
        for name in fields.get("files", [input_path.name]):
            with (folder / name).open("rb") as stream:
                input_hashes[name] = hashlib.file_digest(stream, "sha256").hexdigest()
        books.append({"fields": fields, "input_hashes": input_hashes, "edition_id": None, "done": False})
    return books


def run(args):
    books = input_books(args)
    directory = args.directory.expanduser().resolve()
    directory.mkdir(parents=True, exist_ok=False)
    home = directory / "home"
    first_model = model(args.endpoint)
    initialized = call("init", "--home", home, "--agent-name", "shelf", "--backend-preset", "vllm", "--inference-url", args.endpoint,
                       "--model-name", first_model, "--max-concurrent", args.max_concurrent, "--tool-root", directory)
    reader = initialized["inference_profile_id"]
    librarian = reader
    if args.structure_endpoint:
        config_dir = directory / "config"
        call("config", "export", "--home", home, "--root", config_dir, json_output=False)
        config = json.loads((config_dir / "pack_config.json").read_text())
        backend = config["inference_backends"][0].copy()
        backend.update(backend_id=backend["backend_id"] + "-structure", endpoint=args.structure_endpoint, name="Shelf structure")
        profile = config["inference_profiles"][0].copy()
        profile.update(profile_id=profile["profile_id"] + "-structure", backend_id=backend["backend_id"], model_name=model(args.structure_endpoint))
        for name, doc, command in [("backend", backend, "backend"), ("profile", profile, "profile")]:
            path = directory / (name + ".json")
            path.write_text(json.dumps(doc))
            call("config", command, "set", "--home", home, "--file", path)
        librarian = profile["profile_id"]
    ocr_slots = ["document_reader=" + reader]
    if args.remote_ocr != "off":
        ocr_slots.append("remote_ocr=" + reader)
    for pack, slots in [("ocr", ocr_slots), ("shelf", ["reader=" + reader, "librarian=" + librarian, "page_vision=" + reader])]:
        source = ROOT / "packs/gents" / pack
        call("pack", "build", source, "--out", directory / (pack + ".pack"))
        install = ["pack", "install", source, "--home", home, "--grant-authority"]
        for slot in slots:
            install += ["--inference-slot", slot]
        call(*install)
    source_folders = {Path(book["fields"]["path"]) if "files" in book["fields"]
                      else Path(book["fields"]["path"]).parent for book in books}
    for folder in sorted(source_folders):
        call("plugin", "dirs", "add", folder, "--home", home, json_output=False)
    run_prefix = "shelf-" + datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S%f")
    for i, book in enumerate(books):
        book["fields"]["run_id"] = run_prefix if len(books) == 1 else f"{run_prefix}-{i+1:02}"
        edition_dir = directory / ("edition" if len(books) == 1 else f"edition-{i+1:02}")
        edition_dir.mkdir()
        book["directory"] = edition_dir
        call("plugin", "dirs", "add", edition_dir, "--access", "read_write", "--home", home, json_output=False)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    (directory / "run.json").write_text(json.dumps({"run_id": run_prefix, "home": str(home),
        "books": [{"run_id": b["fields"]["run_id"], "book_id": b["fields"].get("book_id"),
                   "directory": str(b["directory"]), "input_hashes":b["input_hashes"]} for b in books]}))
    with (directory / "server.log").open("w") as log:
        server = subprocess.Popen([GENTS, "server", "--home", str(home), "--http-port", str(port), "--p2p-transport", "none", "--no-codex-shim", "--enable-mcp", *[arg for collection in ["ShelfBook", "ShelfChapter", "ShelfPage", "ShelfExtract", "ShelfSourceReady"] for arg in ["--mcp-query-collection", collection]]], stdout=log, stderr=log, cwd=directory)
        try:
            deadline = time.monotonic() + args.timeout
            ready = time.monotonic() + 60
            while True:
                if server.poll() is not None:
                    raise RuntimeError("runtime exited; inspect server.log")
                if "gents server is running" in (directory / "server.log").read_text():
                    break
                if time.monotonic() > ready:
                    raise RuntimeError("runtime did not become ready; inspect server.log")
                time.sleep(1)
            for book in books:
                call("document", "create", "ShelfJob", "--home", home, "--json", json.dumps(book["fields"]))
                print(f"Shelf run {book['fields']['run_id']}; persisted state: {home}", flush=True)
            while time.monotonic() < deadline:
                if server.poll() is not None:
                    raise RuntimeError("runtime exited; inspect server.log")
                for book in books:
                    if book["done"]:
                        continue
                    run_id = book["fields"]["run_id"]
                    edition_id = book["edition_id"]
                    edition_dir = book["directory"]
                    try:
                        structure_failures = query(home, "ShelfStructureFailure", run_id, ["stage", "reason"])
                        if structure_failures:
                            raise RuntimeError(f"book structure needs review; preserved findings: {structure_failures}")
                        if not book.get("discovery_started") and query(home, "ShelfSourceReady", run_id, ["book_id"]):
                            discovery = source_capsule(home, run_id, book["fields"], edition_dir, f"http://127.0.0.1:{port}/mcp", book["input_hashes"])
                            call("document", "create", "ShelfDiscoveryJob", "--home", home, "--json", json.dumps(discovery))
                            book["discovery_started"] = True
                            print(f"Finding contents and verifying boundaries for {run_id}", flush=True)
                        check_stage_failures(home, [run_id] + ([edition_id] if edition_id else []))
                        failed = [r for r in query(home,"ShelfExtract",run_id,["chunk","extraction_state","error"]) if r["extraction_state"]=="failed"]
                        if failed:
                            raise RuntimeError(f"source extraction failed; preserved diagnostic receipts: {failed}")
                        if edition_id is None and book.get("discovery_started") and query(home, "ShelfStructureReport", run_id, ["book_id"]):
                            structured = edition_dir / "structured-book.json"
                            export(home, run_id, structured, f"http://127.0.0.1:{port}/mcp")
                            if not args.epub:
                                book["done"] = True
                                continue
                            structured_book = json.loads(structured.read_text())
                            edition_id = run_id + "-readable"
                            book["edition_id"] = edition_id
                            call("document", "create", "ShelfPrepareJob", "--home", home, "--json", json.dumps({
                                "run_id":edition_id,"book_id":structured_book["book_id"],"path":str(edition_dir),
                                "structured":structured.name,"output":"book.epub",
                                "modified":datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")}))
                            print(f"Reviewing edition {edition_id}", flush=True)
                        if edition_id:
                            failures = query(home, "ShelfEditionFailure", edition_id, ["chunk_ref", "error"])
                            if failures:
                                raise RuntimeError(f"edition needs review; persisted failures: {failures}")
                            if query(home, "ShelfLibraryEdition", edition_id, ["edition_id"]):
                                print(json.dumps({"edition_id":edition_id,"epub":str(edition_dir/"book.epub")}),flush=True)
                                book["done"] = True
                    except RuntimeError as error:
                        book["error"] = str(error)
                        book["done"] = True
                        print(json.dumps({"run_id": run_id, "book_id": book["fields"].get("book_id"),
                                          "status": "failed", "error": str(error)}), flush=True)
                outcomes = [{"run_id": b["fields"]["run_id"], "book_id": b["fields"].get("book_id"),
                             "status": "failed" if b.get("error") else "completed" if b["done"] else "running",
                             "edition_id": b["edition_id"], "directory": str(b["directory"]),
                             "error": b.get("error")} for b in books]
                snapshot = directory / "results.json.tmp"
                snapshot.write_text(json.dumps(outcomes, indent=2) + "\n")
                snapshot.replace(directory / "results.json")
                if all(book["done"] for book in books):
                    failures = [b for b in books if b.get("error")]
                    if failures:
                        raise RuntimeError(f"{len(failures)} of {len(books)} books failed; inspect results.json and persisted diagnostics")
                    return
                time.sleep(10)
            raise RuntimeError("timed out; persisted state is retained in the run home")
        finally:
            server.terminate()
            try:
                server.wait(timeout=30)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    export_parser = sub.add_parser("export")
    export_parser.add_argument("--home", type=Path, required=True)
    export_parser.add_argument("--run-id", required=True)
    export_parser.add_argument("--output", type=Path, required=True)
    export_parser.add_argument("--mcp-endpoint", help="Local Gents /mcp endpoint for complete long-field reads")
    batch_parser = sub.add_parser("submit-batch", help="Enqueue independent books on one running, configured Gents home")
    batch_parser.add_argument("--home", type=Path, required=True)
    batch_parser.add_argument("--manifest", type=Path, required=True, help="JSON array of {run_id, sources}; paths relative to this file")
    batch_parser.add_argument("--remote-ocr", choices=["off", "auto", "force"], default="off",
                              help="Vision fallback; auto/force require the installed OCR pack's remote_ocr slot")
    search_parser = sub.add_parser("search", help="BM25 over the latest reviewed edition of each book")
    search_parser.add_argument("--home", type=Path, required=True)
    search_parser.add_argument("--text", required=True)
    search_parser.add_argument("--book-id")
    search_parser.add_argument("--edition-id", help="Select an explicit historical edition")
    search_parser.add_argument("--language")
    search_parser.add_argument("--access", choices=["open", "local_only", "restricted", "all"], default="open")
    search_parser.add_argument("--limit", type=int, default=10)
    open_parser = sub.add_parser("open-passage", help="Open an exact citation and verify its text hash")
    for name in ["book-id", "edition-id", "passage-id"]:
        open_parser.add_argument("--" + name, required=True)
    open_parser.add_argument("--home", type=Path, required=True)
    open_parser.add_argument("--mcp-endpoint")
    run_parser = sub.add_parser("run")
    run_parser.add_argument("--access", choices=["open","local_only","restricted"], default="local_only")
    run_parser.add_argument("--license", default="unknown")
    run_parser.add_argument("--epub", action="store_true", help="Continue through text review, EPUB export and native indexing")
    run_parser.add_argument("--book-id", help="Stable work ID shared with source-text intake; defaults to the run ID")
    run_parser.add_argument("sources", type=Path, nargs="*")
    run_parser.add_argument("--manifest", type=Path, help="JSON array of {book_id, sources}; run all books in one shared runtime")
    run_parser.add_argument("--endpoint", required=True, help="OpenAI-compatible /v1 endpoint exposing one model")
    run_parser.add_argument("--structure-endpoint", help="Optional separate endpoint for contents discovery, metadata and classification")
    run_parser.add_argument("--directory", type=Path, required=True, help="New directory for the isolated home and structured book")
    run_parser.add_argument("--timeout", type=int, default=7200)
    run_parser.add_argument("--max-concurrent", type=int, default=3, help="Maximum simultaneous requests per backend")
    run_parser.add_argument("--remote-ocr", choices=["off", "auto", "force"], default="off",
                            help="Bind the reader endpoint for vision OCR: auto tries bundled OCR first; force checks every scanned page")
    args = parser.parse_args()
    try:
        if args.command == "export":
            export(args.home, args.run_id, args.output, args.mcp_endpoint)
        elif args.command == "submit-batch":
            submit_batch(args)
        elif args.command == "search":
            print(json.dumps(search_library(args), ensure_ascii=False, indent=2))
        elif args.command == "open-passage":
            print(json.dumps(open_passage(args), ensure_ascii=False, indent=2))
        else:
            if args.max_concurrent < 1:
                raise RuntimeError("--max-concurrent must be positive")
            run(args)
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"shelf: {error}\n")


if __name__ == "__main__":
    main()
