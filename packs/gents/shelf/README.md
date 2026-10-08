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
For a vision-capable reader endpoint, add `--remote-ocr auto`. The launcher binds
that same endpoint to OCR's optional vision slot. Embedded PDF text is used first;
scanned pages go through bundled ocrs/RTen recognition, with image requests for
reads that fail the OCR pack's quality heuristic. `--remote-ocr force` sends every
scanned page through the vision model; `off` (the default) uses bundled OCR only.
The option is named remote_ocr by the OCR pack even when the model runs on a local
workstation. It does not select a cloud service. Automatic fallback detects
garbled output, not every fluent misreading, and downstream text agents do not
automatically receive every page image.

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

Batch submission also accepts `--remote-ocr auto`; first bind the installed OCR
pack's `remote_ocr` slot to the desired vision profile in that home.

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
New editions release at most 16 outstanding review chunks per book. Each accepted
review releases one successor through the same callback/document mechanism, so
one book cannot enqueue all its reviews ahead of later books. This bounds per-book
queue pressure; it does not promise round-robin scheduling or dynamic backend balancing.
Accepted exact edits retain passage IDs and original source byte spans; invalid
proposals get two repair attempts. After every planned review succeeds, callbacks
publish ShelfPassage records, an audited JSON edition and the EPUB. The EPUB has
chapter navigation and anchors matching the passage records. Inspect
ShelfEditionFailure for exhausted edit repairs. Agent/runtime failures may also
leave a review outstanding; no incomplete edition is published. Editions above
256 review chunks are refused before publication because of the runtime's group
limit. This path currently handles page-based structured input, not direct EPUB,
HTML or TEI intake.

Each prepared edition triggers native indexing into ShelfLibraryEdition and
ShelfLibraryPassage. DefraDB maintains a BM25 full-text index over the final
reviewed passage text. No embedding service or model call is needed for indexing.
Select the `shelf-research` behavior to search and open cited passages using
read-only datastore tools in the same Gents instance.

```sh
python3 scripts/shelf-book.py search --home /path/to/home --text "grain ships" --access all
python3 scripts/shelf-book.py open-passage --home /path/to/home --book-id BOOK --edition-id EDITION --passage-id PASSAGE
```

Search defaults to open-access material, excludes zero scores, and selects the
newest indexed edition per book by its UTC `modified` timestamp (edition ID breaks
ties). Use `--edition-id` for historical research. License/access come from the
structured source's `license` and `access`; absent values remain `unknown` and
`local_only`. These labels are catalog filters, not database authorization rules.
Use DefraDB authorization for readers who must not access restricted documents.

Indexed editions are immutable. Changed text needs a new edition ID and later
modified timestamp; default search excludes the earlier edition while old
citations still open. Stable record identities prevent duplicate indexed passages.
The callback publishes the edition receipt and all its passages transactionally.
Current `source_hash_scope` is `structured_source`: the hash identifies the source
JSON snapshot, not original PDF bytes. `text_hash` identifies exact reviewed text;
`revision` identifies the audited edition artifact. Source locations and EPUB
anchors accompany every hit. Long text requires `--mcp-endpoint` when opening,
with a read-only MCP grant for ShelfLibraryPassage; the opener verifies the text hash.

The initial index explicitly uses the English analyzer. Native tests cover exact
English, French, Latin and Greek terms; multilingual morphology, accent folding
and stop-word quality are not established. Original-file hashes, language-specific
analyzers, direct non-PDF intake and verified backup/restore remain unfinished.

The launcher enables the local read-only query MCP surface for long-field export.
To export an existing run, start its server with `--enable-mcp` and
`--mcp-query-collection` grants for ShelfBook, ShelfChapter, ShelfPage and ShelfExtract,
then run `scripts/shelf-book.py export --home /path/to/home --run-id RUN
--output /path/to/book.json --mcp-endpoint http://127.0.0.1:PORT/mcp`.
The artifact retains OCR markdown, page provenance and review notes; it does not
claim corrected prose or restored layout.
