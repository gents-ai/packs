#!/usr/bin/env python3
"""Turn local PDF, EPUB, HTML, XML/TEI, TXT and section dirs into Shelf units.

Catalog input is JSONL with work_id, title, authors, access, license and
source_path. Paths are relative to --root unless absolute. Optional text_path
or sections_path supplies existing extraction and avoids new OCR. Output is the
catalog/units pair accepted by shelf-intake.py. This command makes no model call.
"""

import argparse
import datetime
from collections import Counter
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path
import posixpath
import re
import subprocess
import zipfile
from urllib.parse import unquote, urlsplit
from xml.etree import ElementTree as ET

PAGE = re.compile(r"(?m)^=====PAGE\s+([^\n]+)\n?")
CHAPTER = re.compile(r"(?m)^=====CHAPTER\s+([^\n]+)\n?")
BLOCKS = {"p", "div", "section", "article", "br", "hr", "h1", "h2", "h3", "h4", "h5",
          "h6", "li", "ul", "ol", "table", "tr", "td", "th", "blockquote", "aside"}


def digest(value):
    return hashlib.sha256(value if isinstance(value, bytes) else value.encode()).hexdigest()


def file_hash(path):
    state = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            state.update(block)
    return state.hexdigest()


def source_hash(path):
    if path.is_file():
        return file_hash(path)
    state = hashlib.sha256()
    for item in sorted(p for p in path.rglob("*") if p.is_file()):
        state.update(str(item.relative_to(path)).encode())
        state.update(b"\0")
        state.update(bytes.fromhex(file_hash(item)))
    return state.hexdigest()


def path_from(root, value):
    path = Path(value).expanduser()
    return (path if path.is_absolute() else root / path).resolve(strict=True)


def source_format(path):
    if path.is_dir():
        return "directory"
    with path.open("rb") as stream:
        header = stream.read(512).lstrip()
    if header.startswith(b"%PDF"):
        return "pdf"
    if zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as archive:
            if "META-INF/container.xml" in archive.namelist():
                return "epub"
    if path.suffix.lower() in {".html", ".htm", ".xhtml"} or b"<html" in header.lower():
        return "html"
    if header.startswith(b"<?xml") or header.startswith((b"<TEI", b"<tei")):
        return "xml"
    return "txt"


