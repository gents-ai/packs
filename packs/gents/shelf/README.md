# Shelf

## Configuration

Install OCR separately, then Shelf (automatic dependencies currently accept only
graph packs). Use absolute paths or `./packs/...` for local packs.

```sh
gents pack build ./packs/gents/ocr
gents pack install ./packs/gents/ocr --inference-slot document_reader=<profile>
gents pack build ./packs/gents/shelf
gents pack install ./packs/gents/shelf --inference-slot librarian=<profile> --inference-slot reader=<profile> --inference-slot page_vision=<vision-profile>
```

Shelf's OCR callbacks pin the OCR artifact declared in pack_config.json. Install
that artifact before running jobs. Bind librarian to a tool-capable model. Allow
read access to the scan folder through `gents plugin dirs add <folder>`.

## Usage

Start `gents server`, then create a ShelfJob with a unique run_id, path and optional
ordered files list using `gents document create ShelfJob --json '<fields>'`.
The path names one file, or a folder whose files list names one book in source order.
Omit files for a single PDF. Keep unrelated books out of the same job.

OCR callbacks persist source pages and figures. The launcher creates a source
capsule from the complete OCR receipt, retaining scan coordinates and immutable
PDF copies for visual inspection. ToC discovery checks every contents page and
its continuation boundary; extraction preserves printed labels and hierarchy.
Parallel agents locate individual entries, discover missing sequences and
investigate gaps. Native handoffs validate the findings before chapter text
review starts. Unresolved findings remain in ShelfStructureFailure.

ShelfChapter retains parentage, numbering, exact page ownership and narration
inclusion with its reason. The reviewed structured artifact includes a human
book hierarchy with ordered paragraphs and stable EPUB/source citations.
EPUB export supports text editions and nested contents navigation. TTS rendering
and Shelf's web interface are not included.

From the packs checkout, an isolated run and validated JSON export can be started with:

```sh
python3 scripts/shelf-book.py run /path/to/book.pdf --endpoint http://host:8000/v1 --directory runs/shelf/book
```

Add `--epub` to continue through editorial review, EPUB export and indexing; the
launcher prints the EPUB path after its index receipt arrives. Use `--book-id`
to retain a work identity across separate runs. Catalog rights default to
`local_only` / `unknown`; supply `--access` and `--license` when known. An open
work requires an explicit license.

Use `--structure-endpoint` for a second model server. For a multi-part book, pass
its files in reading order. The script retains its isolated home and checks that
every extracted page occurs exactly once in the exported chapter ranges.
Use `--max-concurrent N` to set each backend's simultaneous request limit.
For multiple complete books in one runtime, pass `run --manifest books.json --epub`
instead of positional sources. The manifest is an array of
`{"book_id":"stable-id","sources":["part-1.pdf","part-2.pdf"]}` objects;
paths are relative to the manifest. Each book gets a separate export folder,
while model requests share the backend limits. The launcher waits for every
book's export and index receipt and surfaces failed native callbacks.

Check a completed artifact with `python3 scripts/shelf-validate.py --structured
/path/to/plan-…-book.json --epub /path/to/book.epub` (requires `epubcheck`). This
checks source coverage, chapter order, navigation, and passage links; reading
quality still requires comparison with the source.

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

Each successfully exported EPUB triggers native indexing into ShelfLibraryEdition and
ShelfLibraryPassage. DefraDB maintains a BM25 full-text index over the final
reviewed passage text. No embedding service or model call is needed for indexing.
Select the `shelf-research` behavior to search and open cited passages using
read-only datastore tools in the same Gents instance.

### Direct source-text intake

For a corpus that already has extracted text, `scripts/shelf-intake.py` stages
format-normalized units and loads them into the **same** ShelfLibraryEdition and
ShelfLibraryPassage collections. This lane uses no OCR, embedding model or polish
agents. Its editions and passages have `status: source_text`; that label means
their locators and hashes were checked, **not** that the words passed Shelf's
review. Search and the research behavior include both source-text and reviewed
editions and show the status on every hit. A later reviewed edition supersedes
the source-text edition in default search while old citations remain open.

