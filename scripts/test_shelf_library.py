"""Exercise native BM25 and citation reads in an isolated Gents home."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import subprocess
from types import SimpleNamespace
import unittest

spec = importlib.util.spec_from_file_location("shelf_book", Path(__file__).with_name("shelf-book.py"))
shelf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shelf)


class NativeLibrary(unittest.TestCase):
    def test_multilingual_terms_current_editions_access_and_exact_citations(self):
        with tempfile.TemporaryDirectory(prefix="shelf-search-") as directory:
            home = Path(directory) / "home"
            shelf.call("init", "--home", home, "--agent-name", "search-test", "--backend-preset", "vllm",
                       "--inference-url", "http://127.0.0.1:1/v1", "--model-name", "unused")
            for name in ["shelf_library_passage", "shelf_library_edition"]:
                shelf.call("schema", "apply", shelf.ROOT / "packs/gents/shelf/schemas" / (name + ".graphql"), "--home", home)
            texts = {"en": "Egypt grain ships arrive at Ostia.",
                     "fr": "Les navires transportent le blé au port.",
                     "la": "Naves frumentum ad portum portant.",
                     "el": "πλοῖα σῖτον εἰς τὸν λιμένα φέρουσιν."}

            def publish(book, edition, text, language="en", access="open", modified="2026-10-08T00:00:00Z"):
                common = dict(run_id=edition, book_id=book, edition_id=edition, title=book, author="Fixture",
                              language=language, access=access, license="CC0-1.0", status="reviewed",
                              revision=edition, source_hash="fixture-source", source_hash_scope="structured_source")
                shelf.call("document", "create", "ShelfLibraryEdition", "--home", home,
                           "--json", json.dumps(dict(common, modified=modified, passage_count=1)))
                row = dict(common, record_id=edition + "-p", passage_id="p", chapter_title="Chapter",
                           text=text, text_hash=hashlib.sha256(text.encode()).hexdigest(),
                           source_spans_json='[{"source":"fixture.pdf","page":1,"start_byte":0,"end_byte":10}]',
                           epub_href="OEBPS/s.xhtml#p")
                shelf.call("document", "create", "ShelfLibraryPassage", "--home", home, "--json", json.dumps(row))
                return row

            rows = {lang: publish(lang, "edition-" + lang, text, lang) for lang, text in texts.items()}
            publish("en", "old-en", "Obsoleteunique grain fleet.", modified="2025-01-01T00:00:00Z")
            publish("private", "private-edition", "grain grain grain privateunique", access="local_only")
            publish("changed", "formerly-open", "formeropenunique", modified="2025-01-01T00:00:00Z")
            publish("changed", "now-private", "formeropenunique", access="local_only")
            args = SimpleNamespace(home=home, text="grain", limit=10, book_id=None, edition_id=None,
                                   language=None, access="open")
            for language, term in [("en", "grain"), ("fr", "navires"), ("la", "frumentum"), ("el", "σῖτον")]:
                args.text = term
                result = shelf.search_library(args)
                self.assertEqual([r["book_id"] for r in result["results"]], [language])
                self.assertGreater(result["results"][0]["_score"], 0)
            args.text = "absentuniqueterm"
            self.assertEqual(shelf.search_library(args)["results"], [])
            args.text = "formeropenunique"
            self.assertEqual(shelf.search_library(args)["results"], [])
            args.text = "Obsoleteunique"
            self.assertEqual(shelf.search_library(args)["results"], [])
            args.edition_id = "old-en"
            self.assertEqual(shelf.search_library(args)["results"][0]["edition_id"], "old-en")
            args.edition_id = None
            args.text = "privateunique"
            self.assertEqual(shelf.search_library(args)["results"], [])
            args.access = "local_only"
            self.assertEqual(shelf.search_library(args)["results"][0]["book_id"], "private")
            for lang in texts:
                opened = shelf.open_passage(SimpleNamespace(home=home, book_id=lang, edition_id="edition-" + lang,
                                                           passage_id="p", mcp_endpoint=None))
                self.assertEqual(opened["text"], rows[lang]["text"])
                self.assertEqual(opened["epub_href"], rows[lang]["epub_href"])
            # The immutable record identity refuses a duplicate instead of multiplying search hits.
            duplicate = subprocess.run([shelf.GENTS, "document", "create", "ShelfLibraryPassage", "--home", str(home), "--json", json.dumps(rows["en"])], capture_output=True, text=True)
            self.assertNotEqual(duplicate.returncode, 0)
            self.assertIn("already exists", duplicate.stderr)
            args.access = "open"
            args.text = "grain"
            self.assertEqual(shelf.search_library(args)["returned_count"], 1)


if __name__ == "__main__":
    unittest.main()