def spans(text, target=4500):
    start = 0
    while start < len(text):
        end = min(len(text), start + target)
        if end < len(text):
            cut = text.rfind("\n\n", start + target // 2, end)
            if cut > start:
                end = cut + 2
        yield start, end, text[start:end]
        start = end


def marked_text(text, form_feed_kind="form_feed_page"):
    if "\f" in text:
        pages = text.split("\f")
        if pages[-1] == "":
            pages.pop()
        offset = 0
        for number, page in enumerate(pages, 1):
            yield form_feed_kind, {"page": number}, page, offset, offset + len(page), []
            offset += len(page) + 1
        return
    for marker, kind in ((PAGE, "page_marker"), (CHAPTER, "chapter_marker")):
        matches = list(marker.finditer(text))
        if matches:
            if text[:matches[0].start()].strip():
                for start, end, part in spans(text[:matches[0].start()]):
                    yield "text_span", {"start": start, "end": end}, part, start, end, []
            for index, match in enumerate(matches):
                start = match.end()
                end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
                for local_start, local_end, part in spans(text[start:end]):
                    yield kind, {"label": match.group(1).strip(), "start": local_start,
                                 "end": local_end}, part, start + local_start, start + local_end, []
            return
    for start, end, part in spans(text):
        yield "text_span", {"start": start, "end": end}, part, start, end, []


class TextHTML(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.parts, self.anchors, self.length, self.skip = [], [], 0, 0
        self.headings = []
        self.heading = None

    def add(self, value):
        self.parts.append(value)
        self.length += len(value)

    def newline(self):
        if not self.parts or not self.parts[-1].endswith("\n"):
            self.add("\n")

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag in {"script", "style", "head", "nav"}:
            self.skip += 1
            return
        if self.skip:
            return
        if tag in BLOCKS:
            self.newline()
        if tag in {"h1", "h2", "h3", "h4", "h5", "h6"}:
            self.heading = [self.length, int(tag[1]), []]
        if attrs.get("id"):
            self.anchors.append({"id": attrs["id"], "char_offset": self.length})
        if tag == "img" and attrs.get("alt"):
            self.add(f"[image: {attrs['alt']}]")

    def handle_endtag(self, tag):
        if self.heading and tag == f"h{self.heading[1]}":
            self.headings.append({"char_offset":self.heading[0], "level":self.heading[1], "title":"".join(self.heading[2]).strip()})
            self.heading = None
        if tag in {"script", "style", "head", "nav"}:
            self.skip = max(0, self.skip - 1)
        elif not self.skip and tag in BLOCKS:
            self.newline()

    def handle_data(self, value):
        if not self.skip:
            self.add(value)
            if self.heading is not None:
                self.heading[2].append(value)


def html_units(path, href=None, spine_index=None):
    parser = TextHTML()
    parser.feed(path.read_text(errors="replace") if isinstance(path, Path) else path)
    text = "".join(parser.parts)
    for start, end, part in spans(text):
        anchors = [{"id": a["id"], "char_offset": a["char_offset"] - start}
                   for a in parser.anchors if start <= a["char_offset"] < end]
        if spine_index is None:
            kind, locator = "html_span", {"file": path.name if isinstance(path, Path) else "",
                                          "start": start, "end": end}
        else:
            kind, locator = "epub_spine", {"spine_index": spine_index, "href": href,
                                            "char_start": start, "char_end": end}
        headings = [dict(h, char_offset=h["char_offset"]-start) for h in parser.headings if start <= h["char_offset"] < end]
        if headings:
            locator["headings"] = headings
        if parser.headings:
            locator["title"] = parser.headings[0]["title"]
        yield kind, locator, part, start, end, anchors


def epub_units(path):
    with zipfile.ZipFile(path) as archive:
        if archive.testzip() is not None:
            raise ValueError(f"corrupt EPUB: {path}")
        container = ET.fromstring(archive.read("META-INF/container.xml"))
        opf_path = next(node.attrib["full-path"] for node in container.iter()
                        if node.tag.endswith("rootfile"))
        opf = ET.fromstring(archive.read(opf_path))
        items = {node.attrib["id"]: (node.attrib["href"], node.attrib.get("media-type", "")) for node in opf.iter()
                 if node.tag.endswith("item") and "id" in node.attrib}
        spine = [node.attrib["idref"] for node in opf.iter() if node.tag.endswith("itemref")]
        for number, item_id in enumerate(spine, 1):
            if item_id not in items:
                raise ValueError(f"EPUB spine references missing item: {item_id}")
            href, media = items[item_id]
            parsed = urlsplit(href)
            if parsed.scheme or parsed.netloc:
                raise ValueError(f"EPUB spine is not a local resource: {href}")
            entry = posixpath.normpath(posixpath.join(posixpath.dirname(opf_path), unquote(parsed.path)))
            if media and media not in {"application/xhtml+xml", "text/html"}:
                raise ValueError(f"unsupported EPUB spine media type: {media}")
            if not media and not entry.lower().endswith((".html", ".htm", ".xhtml")):
                raise ValueError(f"EPUB spine has no supported content type: {entry}")
            if entry.startswith("../") or entry.startswith("/"):
                raise ValueError(f"EPUB spine escapes archive: {entry}")
            yield from html_units(archive.read(entry).decode("utf-8", errors="replace"),
                                  entry, number)


def xml_units(path):
    root = ET.parse(path).getroot()
    number = 0
    blocks = {"p", "l", "ab", "head", "item", "speaker", "stage", "note", "quote", "cit", "cell", "row", "trailer", "byline", "lg", "sp"}

    def emit(element, text, context, segment="element"):
        nonlocal number
        if not text.strip():
            return
        number += 1
        tag = element.tag.rsplit("}", 1)[-1].lower()
        for start, end, part in spans(text):
            yield "xml_element", {"element": tag, "n": element.attrib.get("n", ""),
                "division_path": context, "segment": segment,
                "xml_id": element.attrib.get("{http://www.w3.org/XML/1998/namespace}id", ""),
                "ordinal": number, "start": start, "end": end}, part, start, end, []

    def walk(element, parents):
        tag = element.tag.rsplit("}", 1)[-1].lower()
        if tag == "teiheader":
            return
        context = parents + ([element.attrib["n"]] if re.fullmatch(r"div[0-9]*", tag) and element.attrib.get("n") else [])
        if tag in blocks:
            if tag in {"lg", "sp", "row"}:
                text = (element.text or "") + "\n".join("".join(child.itertext()) + (child.tail or "") for child in element)
            else:
                text = "".join(element.itertext())
            yield from emit(element, text, context)
            return
        yield from emit(element, element.text or "", context, "text")
        for index, child in enumerate(element):
            yield from walk(child, context)
            yield from emit(element, child.tail or "", context, f"tail:{index}")

    yield from walk(root, [])
    if number == 0:
        raise ValueError("XML has no text outside its metadata header")


def directory_units(path, sections_path=None):
    if sections_path:
        with sections_path.open() as stream:
            for line in stream:
                row = json.loads(line)
                text = row["text"]
                yield "ancient_citation", {"citation": row.get("cit", row.get("citation", ""))}, \
                    text, 0, len(text), []
        return
    for file in sorted(p for p in path.rglob("*") if p.is_file()
                       and p.suffix.lower() in {".txt", ".html", ".htm", ".xml", ".jsonl"}):
        relative = str(file.relative_to(path))
        if file.suffix.lower() in {".html", ".htm"}:
            rows = html_units(file)
        elif file.suffix.lower() == ".xml":
            rows = xml_units(file)
        elif file.suffix.lower() == ".jsonl":
            def jsonl_rows():
                with file.open() as stream:
                    for number, line in enumerate(stream, 1):
                        row = json.loads(line)
                        text = row["text"]
                        yield "ancient_citation", {"citation": row.get("cit", row.get("citation", "")),
                                                   "line": number}, text, 0, len(text), []
            rows = jsonl_rows()
        else:
            rows = marked_text(file.read_text(errors="replace"))
        for kind, locator, text, start, end, anchors in rows:
            yield kind, dict(locator, file=relative), text, start, end, anchors


def pdf_units(path, text_path=None):
    info = subprocess.run(["pdfinfo", str(path)], capture_output=True, text=True, check=True)
    match = re.search(r"^Pages:\s+(\d+)", info.stdout, flags=re.M)
    if not match:
        raise ValueError(f"pdfinfo returned no page count: {path}")
    count = int(match.group(1))
    if text_path:
        text = text_path.read_text(errors="replace")
    else:
        result = subprocess.run(["pdftotext", str(path), "-"], capture_output=True,
                                text=True, errors="replace", check=True)
        text = result.stdout
    if "\f" not in text:
        # A separate unpaginated OCR dump is useful for search but cannot cite pages.
        yield from marked_text(text)
        return
    units = list(marked_text(text, form_feed_kind="pdf_page"))
    pages = [u for u in units if u[0] == "pdf_page"]
    if len(pages) != count:
        raise ValueError(f"PDF/text page mismatch: {path}: {len(pages)} vs {count}")
    yield from units


def normalize(catalog, root, output_catalog, output_units):
    root = root.resolve(strict=True)
    final_catalog, final_units = output_catalog, output_units
    output_catalog = output_catalog.with_name(output_catalog.name + ".partial")
    output_units = output_units.with_name(output_units.name + ".partial")
    with catalog.open() as stream:
        works = [json.loads(line) for line in stream]
    if len({w["work_id"] for w in works}) != len(works):
        raise ValueError("duplicate work_id")
    works.sort(key=lambda w: w["work_id"])
    counts = Counter()
    with output_catalog.open("w") as catalog_stream, output_units.open("w") as unit_stream:
        for work in works:
            path = path_from(root, work["source_path"])
            fmt = source_format(path)
            if work.get("source_format") and work["source_format"] not in {fmt, "htm" if fmt == "html" else fmt}:
                raise ValueError(f"declared/source format mismatch: {path}: {work['source_format']} vs {fmt}")
            actual = source_hash(path)
            if work.get("source_sha256") and work["source_sha256"] != actual:
                raise ValueError(f"source hash mismatch: {path}")
            work["source_sha256"], work["source_format"] = actual, fmt
            text_path = path_from(root, work["text_path"]) if work.get("text_path") else None
            sections_path = path_from(root, work["sections_path"]) if work.get("sections_path") else None
            if sections_path:
                units = directory_units(path, sections_path)
            elif fmt == "directory":
                units = directory_units(path)
            elif fmt == "epub":
                units = epub_units(path)
            elif fmt == "pdf":
                units = pdf_units(path, text_path)
            elif fmt == "html":
                supplied = text_path.read_text(errors="replace") if text_path else ""
                units = marked_text(supplied) if PAGE.search(supplied) or CHAPTER.search(supplied) else html_units(path)
            elif fmt == "xml":
                units = xml_units(path)
            else:
                units = marked_text((text_path or path).read_text(errors="replace"))
            if "modified" not in work:
                candidates = [path] + ([text_path] if text_path else []) + ([sections_path] if sections_path else [])
                files = [f for candidate in candidates for f in (candidate.rglob("*") if candidate.is_dir() else [candidate]) if f.is_file()]
                modified = max(f.stat().st_mtime for f in files)
                work["modified"] = datetime.datetime.fromtimestamp(modified, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
            catalog_stream.write(json.dumps(work, ensure_ascii=False) + "\n")
            ordinal = 0
            for ordinal, (kind, locator, text, start, end, anchors) in enumerate(units, 1):
                identity = json.dumps([work["work_id"], actual, kind, ordinal, locator],
                                      ensure_ascii=False, sort_keys=True)
                row = {"unit_id": "u-" + digest(identity)[:24], "work_id": work["work_id"],
                       "ordinal": ordinal, "locator_kind": kind, "locator": locator,
                       "char_start": start, "char_end": end, "anchors": anchors,
                       "text": text, "text_sha256": digest(text), "source_sha256": actual}
                unit_stream.write(json.dumps(row, ensure_ascii=False) + "\n")
                counts[kind] += 1
            if ordinal == 0:
                raise ValueError(f"no text units for {work['work_id']}")
            counts["works"] += 1
    output_catalog.replace(final_catalog)
    output_units.replace(final_units)
    return dict(counts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catalog", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--output-catalog", type=Path, required=True)
    parser.add_argument("--output-units", type=Path, required=True)
    args = parser.parse_args()
    if args.output_catalog.exists() or args.output_units.exists():
        parser.error("output files already exist")
    args.output_catalog.parent.mkdir(parents=True, exist_ok=True)
    args.output_units.parent.mkdir(parents=True, exist_ok=True)
    try:
        print(json.dumps(normalize(args.catalog, args.root, args.output_catalog, args.output_units)))
    except (OSError, ValueError, KeyError, StopIteration, zipfile.BadZipFile, subprocess.CalledProcessError, ET.ParseError) as error:
        parser.exit(1, f"shelf-normalize: {error}\n")


if __name__ == "__main__":
    main()
