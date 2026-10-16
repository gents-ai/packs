#!/usr/bin/env python3
"""Check a reviewed Shelf artifact against its EPUB and original page spans."""
import argparse
import hashlib
import json
from pathlib import Path
import posixpath
import subprocess
import xml.etree.ElementTree as ET
import zipfile


def chapter_label(chapter):
    number = chapter.get("entry_number")
    if not number:
        return chapter["title"]
    kind = chapter.get("level_name") or ""
    label = kind[:1].upper() + kind[1:]
    marker = f"{label} {number}".strip()
    return marker if chapter["title"].strip().lower() == marker.lower() else f"{marker}: {chapter['title']}"


def validate_human_book(book):
    if "book" not in book:
        return
    human = []
    def walk(sections, parent=None):
        for section in sections:
            if not isinstance(section.get("audio_include"), bool) or not section.get("audio_include_reasoning"):
                raise ValueError("human chapter needs a narration decision and reason")
            human.append((section, parent))
            walk(section["sections"], section["id"])
    walk(book["book"]["sections"])
    edition = book["edition"]["chapters"]
    if [s["id"] for s, _ in human] != [c["id"] for c in edition]:
        raise ValueError("human hierarchy disagrees with EPUB chapter order")
    for (section, parent), chapter in zip(human, edition):
        if parent != chapter.get("parent_id"):
            raise ValueError("human chapter parent disagrees with EPUB hierarchy")
        for key in ["title", "entry_number", "level", "level_name", "matter_type", "content_type", "audio_include", "audio_include_reasoning"]:
            if section.get(key) != chapter.get(key):
                raise ValueError(f"human chapter metadata differs: {key}")
        if [(p["id"], p["text"], p["source_spans"]) for p in section["paragraphs"]] != [(b["id"], b["markdown"], b["sources"]) for b in chapter["blocks"]]:
            raise ValueError("human paragraphs differ from the accepted EPUB text")
        if [p["ordinal"] for p in section["paragraphs"]] != list(range(1, len(section["paragraphs"]) + 1)):
            raise ValueError("human paragraph ordering is inconsistent")


def validate(structured, epub):
    book = json.loads(structured.read_text())
    validate_human_book(book)
    edition, original = book["edition"], book["original"]
    pages = {}
    for chapter in original["chapters"]:
        for page in chapter["pages"]:
            key = (page["source"], page["page"])
            if key in pages:
                raise ValueError(f"duplicate original page: {key}")
            pages[key] = page["markdown"].encode()
    sources = original.get("sources")
    if sources is None:
        sources = original["source_manifest"]
        if isinstance(sources, str):
            sources = json.loads(sources)
    expected_pages = {(source["source"], page)
                      for source in sources
                      for page in range(1, source["page_count"] + 1)}
    if set(pages) != expected_pages:
        raise ValueError("original chapter ranges do not cover source pages exactly")
    x = {"h": "http://www.w3.org/1999/xhtml", "o": "http://www.idpf.org/2007/opf"}
    count, seen = 0, set()
    with zipfile.ZipFile(epub) as archive:
        container = ET.fromstring(archive.read("META-INF/container.xml"))
        opf_path = next(e.attrib["full-path"] for e in container.iter() if e.tag.endswith("}rootfile"))
        opf = ET.fromstring(archive.read(opf_path))
        folder = posixpath.dirname(opf_path)
        items = {e.attrib["id"]: e.attrib for e in opf.findall("o:manifest/o:item", x)}
        spine = [items[e.attrib["idref"]]["href"] for e in opf.findall("o:spine/o:itemref", x)
                 if "nav" not in items[e.attrib["idref"]].get("properties", "").split()]
        expected_spine = [c["id"] + ".xhtml" for c in edition["chapters"]]
        if spine != expected_spine:
            raise ValueError("EPUB spine disagrees with manuscript chapter order")
        nav_item = next(i for i in items.values() if "nav" in i.get("properties", "").split())
        nav = ET.fromstring(archive.read(posixpath.join(folder, nav_item["href"])))
        toc = next(e for e in nav.findall(".//h:nav", x)
                   if e.attrib.get("{http://www.idpf.org/2007/ops}type") == "toc")
        links = toc.findall(".//h:a", x)
        if [(a.attrib["href"], "".join(a.itertext())) for a in links] != [
                (name, chapter_label(c)) for name, c in zip(expected_spine, edition["chapters"])]:
            raise ValueError("EPUB table of contents disagrees with manuscript")
        documents = {}
        for chapter, name in zip(edition["chapters"], spine):
            doc = ET.fromstring(archive.read(posixpath.join(folder, name)))
            documents[posixpath.join(folder, name)] = doc
            ids = {}
            for element in doc.iter():
                if "id" in element.attrib:
                    identifier = element.attrib["id"]
                    if identifier in ids:
                        raise ValueError(f"duplicate XHTML anchor: {identifier}")
                    ids[identifier] = element
            for block in chapter["blocks"]:
                identifier = block["id"]
                if identifier in seen or identifier not in ids:
                    raise ValueError(f"duplicate or missing passage anchor: {identifier}")
                seen.add(identifier)
                if block["markdown"].strip() and not "".join(ids[identifier].itertext()).strip():
                    raise ValueError(f"empty rendered passage: {identifier}")
                if not block["sources"]:
                    raise ValueError(f"passage has no source: {identifier}")
                for span in block["sources"]:
                    raw = pages[(span["source"], span["page"])]
                    start, end = span["start_byte"], span["end_byte"]
                    if not 0 <= start < end <= len(raw):
                        raise ValueError(f"invalid source span: {identifier}")
                    raw[start:end].decode("utf-8")
                count += 1
        passages = book["passages"]
        if len(passages) != count or {p["passage_id"] for p in passages} != seen:
            raise ValueError("machine passage inventory differs from EPUB")
        for passage in passages:
            path, anchor = passage["epub_href"].split("#", 1)
            doc = documents[path]
            if not any(e.attrib.get("id") == anchor for e in doc.iter()):
                raise ValueError(f"citation does not resolve: {passage['epub_href']}")
    subprocess.run(["epubcheck", str(epub)], check=True)
    return {"epub": str(epub.resolve()), "sha256": hashlib.sha256(epub.read_bytes()).hexdigest(),
            "chapters": len(edition["chapters"]), "source_pages": len(pages), "passages": count,
            "automated_checks": "passed", "reading_quality": "requires source comparison and inspection"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--structured", required=True, type=Path)
    parser.add_argument("--epub", required=True, type=Path)
    args = parser.parse_args()
    print(json.dumps(validate(args.structured, args.epub), indent=2))
