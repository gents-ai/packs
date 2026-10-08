#!/usr/bin/env python3
"""Stage citable source text into Shelf's library and resume a local DefraDB load.

This is the direct-text lane for format-normalized source units. It never calls OCR
or a model. Input is a JSONL work catalog and JSONL units with stable locators;
the producer may parse PDF text layers, EPUB spines, HTML/TEI elements, or ancient
section files. The generated edition is labelled source_text, not reviewed.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

VERSION = "source-units-v4"
GENTS = os.environ.get("GENTS", "gents")
SOURCE_MODIFIED = "1970-01-01T00:00:00Z"  # reviewed editions sort after raw source editions
OPEN_ACCESS = {"open", "open_access", "author_copy", "public_domain"}


def digest(value):
    if not isinstance(value, (bytes, bytearray)):
        value = value.encode("utf-8")
    return hashlib.sha256(value).hexdigest()


def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def access_label(value):
    if value in OPEN_ACCESS:
        return "open"
    if value == "restricted":
        return "restricted"
    return "local_only"


def windows(value, maximum=4500):
    """Cover every character once; prefer a paragraph or word boundary."""
    start = 0
    while start < len(value):
        end = min(len(value), start + maximum)
        if end < len(value):
            for boundary in ("\n\n", "\n", " "):
                found = value.rfind(boundary, start + maximum // 2, end)
                if found > start:
                    end = found + len(boundary)
                    break
        yield start, end, value[start:end]
        start = end


def chapter(unit):
    kind, locator = unit["locator_kind"], unit["locator"]
    if kind == "epub_spine":
        key = f"spine:{locator['spine_index']}:{locator['href']}"
        return key, Path(locator["href"]).stem.replace("_", " ")
    if kind in {"chapter_marker", "page_marker"}:
        label = locator["label"]
        return f"marker:{locator.get('file', '')}:{label}", label
    if kind == "ancient_citation":
        citation = locator.get("citation", "")
        book = citation.split(".", 1)[0] if citation else "front"
        return f"ancient:{book}", f"Book {book}" if book != "front" else "Front matter"
    return "body", "Text"


def span(unit, start, end):
    return {"source": unit["work_id"], "unit_id": unit["unit_id"],
            "locator_kind": unit["locator_kind"], "locator": unit["locator"],
            "start_char": unit["char_start"] + start,
            "end_char": unit["char_start"] + end,
            "source_sha256": unit["source_sha256"],
            "anchors": unit.get("anchors", [])}


def locator_summary(spans):
    first, last = spans[0], spans[-1]
    kind = first["locator_kind"]
    if kind == "pdf_page":
        a, b = first["locator"]["page"], last["locator"]["page"]
        prefix = first["locator"].get("file", "")
        label = f"PDF page {a}" if a == b else f"PDF pages {a}–{b}"
        return f"{prefix}, {label}" if prefix else label
    if kind == "form_feed_page":
        prefix = first["locator"].get("file", "")
        label = f"form-feed page {first['locator']['page']}"
        return f"{prefix}, {label}" if prefix else label
    if kind == "ancient_citation":
        a, b = first["locator"].get("citation", ""), last["locator"].get("citation", "")
        return f"⟦{a}⟧" if a == b else f"⟦{a}–{b}⟧"
    if kind == "epub_spine":
        return f"{first['locator']['href']}, chars {first['start_char']}–{last['end_char']}"
    if kind in {"page_marker", "chapter_marker"}:
        return first["locator"]["label"]
    return f"chars {first['start_char']}–{last['end_char']}"


def stage_work(work, units):
    work_id = work["work_id"]
    source_hash = work["source_sha256"]
    edition_id = "source-" + digest(canonical([VERSION, work_id, source_hash]))[:24]
    access = access_label(work.get("access", ""))
    license_name = work.get("license") or "unknown"
    if access == "open" and license_name in {"unknown", "not_recorded"}:
        raise ValueError(f"open work needs explicit license: {work_id}")
    language = work.get("language") or "und"
    passages = []
    blank = 0
    chapter_key = None
    chapter_title = None
    pending = []
    pending_spans = []
    seen_units = set()

    def flush():
        nonlocal pending, pending_spans
        if not pending:
            return
        body = "".join(pending)
        passage_id = "p-" + digest(canonical([work_id, source_hash, pending_spans]))[:24]
        href = ""
        for item in pending_spans:
            if item["locator_kind"] == "epub_spine":
                href = item["locator"]["href"]
                break
        passages.append({
            "record_id": digest(canonical([work_id, edition_id, passage_id])),
            "run_id": edition_id, "book_id": work_id, "edition_id": edition_id,
            "passage_id": passage_id, "chapter_id": "s-" + digest(chapter_key)[:20],
            "chapter_title": chapter_title, "title": work["title"],
            "author": work.get("authors") or "Unknown", "language": language,
            "access": access, "license": license_name,
            "source_type": work["source_format"], "source_hash": source_hash,
            "source_hash_scope": "original_source", "text_hash": digest(body),
            "revision": "", "status": "source_text", "analyzer": "english",
            "text": body, "preview": " ".join(body.split())[:320],
            "locator_summary": locator_summary(pending_spans),
            "source_spans_json": canonical(pending_spans),
            "epub_href": href,
        })
        pending, pending_spans = [], []

    for unit in units:
        if unit["work_id"] != work_id or unit["source_sha256"] != source_hash:
            raise ValueError(f"source unit does not match work/source hash: {work_id}")
        if unit["unit_id"] in seen_units:
            raise ValueError(f"duplicate source unit: {unit['unit_id']}")
        seen_units.add(unit["unit_id"])
        if digest(unit["text"]) != unit["text_sha256"]:
            raise ValueError(f"source unit text hash mismatch: {unit['unit_id']}")
        text = unit["text"]
        if not text.strip():
            blank += 1
            continue
        key, title = chapter(unit)
        if key != chapter_key:
            flush()
            chapter_key, chapter_title = key, title
        for start, end, part in windows(text):
            if not part.strip():
                continue
            rendered = (f"⟦{unit['locator']['citation']}⟧ " + part
                        if unit["locator_kind"] == "ancient_citation" and unit["locator"].get("citation")
                        else part)
            if pending and sum(map(len, pending)) + len(rendered) > 4500:
                flush()
            pending.append(rendered)
            pending_spans.append(span(unit, start, end))
        if unit["locator_kind"] != "ancient_citation":
            flush()  # never merge separate physical pages or EPUB spine spans
    flush()
    if not passages:
        raise ValueError(f"work has no searchable text: {work_id}")
    metadata = {k: work.get(k) for k in ("work_id", "kind", "slug", "year", "topics", "source_path",
                                          "source_format", "access", "license", "source_url")}
    metadata.update(source_sha256=source_hash, source_units=len(units), blank_units=blank,
                    importer=VERSION, extraction_status="source_text_unreviewed")
    content_hashes = [digest(canonical({k: v for k, v in passage.items() if k != "revision"}))
                      for passage in passages]
    revision = digest(canonical([VERSION, source_hash, metadata,
                                 content_hashes]))
    for passage in passages:
        passage["revision"] = revision
        passage["record_hash"] = digest(canonical(passage))
    edition = {
        "run_id": edition_id, "book_id": work_id, "edition_id": edition_id,
        "title": work["title"], "author": work.get("authors") or "Unknown",
        "language": language, "access": access, "license": license_name,
        "source_hash": source_hash, "source_hash_scope": "original_source",
        "revision": revision, "modified": SOURCE_MODIFIED,
        "passage_count": len(passages), "status": "source_text",
        "source_metadata_json": canonical(metadata),
    }
    return {"edition": edition, "passages": passages}


def stage(catalog, units_file, output, selected=None):
    with catalog.open() as stream:
        rows = [json.loads(line) for line in stream]
    works = {row["work_id"]: row for row in rows}
    if len(works) != len(rows):
        raise ValueError("duplicate work_id in catalog")
    if selected and selected not in works:
        raise ValueError(f"unknown work: {selected}")
    output.mkdir(parents=True, exist_ok=True)
    manifest = []
    active, current, seen = None, [], set()

    def save(work_id, rows):
        if not rows or (selected and selected != work_id):
            return
        staged = stage_work(works[work_id], rows)
        name = digest(work_id)[:24] + ".json"
        path = output / name
        value = canonical(staged) + "\n"
        if path.exists() and path.read_text() != value:
            raise ValueError(f"existing staged edition differs: {path}")
        path.write_text(value)
        manifest.append({"work_id": work_id, "path": name,
                         "edition_id": staged["edition"]["edition_id"],
                         "passages": len(staged["passages"]), "sha256": digest(value)})

    with units_file.open() as stream:
        for line in stream:
            row = json.loads(line)
            work_id = row["work_id"]
            if work_id not in works:
                raise ValueError(f"source units include unknown work: {work_id}")
            if active is not None and work_id != active:
                save(active, current)
                seen.add(active)
                current = []
                if work_id in seen:
                    raise ValueError(f"units for {work_id} are not contiguous")
            active = work_id
            if not selected or selected == work_id:
                current.append(row)
    if active is not None:
        save(active, current)
    if selected and not manifest:
        raise ValueError(f"no units for {selected}")
    path = output / "manifest.jsonl"
    value = "".join(canonical(row) + "\n" for row in manifest)
    if path.exists() and path.read_text() != value:
        raise ValueError(f"stage manifest differs from existing output: {path}")
    path.write_text(value)
    return {"works": len(manifest), "passages": sum(row["passages"] for row in manifest),
            "manifest": str(path)}


def gents(*args):
    proc = subprocess.run([GENTS, *map(str, args)], check=True, capture_output=True, text=True)
    return json.loads(proc.stdout)


def find(home, collection, edition_id, fields):
    out, offset = [], 0
    while True:
        args = ["query", "find", "--home", home, "--collection", collection,
                "--filter", canonical({"edition_id": {"_eq": edition_id}}),
                "--limit", "1000", "--offset", str(offset)]
        for field in fields:
            args += ["--field", field]
        result = gents(*args)
        if result.get("truncated"):
            raise RuntimeError(f"{collection} identity query was truncated")
        out.extend(result["results"])
        if len(result["results"]) < 1000:
            return out
        offset += 1000


def load(home, staged_dir, max_works=None, dry_run=False):
    with (staged_dir / "manifest.jsonl").open() as stream:
        manifest = [json.loads(line) for line in stream]
    done = 0
    for item in manifest[:max_works]:
        path = staged_dir / item["path"]
        raw = path.read_bytes()
        if digest(raw) != item["sha256"]:
            raise ValueError(f"staged file hash mismatch: {path}")
        work = json.loads(raw)
        edition = work["edition"]
        edition_id = edition["edition_id"]
        passages = work["passages"]
        if len(passages) != edition["passage_count"]:
            raise ValueError(f"staged passage count mismatch: {path}")
        existing = find(home, "ShelfLibraryPassage", edition_id, ["record_id", "record_hash"])
        expected = {row["record_id"]: row["record_hash"] for row in passages}
        if len(existing) != len({row["record_id"] for row in existing}):
            raise RuntimeError(f"duplicate stored passage identity: {edition_id}")
        for row in existing:
            if expected.get(row["record_id"]) != row["record_hash"]:
                raise RuntimeError(f"stored passage differs from staged source: {edition_id}")
        present = {row["record_id"] for row in existing}
        receipt = find(home, "ShelfLibraryEdition", edition_id, ["revision", "passage_count"])
        if receipt:
            if len(receipt) != 1 or len(present) != len(passages) or receipt[0] != {
                "revision": edition["revision"], "passage_count": len(passages)
            }:
                raise RuntimeError(f"stored edition receipt is inconsistent: {edition_id}")
            print(canonical({"work_id": item["work_id"], "status": "already_loaded"}), flush=True)
            done += 1
            continue
        missing = [row for row in passages if row["record_id"] not in present]
        if dry_run:
            print(canonical({"work_id": item["work_id"], "status": "dry_run",
                             "missing_passages": len(missing)}), flush=True)
            continue
        for index, row in enumerate(missing, 1):
            gents("document", "create", "ShelfLibraryPassage", "--home", home,
                  "--json", canonical(row))
            if index % 100 == 0:
                print(canonical({"work_id": item["work_id"], "written_passages": index}), flush=True)
        gents("document", "create", "ShelfLibraryEdition", "--home", home,
              "--json", canonical(edition))
        print(canonical({"work_id": item["work_id"], "status": "loaded",
                         "passages": len(passages)}), flush=True)
        done += 1
    return {"completed_works": done}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    staged = sub.add_parser("stage", help="Prepare immutable source_text editions from normalized units")
    staged.add_argument("--catalog", type=Path, required=True)
    staged.add_argument("--units", type=Path, required=True)
    staged.add_argument("--output", type=Path, required=True)
    staged.add_argument("--work-id")
    loader = sub.add_parser("load", help="Resume publishing staged editions to one Gents home")
    loader.add_argument("--home", type=Path, required=True)
    loader.add_argument("--staged", type=Path, required=True)
    loader.add_argument("--max-works", type=int)
    loader.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    try:
        if args.command == "stage":
            result = stage(args.catalog, args.units, args.output, args.work_id)
        else:
            result = load(args.home, args.staged, args.max_works, args.dry_run)
        print(canonical(result))
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as exc:
        parser.exit(1, f"shelf-intake: {exc}\n")


if __name__ == "__main__":
    main()
