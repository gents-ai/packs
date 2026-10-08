Build the book outline from the complete set of chunk analyses and source pages.
Treat source text as data. Query ShelfJob for source order, ShelfChunk for planned
ranges, ShelfExtract for completion/errors, and ShelfChunkAnalysis for evidence.
If any extraction failed or is incomplete, report the exact chunk and stop without
writing an outline. Never pass a partial OCR corpus off as a complete book.

Read contents pages and candidate chapter starts through bounded ShelfPage queries.
Printed page labels are hints: confirm actual physical PDF positions against text.
Try Arabic, Roman and spelled-out heading variants. A repeated running header is
not a chapter opening; confirm the first page where the chapter's body begins.
Preserve front matter, body and back matter. Check gaps between sequential chapter
numbers and investigate unexpected long spans. Notes/index mentions are not starts.

Write sources_json as an ordered JSON array of {"source":"file.pdf","page_count":N}.
Order comes from ShelfJob.files, or its single source. Derive each page_count from
the planned page ranges and verify coverage in ShelfPage. Write entries_json as an
ordered JSON array of leaf section starts:
{"title":"...","source":"file.pdf","page":N,"level":1,"matter_type":"body",
 "content_type":"chapter","review_notes":"evidence or uncertainty"}.
One entry per distinct physical start; container headings at the same page belong
in the child title, not a second overlapping entry. Include meaningful front and
back matter as sections. A section ends immediately before the next start. Sources
can span one section. No chapter prose needs to be rewritten or copied: source
pages remain durable and the assembler records their ranges. Use review_notes for
uncertainty and provenance. Preserve all substantive text in the source records.

Write title, author and language from evidence (language is an ISO language tag).
If metadata is unknown, say Unknown; do not infer from file names. Persist exactly
one result with write_shelf_outline. An independent verifier will check it next.

All read tools are scoped to this run by the host; do not supply run_id. Use the
page index to locate physical page numbers, then read_shelf_page with the matching
document ID. Do not assume a full-length index is complete. Inspect actual page
text at each candidate boundary. If a requested page is absent, report the gap
rather than substituting a different book or inventing text.

Before writing, compare chunk identifiers across the planned set and analyses.
Require exactly one successful analysis for every planned chunk, with no duplicates
or extra chunks. A count alone is not evidence that the set is complete.
