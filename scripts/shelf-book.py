#!/usr/bin/env python3
"""Run Shelf through Gents' document owners or export a persisted structured book."""
import argparse
import datetime
import functools
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


def export(home, run_id, output, mcp_endpoint=None):
    read = functools.partial(query, reader=FieldReader(mcp_endpoint) if mcp_endpoint else None)
    books = read(home, "ShelfBook", run_id, ["book_id", "title", "author", "language", "source_manifest", "page_count", "chapter_count", "review_notes"])
    if len(books) != 1:
        raise RuntimeError(f"expected one assembled book for {run_id}, found {len(books)}")
    book = books[0]
    chunks = read(home, "ShelfChunk", run_id, ["chunk", "source", "pages"])
    extracts = read(home, "ShelfExtract", run_id, ["chunk", "source", "complete", "error", "cursor", "warnings"])
    chunk_keys = {(row["chunk"], row["source"]) for row in chunks}
    extraction_keys = {(row["chunk"], row["source"]) for row in extracts}
    if (not chunks or len(chunk_keys) != len(chunks) or len(extraction_keys) != len(extracts)
            or chunk_keys != extraction_keys
            or any(not row["complete"] or row.get("error") or row.get("cursor") for row in extracts)):
        raise RuntimeError("source extraction is incomplete or inconsistent; inspect ShelfExtract")
    reports = read(home, "ShelfStructureReport", run_id, ["covered_pages", "page_count", "chapter_count"])
    if len(reports) != 1 or reports[0]["covered_pages"] != book["page_count"]:
        raise RuntimeError("missing or inconsistent structure receipt")
    chapters = read(home, "ShelfChapter", run_id, ["sequence", "title", "level", "matter_type", "content_type", "start_page", "end_page", "source_ranges_json", "review_notes"])
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
        if len(chapter["pages"]) != chapter["end_page"] - chapter["start_page"] + 1:
            raise RuntimeError("chapter source ranges disagree with cumulative scan positions")
    if used != expected:
        raise RuntimeError("chapters leave source pages unassigned")
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


def run(args):
    inputs = [p.expanduser().resolve(strict=True) for p in args.sources]
    if any(not p.is_file() for p in inputs) or len(set(inputs)) != len(inputs):
        raise RuntimeError("sources must be distinct existing files")
    if len({p.parent for p in inputs}) != 1:
        raise RuntimeError("multi-part sources must be in the same folder; list them in reading order")
    directory = args.directory.expanduser().resolve()
    directory.mkdir(parents=True, exist_ok=False)
    home = directory / "home"
    first_model = model(args.endpoint)
    initialized = call("init", "--home", home, "--agent-name", "shelf", "--backend-preset", "vllm", "--inference-url", args.endpoint,
                       "--model-name", first_model, "--max-concurrent", "3", "--tool-root", directory)
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
    for pack, slots in [("ocr", ["document_reader=" + reader]), ("shelf", ["reader=" + reader, "librarian=" + librarian])]:
        source = ROOT / "packs/gents" / pack
        call("pack", "build", source, "--out", directory / (pack + ".pack"))
        install = ["pack", "install", source, "--home", home, "--grant-authority"]
        for slot in slots:
            install += ["--inference-slot", slot]
        call(*install)
    call("plugin", "dirs", "add", inputs[0].parent, "--home", home, json_output=False)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    run_id = "shelf-" + datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S%f")
    (directory / "run.json").write_text(json.dumps({"run_id": run_id, "home": str(home)}))
    with (directory / "server.log").open("w") as log:
        server = subprocess.Popen([GENTS, "server", "--home", str(home), "--http-port", str(port), "--p2p-transport", "none", "--no-codex-shim", "--enable-mcp", *[arg for collection in ["ShelfBook", "ShelfChapter", "ShelfPage", "ShelfExtract"] for arg in ["--mcp-query-collection", collection]]], stdout=log, stderr=log, cwd=directory)
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
            fields = {"run_id": run_id, "path": str(inputs[0] if len(inputs) == 1 else inputs[0].parent), "ocr": "auto", "remote_ocr": "off", "figure_images": False}
            if len(inputs) > 1:
                fields["files"] = [p.name for p in inputs]
            call("document", "create", "ShelfJob", "--home", home, "--json", json.dumps(fields))
            print(f"Shelf run {run_id}; persisted state: {home}", flush=True)
            while time.monotonic() < deadline:
                if server.poll() is not None:
                    raise RuntimeError("runtime exited; inspect server.log")
                if query(home, "ShelfStructureReport", run_id, ["book_id"]):
                    export(home, run_id, directory / "structured-book.json", f"http://127.0.0.1:{port}/mcp")
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
    run_parser = sub.add_parser("run")
    run_parser.add_argument("sources", type=Path, nargs="+")
    run_parser.add_argument("--endpoint", required=True, help="OpenAI-compatible /v1 endpoint exposing one model")
    run_parser.add_argument("--structure-endpoint", help="Optional separate endpoint for outline and verification")
    run_parser.add_argument("--directory", type=Path, required=True, help="New directory for the isolated home and structured book")
    run_parser.add_argument("--timeout", type=int, default=7200)
    args = parser.parse_args()
    try:
        if args.command == "export":
            export(args.home, args.run_id, args.output, args.mcp_endpoint)
        else:
            run(args)
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"shelf: {error}\n")


if __name__ == "__main__":
    main()
