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
