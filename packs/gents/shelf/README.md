# Shelf

## Configuration

Install OCR separately, then Shelf (automatic dependencies currently accept only
graph packs). Use absolute paths or `./packs/...` for local packs.

```sh
gents pack build ./packs/gents/ocr
gents pack install ./packs/gents/ocr --inference-slot document_reader=<profile>
gents pack build ./packs/gents/shelf
gents pack install ./packs/gents/shelf --inference-slot librarian=<profile> --inference-slot reader=<profile>
```

Shelf's OCR callbacks pin the OCR artifact declared in pack_config.json. Install
that artifact before running jobs. Bind librarian to a tool-capable model. Allow
read access to the scan folder through `gents plugin dirs add <folder>`.

## Usage

Start `gents server`, then create a ShelfJob with a unique run_id, path and optional
ordered files list using `gents document create ShelfJob --json '<fields>'`.
The path names one file, or a folder whose files list names one book in source order.
Omit files for a single PDF. Keep unrelated books out of the same job.

OCR callbacks persist chunks, extracted pages and figures. Chunk analysis agents
identify contents and heading evidence; a grouped outline agent resolves chapter
starts; a verifier checks source pages; native assembly writes ShelfBook,
ShelfChapter and ShelfStructureReport with complete scan-page coverage. Each chapter
references the original pages through source_ranges_json. Select the Shelf behavior
to inspect the library. A failed extraction prevents structure publication.

EPUB export currently supports text editions. Audio, rich layout and Shelf's custom
web interface are not included.

From the packs checkout, an isolated run and validated JSON export can be started with:

```sh
python3 scripts/shelf-book.py run /path/to/book.pdf --endpoint http://host:8000/v1 --directory runs/shelf/book
```

Use `--structure-endpoint` for a second model server. For a multi-part book, pass
its files in reading order. The script retains its isolated home and checks that
every extracted page occurs exactly once in the exported chapter ranges.
Use `--max-concurrent N` to set each backend's simultaneous request limit.

For multiple independent books, install Shelf once in a running Gents home and
grant read access to the source directories. Submit a JSON manifest to that home:

```json
[
  {"run_id": "book-one-v1", "sources": ["scans/one.pdf"]},
  {"run_id": "book-two-v1", "sources": ["scans/two-part-1.pdf", "scans/two-part-2.pdf"]}
]
```

```sh
python3 scripts/shelf-book.py submit-batch --home /path/to/home --manifest books.json
```

Paths are relative to the manifest. The command enqueues every book without
waiting for another to finish. Runtime backend limits govern execution; trigger
groups and agent reads are scoped by each unique run ID. Re-submitting the same
manifest skips existing identical jobs; changed input needs a new run ID. Submit
from one operator at a time. This command submits jobs, not a completion receipt.

To prepare a reviewed text edition from validated structured JSON, grant write
access to a separate export directory containing that JSON, then create a
`ShelfPrepareJob` with `run_id` (a new edition ID), `book_id`, `path`, `structured`
(the JSON filename), `modified` (UTC timestamp), and `output` (EPUB filename).
Preparation records mechanical corrections and fans out focused agent reviews.
Accepted exact edits retain passage IDs and original source byte spans; invalid
proposals get two repair attempts. After every planned review succeeds, callbacks
publish ShelfPassage records, an audited JSON edition and the EPUB. The EPUB has
chapter navigation and anchors matching the passage records. Inspect
ShelfEditionFailure for exhausted edit repairs. Agent/runtime failures may also
leave a review outstanding; no incomplete edition is published. Editions above
256 review chunks are refused before publication because of the runtime's group
limit. This path currently handles page-based structured input, not direct EPUB,
HTML or TEI intake.

The launcher enables the local read-only query MCP surface for long-field export.
To export an existing run, start its server with `--enable-mcp` and
`--mcp-query-collection` grants for ShelfBook, ShelfChapter, ShelfPage and ShelfExtract,
then run `scripts/shelf-book.py export --home /path/to/home --run-id RUN
--output /path/to/book.json --mcp-endpoint http://127.0.0.1:PORT/mcp`.
The artifact retains OCR markdown, page provenance and review notes; it does not
claim corrected prose or restored layout.