The catalog is JSONL with one work per row: `work_id`, `source_sha256`, `title`,
`authors`, `language`, `access`, `license`, `source_format`, and `source_path`.
Units are JSONL grouped by `work_id`, each with `unit_id`, the matching
`source_sha256`, `locator_kind`, a format-specific `locator`, `char_start`,
`char_end`, `text`, and `text_sha256`. A PDF unit locator carries a physical
`page`; EPUB uses `spine_index`, `href`, and character offsets; HTML/TEI and
unpaginated text use their real element, marker, or character spans; form feeds
in standalone text are recorded as form-feed pages, not PDF pages; ancient
sections use `citation`. No format is assigned an invented PDF page. Upstream
extractors may provide these units from raw files, and the staging command
rejects mismatched hashes, unknown works, or noncontiguous work groups.
`scripts/shelf-normalize.py` is a local producer for a catalog of raw source
paths: it sniffs PDF/EPUB/HTML/XML/TXT/directory content, verifies or fills
source hashes, follows EPUB OPF spine order, preserves HTML IDs and TEI `n`
labels, and retains PDF physical pages from a text layer or supplied page-aligned
text. A separate unpaginated text dump is kept unpaginated. The parser records
empty PDF pages for coverage; missing/poor text still needs selective OCR or
human review before making a reliable cited edition. Directory inputs can use
`sections_path` for a JSONL file of `{cit,text}` source sections.

```sh
python3 scripts/shelf-normalize.py --catalog raw-catalog.jsonl --root /path/to/sources --output-catalog catalog.jsonl --output-units units.jsonl
python3 scripts/shelf-intake.py stage --catalog catalog.jsonl --units units.jsonl --output staged-library
python3 scripts/shelf-intake.py load --home /path/to/home --staged staged-library --dry-run
python3 scripts/shelf-intake.py load --home /path/to/home --staged staged-library
```

The load and search commands require a Gents CLI with `document create` and
`query search`; set `GENTS=/path/to/new/gents` when another version is on PATH.

The loader resumes missing passages by stable record ID and writes an edition
receipt only after all passages exist. Re-running an identical staged corpus is
safe; a conflicting stored record hash or incomplete edition receipt stops the
load. Run one loader at a time per home. `source_hash_scope: original_source` refers to the catalog's hash of the
original source file or source directory; the stage does not independently
re-read the original. Preserve the catalog and unit files with a backup. The
original access category is kept in `source_metadata_json`; open-access,
author-copy and public-domain categories map to searchable `open`, whereas
unrecognized access stays `local_only`. Original license text is retained.
This operator path reads the raw formats above without creating a `ShelfJob`;
automatic selective OCR and polished edition assembly still use the separate
page-based Shelf workflow. Review a normalized unit's text and locator before
using it as a source of numerical or verbatim evidence.

```sh
python3 scripts/shelf-book.py search --home /path/to/home --text "grain ships" --access all
python3 scripts/shelf-book.py open-passage --home /path/to/home --book-id BOOK --edition-id EDITION --passage-id PASSAGE
```

Search defaults to open-access material, excludes zero scores, and selects the
latest reviewed edition per book, or the latest source-text edition when no
reviewed edition exists. Editions of the same status are ordered by UTC
`modified`. Ambiguous books are omitted with a warning from library-wide search;
a book-specific search requires an explicit `--edition-id` to resolve the tie.
Use `--edition-id` for historical research. License/access come from the
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
and stop-word quality are not established. Reviewed scans retain original-file
hashes. Language-specific analyzers, automatic selective OCR and a pack-owned
backup/restore workflow remain unfinished.

The launcher enables the local read-only query MCP surface for long-field export.
To export an existing run, start its server with `--enable-mcp` and
`--mcp-query-collection` grants for ShelfBook, ShelfChapter, ShelfPage and ShelfExtract,
then run `scripts/shelf-book.py export --home /path/to/home --run-id RUN
--output /path/to/book.json --mcp-endpoint http://127.0.0.1:PORT/mcp`.
The artifact retains OCR markdown, page provenance and review notes; it does not
claim corrected prose or restored layout.

The reviewed scan workflow currently requires PDFs. Its native plan supplies
source order, page counts, work identity, chunk identity and fan-in size. Agents
locate contents entries, investigate missing boundaries and classify verified
sections; host-filled fields preserve the book evidence through those handoffs.
Incomplete or non-PDF extractions cannot start discovery. Use source-text intake
for other formats.

OCR chunk reads resume through durable `ShelfReadRequest` documents when the
vision batch or time budget is exhausted. Only the completed chunk publishes
its pages and extraction receipt. Excessively large or non-progressing reads
fail explicitly. Accepted polish checkpoints are immutable: duplicate proposals
replay the accepted receipt and cannot consume repair attempts or replace text.

Use `run --book-id WORK` or `book_id` on a batch item to share a work identity
with direct source intake. Normalization records source modification time; an
operator can supply `modified` in the catalog to order revised transcriptions.
Metadata or text corrections produce a new source edition. Generated citation
prefixes and separators are recorded on each source span so exact passage text
can be reconstructed from source units.
