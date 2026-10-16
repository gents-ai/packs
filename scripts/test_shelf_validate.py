import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location("shelf_validate", Path(__file__).with_name("shelf-validate.py"))
validator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(validator)


class EditionValidation(unittest.TestCase):
    def fixture(self, root, *, href="chapter.xhtml", span_end=5, anchor="passage"):
        source = {"source": "scan.pdf", "page": 1, "start_byte": 0, "end_byte": span_end}
        book = {
            "edition": {"chapters": [{"id": "chapter", "title": "One", "blocks": [
                {"id": "passage", "markdown": "Text.", "sources": [source]}]}]},
            "original": {"sources": [{"source": "scan.pdf", "page_count": 1}], "chapters": [
                {"pages": [{"source": "scan.pdf", "page": 1, "markdown": "Text."}]}]},
            "passages": [{"passage_id": "passage", "epub_href": "OEBPS/chapter.xhtml#passage"}],
        }
        structured, epub = root / "book.json", root / "book.epub"
        structured.write_text(json.dumps(book))
        with zipfile.ZipFile(epub, "w") as z:
            z.writestr("META-INF/container.xml", '<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/book.opf"/></rootfiles></container>')
            z.writestr("OEBPS/book.opf", '<package xmlns="http://www.idpf.org/2007/opf"><manifest><item id="nav" href="nav.xhtml" properties="nav"/><item id="chapter" href="chapter.xhtml"/></manifest><spine><itemref idref="nav"/><itemref idref="chapter"/></spine></package>')
            z.writestr("OEBPS/nav.xhtml", f'<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><nav epub:type="toc"><a href="{href}">One</a></nav></html>')
            z.writestr("OEBPS/chapter.xhtml", f'<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="{anchor}">Text.</p></body></html>')
        return structured, epub

    def test_human_hierarchy_narration_and_paragraphs_match_the_epub_projection(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(validator.subprocess,"run"):
            structured,epub=self.fixture(Path(tmp))
            book=json.loads(structured.read_text());chapter=book["edition"]["chapters"][0]
            chapter.update(level=1,level_name="chapter",matter_type="body",content_type="body",audio_include=True,audio_include_reasoning="Narrative prose.")
            section={k:chapter[k] for k in ["id","title","level","level_name","matter_type","content_type","audio_include","audio_include_reasoning"]}
            section.update(sections=[],paragraphs=[{"id":"passage","ordinal":1,"text":"Text.","source_spans":chapter["blocks"][0]["sources"]}])
            book["book"]={"sections":[section]}
            structured.write_text(json.dumps(book));validator.validate(structured,epub)
            for key,value,error in [("audio_include",False,"metadata differs"),("paragraphs",[{"id":"passage","ordinal":1,"text":"Rewritten.","source_spans":chapter["blocks"][0]["sources"]}],"paragraphs differ")]:
                altered=json.loads(json.dumps(book));altered["book"]["sections"][0][key]=value
                structured.write_text(json.dumps(altered))
                with self.assertRaisesRegex(ValueError,error):validator.validate(structured,epub)

    def test_navigation_anchors_and_source_spans_are_checked_independently(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(validator.subprocess, "run") as check:
            root = Path(tmp)
            result = validator.validate(*self.fixture(root))
            self.assertEqual(result["passages"], 1)
            check.assert_called_once_with(["epubcheck", str(root / "book.epub")], check=True)
            for kwargs, error in [({"href": "wrong.xhtml"}, "table of contents"),
                                  ({"span_end": 10}, "source span"),
                                  ({"anchor": "missing"}, "passage anchor")]:
                with self.subTest(kwargs=kwargs), self.assertRaisesRegex(ValueError, error):
                    validator.validate(*self.fixture(root, **kwargs))


if __name__ == "__main__":
    unittest.main()
