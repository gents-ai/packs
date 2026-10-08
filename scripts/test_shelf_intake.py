"""Check that direct-text editions retain source identity and citable locators."""

import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("shelf_intake", Path(__file__).with_name("shelf-intake.py"))
intake = importlib.util.module_from_spec(spec)
spec.loader.exec_module(intake)


def work(**changes):
    row = dict(work_id="work:one", source_sha256="a" * 64, title="Roman Trade",
               authors="Scholar", language="en", access="open_access",
               license="CC BY 4.0", source_format="pdf", source_path="book.pdf")
    row.update(changes)
    return row


def unit(ordinal, text, kind="pdf_page", locator=None):
    locator = locator or {"page": ordinal}
    return dict(unit_id=f"unit-{ordinal}", work_id="work:one", source_sha256="a" * 64,
                locator_kind=kind, locator=locator, char_start=0, char_end=len(text),
                text=text, text_sha256=intake.digest(text))


class SourceIntake(unittest.TestCase):
    def test_migrated_chapter_keeps_coarse_range_and_export_hash_scope(self):
        source = unit(1, "word " * 2000, "chapter_range", {
            "chapter_id": "old-chapter", "title": "Trade", "start_page": 12, "end_page": 28})
        result = intake.stage_work(work(source_hash_scope="legacy_export",
            provenance={"legacy_status": "complete"}), [source])
        self.assertGreater(len(result["passages"]), 1)
        for passage in result["passages"]:
            self.assertEqual(passage["locator_summary"], "Trade, scan pages 12–28 (chapter range)")
            self.assertEqual(passage["source_hash_scope"], "legacy_export")
            self.assertEqual(passage["status"], "source_text")
            self.assertNotIn("page", json.loads(passage["source_spans_json"])[0]["locator"])
        self.assertEqual(json.loads(result["edition"]["source_metadata_json"])["provenance"],
                         {"legacy_status": "complete"})

    def test_pdf_keeps_physical_page_numbers_and_blank_page_coverage(self):
        staged = intake.stage_work(work(), [unit(1, "Ships arrived."), unit(2, "\n"),
                                            unit(3, "The grain fleet sailed.")])
        self.assertEqual(staged["edition"]["access"], "open")
        self.assertEqual(staged["edition"]["status"], "source_text")
        self.assertEqual(len(staged["passages"]), 2)
        self.assertEqual([json.loads(p["source_spans_json"])[0]["locator"]["page"]
                          for p in staged["passages"]], [1, 3])
        self.assertEqual([p["locator_summary"] for p in staged["passages"]],
                         ["PDF page 1", "PDF page 3"])
        metadata = json.loads(staged["edition"]["source_metadata_json"])
        self.assertEqual((metadata["source_units"], metadata["blank_units"]), (3, 1))

    def test_epub_uses_spine_href_and_never_invents_a_page(self):
        source = unit(1, "Muziris cargo", "epub_spine",
                      {"spine_index": 7, "href": "OEBPS/chapter.xhtml",
                       "char_start": 51, "char_end": 64})
        staged = intake.stage_work(work(source_format="epub", access="local_only",
                                        license="not_recorded"), [source])
        passage = staged["passages"][0]
        self.assertEqual(passage["epub_href"], "OEBPS/chapter.xhtml")
        self.assertEqual(passage["chapter_title"], "chapter")
        self.assertNotIn("page", json.loads(passage["source_spans_json"])[0]["locator"])
        self.assertEqual(staged, intake.stage_work(work(source_format="epub", access="local_only",
                                                       license="not_recorded"), [source]))

    def test_ancient_sections_join_without_losing_citations(self):
        rows = [unit(1, "A fleet sailed.", "ancient_citation", {"citation": "1.2"}),
                unit(2, "Another arrived.", "ancient_citation", {"citation": "1.3"})]
        staged = intake.stage_work(work(source_format="directory"), rows)
        self.assertEqual(len(staged["passages"]), 1)
        passage = staged["passages"][0]
        self.assertIn("⟦1.2⟧", passage["text"])
        self.assertIn("⟦1.3⟧", passage["text"])
        self.assertEqual(passage["locator_summary"], "⟦1.2–1.3⟧")
        self.assertEqual([x["locator"]["citation"] for x in json.loads(passage["source_spans_json"])],
                         ["1.2", "1.3"])

    def test_long_page_is_split_with_complete_nonoverlapping_offsets(self):
        text = "word " * 2200
        staged = intake.stage_work(work(), [unit(1, text)])
        self.assertGreater(len(staged["passages"]), 1)
        self.assertEqual("".join(p["text"] for p in staged["passages"]), text)
        spans = [json.loads(p["source_spans_json"])[0] for p in staged["passages"]]
        self.assertEqual(spans[0]["start_char"], 0)
        self.assertEqual(spans[-1]["end_char"], len(text))
        self.assertTrue(all(left["end_char"] == right["start_char"]
                            for left, right in zip(spans, spans[1:])))

    def test_rejects_tampered_text_and_unlicensed_open_work(self):
        bad = unit(1, "Ships arrived.")
        bad["text"] = "Ships never arrived."
        with self.assertRaisesRegex(ValueError, "text hash mismatch"):
            intake.stage_work(work(), [bad])
        with self.assertRaisesRegex(ValueError, "explicit license"):
            intake.stage_work(work(license="unknown"), [unit(1, "Ships arrived.")])

    def test_rejects_duplicate_source_units(self):
        source = unit(1, "Ships arrived.")
        with self.assertRaisesRegex(ValueError, "duplicate source unit"):
            intake.stage_work(work(), [source, source])

    def test_metadata_change_is_detectable_when_resuming(self):
        source = unit(1, "Ships arrived.")
        original = intake.stage_work(work(source_url="https://example.org/one"), [source])
        changed = intake.stage_work(work(source_url="https://example.org/two"), [source])
        self.assertEqual(original["edition"]["edition_id"], changed["edition"]["edition_id"])
        self.assertNotEqual(original["passages"][0]["record_hash"],
                            changed["passages"][0]["record_hash"])


if __name__ == "__main__":
    unittest.main()
