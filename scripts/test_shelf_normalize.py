"""Exercise raw-format normalization before Shelf's direct-text staging."""

import importlib.util
import json
from pathlib import Path
import shutil
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location("shelf_normalize", Path(__file__).with_name("shelf-normalize.py"))
normal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(normal)


class RawFormats(unittest.TestCase):
    def test_document_metadata_is_excluded_and_nested_tei_text_is_not_duplicated(self):
        html = '<html><head><title>Metadata title</title></head><body><nav>Menu</nav><h1>Real <em>Chapter</em></h1><p>Body text.</p></body></html>'
        rows = list(normal.html_units(html, "c.xhtml", 1))
        text = "".join(r[2] for r in rows)
        self.assertNotIn("Metadata title", text)
        self.assertNotIn("Menu", text)
        self.assertEqual(rows[0][1]["title"], "Real Chapter")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/"source.xml"
            path.write_text('<TEI><teiHeader><p>License metadata</p></teiHeader><text><div n="2"><p n="3">Body <note><p>Inline note</p></note> end.</p></div></text></TEI>')
            rows = list(normal.xml_units(path))
            self.assertEqual(len(rows), 1)
            self.assertNotIn("License metadata", rows[0][2])
            self.assertEqual(rows[0][2].count("Inline note"), 1)
            self.assertEqual(rows[0][1]["division_path"], ["2"])

    def test_epub_decodes_hrefs_and_rejects_missing_spine_items(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/"book.epub"
            with zipfile.ZipFile(path,"w") as archive:
                archive.writestr("META-INF/container.xml", '<container><rootfile full-path="book.opf"/></container>')
                archive.writestr("book.opf", '<package><manifest><item id="a" href="chapter%20one#start" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/></spine></package>')
                archive.writestr("chapter one", '<h1 id="start">Trade</h1><p>Ships.</p>')
            self.assertEqual(list(normal.epub_units(path))[0][1]["href"], "chapter one")
            missing=Path(directory)/"missing.epub"
            with zipfile.ZipFile(missing,"w") as archive:
                archive.writestr("META-INF/container.xml", '<container><rootfile full-path="book.opf"/></container>')
                archive.writestr("book.opf", '<package><spine><itemref idref="absent"/></spine></package>')
            with self.assertRaisesRegex(ValueError,"missing item"):
                list(normal.epub_units(missing))

    def test_tei_p4_divisions_keep_verse_speech_tables_and_loose_text(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/"verse.xml"
            path.write_text('<TEI><text><div1 n="3">Opening <hi>aside</hi> tail<lg><l>First verse</l><l>Second verse</l></lg><sp><speaker>Captain</speaker><p>We sail.</p></sp><list><item>Grain</item></list><table><row><cell>Ships</cell><cell>12</cell></row></table><trailer>End</trailer></div1></text></TEI>')
            rows=list(normal.xml_units(path))
            text="\n".join(r[2] for r in rows)
            for word in ['Opening','aside','tail','First verse','Second verse','Captain','We sail.','Grain','Ships','12','End']:
                self.assertEqual(text.count(word),1,word)
            verses=[r for r in rows if r[1]['element']=='lg']
            self.assertEqual(len(verses),1)
            self.assertEqual(verses[0][1]['division_path'],['3'])

    def test_epub_spine_order_href_and_anchor(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "book.epub"
            with zipfile.ZipFile(path, "w") as archive:
                archive.writestr("META-INF/container.xml", '<container><rootfiles><rootfile full-path="OPS/book.opf"/></rootfiles></container>')
                archive.writestr("OPS/book.opf", '''<package><manifest>
                    <item id="a" href="first.xhtml"/><item id="b" href="second.xhtml"/>
                    </manifest><spine><itemref idref="b"/><itemref idref="a"/></spine></package>''')
                archive.writestr("OPS/first.xhtml", '<html><body><p id="first">First chapter.</p></body></html>')
                archive.writestr("OPS/second.xhtml", '<html><body><p id="second">Second chapter.</p></body></html>')
            self.assertEqual(normal.source_format(path), "epub")
            rows = list(normal.epub_units(path))
            self.assertEqual([row[1]["href"] for row in rows],
                             ["OPS/second.xhtml", "OPS/first.xhtml"])
            self.assertEqual(rows[0][5][0]["id"], "second")
            self.assertNotIn("page", rows[0][1])

    def test_html_and_tei_keep_real_element_locators(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            html = root / "article.html"
            html.write_text('<html><body><p id="intro">Harbour <b>trade</b>.</p><script>bad</script></body></html>')
            rows = list(normal.html_units(html))
            self.assertEqual(rows[0][0], "html_span")
            self.assertIn("Harbour trade.", rows[0][2])
            self.assertNotIn("bad", rows[0][2])
            self.assertEqual(rows[0][5][0]["id"], "intro")
            tei = root / "source.xml"
            tei.write_text('<TEI><text><p n="2.1">A ship sailed.</p></text></TEI>')
            self.assertEqual(list(normal.xml_units(tei))[0][1]["n"], "2.1")

    def test_text_page_markers_and_pdf_physical_pages(self):
        rows = list(normal.marked_text("Preface\n=====PAGE 7\nShips.\n=====PAGE 8\nCargo."))
        self.assertEqual([row[0] for row in rows], ["text_span", "page_marker", "page_marker"])
        self.assertEqual([row[1]["label"] for row in rows[1:]], ["7", "8"])
        self.assertEqual([row[0] for row in normal.marked_text("First\fSecond\f")],
                         ["form_feed_page", "form_feed_page"])
        if not shutil.which("pdfinfo") or not shutil.which("pdftotext"):
            self.skipTest("Poppler command line tools unavailable")
        pdf = Path(__file__).resolve().parents[1] / "packs/gents/ocr/plugins/ocr/tests/fixtures/text.pdf"
        pages = list(normal.pdf_units(pdf))
        self.assertEqual([row[1]["page"] for row in pages], [1, 2])
        self.assertIn("Annual Report", pages[0][2])

    def test_catalog_hash_and_normalized_units_round_trip(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "source.txt").write_text("=====PAGE 3\nA Roman ship.")
            catalog = root / "catalog.jsonl"
            work = dict(work_id="sample", title="Sample", authors="Historian", access="open",
                        license="CC0", source_path="source.txt")
            catalog.write_text(json.dumps(work) + "\n")
            out_catalog, out_units = root / "out-catalog.jsonl", root / "out-units.jsonl"
            summary = normal.normalize(catalog, root, out_catalog, out_units)
            self.assertEqual(summary["works"], 1)
            row = json.loads(out_units.read_text())
            self.assertEqual(row["locator"]["label"], "3")
            self.assertEqual(row["text_sha256"], normal.digest(row["text"]))
            self.assertEqual(json.loads(out_catalog.read_text())["source_sha256"],
                             normal.file_hash(root / "source.txt"))
            work["source_sha256"] = "bad"
            catalog.write_text(json.dumps(work) + "\n")
            with self.assertRaisesRegex(ValueError, "source hash mismatch"):
                normal.normalize(catalog, root, root / "other-cat", root / "other-units")

    def test_supplied_html_page_markers_take_precedence_over_unmarked_dom_text(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "article.html").write_text('<html><body><p id="a">Evidence.</p></body></html>')
            (root / "article.txt").write_text("=====PAGE 12\nEvidence.")
            catalog = root / "catalog.jsonl"
            catalog.write_text(json.dumps(dict(work_id="article", title="Article", authors="Scholar",
                                               access="open", license="CC0", source_path="article.html",
                                               text_path="article.txt")) + "\n")
            units = root / "units.jsonl"
            normal.normalize(catalog, root, root / "enriched.jsonl", units)
            row = json.loads(units.read_text())
            self.assertEqual(row["locator_kind"], "page_marker")
            self.assertEqual(row["locator"]["label"], "12")

    def test_directory_pages_keep_their_source_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "part-a.txt").write_text("First page.\f")
            (root / "part-b.txt").write_text("Second page.\f")
            rows = list(normal.directory_units(root))
            self.assertEqual([r[0] for r in rows], ["form_feed_page", "form_feed_page"])
            self.assertEqual([(r[1]["file"], r[1]["page"]) for r in rows],
                             [("part-a.txt", 1), ("part-b.txt", 1)])


if __name__ == "__main__":
    unittest.main()
