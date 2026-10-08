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

The launcher enables the local read-only query MCP surface for long-field export.
To export an existing run, start its server with `--enable-mcp` and
`--mcp-query-collection` grants for ShelfBook, ShelfChapter, ShelfPage and ShelfExtract,
then run `scripts/shelf-book.py export --home /path/to/home --run-id RUN
--output /path/to/book.json --mcp-endpoint http://127.0.0.1:PORT/mcp`.
The artifact retains OCR markdown, page provenance and review notes; it does not
claim corrected prose or restored layout.
