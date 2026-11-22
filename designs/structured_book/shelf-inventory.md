# Shelf book pipeline: migration inventory for Gents packs

Status: read-only inventory, phase 1. Nothing in `shelf`, `gents` or `packs` was modified.

Source root used throughout: `shelf/` = `$SRC/github.com/jackzampolin/shelf`.
Packs root: `packs/` = `$SRC/github.com/gents-ai/packs`.

Purpose: list everything that has to move so that PDF/scan -> OCR -> structured book runs inside a
gents runtime (DefraDB-backed) with no Shelf process. The proposed new pack is called
`packs/gents/structured_book/` below. That name is a placeholder.

---

## 0. Findings that change the migration plan

Read these before the stage detail. Each one is a place where the Shelf design does not map
directly onto the current pack model.

1. **Shelf is one big in-memory state machine. Gents is event-driven over documents.**
   `shelf/internal/jobs/process_book/job/job.go` (`Start`, `OnComplete`) and `state.go`
   (`MaybeStartBookOperations`, `CheckCompletion`) keep the stage gates in memory on `BookState`.
   DB writes are fire-and-forget (ADR `docs/decisions/010-async-writes-refactor.md`). Every gate
   that Shelf checks in memory has to become either a created document that an `event_source`
   watches, or a deterministic "gate" callback that counts documents and emits a readiness
   document. Examples: "pages 1..20 OCR'd" for metadata, "pages 1..30 OCR'd" for ToC finder, "all
   pages OCR'd" for link, "all entries linked", "all polish done". This is the biggest design task
   (WORK-LIST `TR-*`, `S-stage`).
2. **Agent tools are deterministic Go code that reads the book.** `grep_text`, `get_page_ocr` (with
   evidence), `get_heading_pages`, `get_frontmatter_grep_report` and `get_gap_context` all read page
   OCR through `BookState`. Pack plugins are sealed: no filesystem, network or env (packs README,
   "Plugins"). They get data only on stdin or through `bind_dir`. So each of these tools has to be
   either (a) a `datastore_tool_surfaces` `kind: "query"` read over `Page`, with the heuristics moved
   into a plugin that the model calls with text it already fetched, or (b) a plugin that is handed the
   page corpus by a callback. The ranking and evidence logic (`ocr_evidence*.go`, about 800 lines)
   cannot be a datastore query.
3. **The ToC link validator depends on Chandra `data-label` blocks.**
   `shelf/internal/agents/toc_entry_finder/tools/ocr_evidence_extract.go` finds `Page-Header`,
   `Page-Footer` and `Section-Header` blocks with the regex `data-label=["']…["']`. It also re-wraps
   `Page.header`/`Page.footer` as such blocks
   (`shelf/internal/jobs/common/page_reader_impl.go:GetOcrMarkdownWithPageFurniture`). The gents OCR
   pack (`packs/packs/gents/ocr`, `remote.rs:markdown`) turns remote HTML into plain Markdown and
   writes `OcrPage {run_id, chunk, source, page, markdown}`. It has no header or footer fields and no
   labels. An adapter has to rebuild `header`, `footer` and section headings, or the validator has to
   be rewritten against Markdown headings only. Highest-risk portability item.
4. **Several agents use vision.** `load_page_image` (toc_finder, toc_entry_finder, chapter_finder,
   gap_investigator) attaches a 300-dpi PNG from `~/.shelf/.../page_NNNN.png` to the next LLM turn
   (`shelf/internal/agent/agent.go`, `GetImages`). The OCR pack keeps no page images (only figure crops
   in `OcrFigure.image_base64`, and only when `figure_images` is set). Two things are open: whether
   GLM-5.3-Flash-NVFP4 accepts images, and where page rasters would live (see Open questions).
5. **Paragraphs are never written for scans.** Only `shelf/internal/epubimport/import.go` writes
   `Paragraph`. The scan pipeline stops at `Chapter.polished_text`. The research MCP falls back to one
   passage per chapter (`shelf/internal/researchmcp/client.go:loadSnapshot`).
6. **`source_sha256` is set only by EPUB import.** PDF ingest (`shelf/internal/ingest/job.go`) never
   sets it, so scan books feed an empty string into the research `structure_digest`. The new pack
   should hash the source PDF at ingest.
7. **The metadata prompt promises a tool that does not exist.** `shelf/internal/prompts/metadata/system.tmpl`
   says "with web search capability" and "Use web search to verify". The work unit
   (`metadata/workunit.go`) sends no tools. Port it as a no-tool structured call, or give it a search
   tool on purpose.
8. **Two prompts are Go string constants, not templates.** The structure classify and polish system
   prompts live in `shelf/internal/jobs/common/structure_prompts.go` (`ClassifySystemPrompt`,
   `PolishSystemPrompt`). `GetEmbeddedDefault` does not register them for book overrides. Several user
   prompts are built in Go (`BuildUserPrompt` in toc_entry_finder, chapter_finder and gap_investigator,
   and `BuildClassifyPrompt`/`BuildPolishPrompt`). When moving these to pack `tasks/*/prompt.md` they
   have to be turned into templates. That is not a plain copy.
9. **Plugins as graph stages and model tools.** The packs README says both call paths are "not wired
   yet", but the OCR pack README documents both `integrations.plugins` (model tool) and `callbacks`
   (plugin node). Before planning, confirm against gents v0.20.0 which of these works (see Open
   questions).
10. **Gents installs only graph packs compiled into its binary today** (OCR pack README, "graph
    node"). Prefer `event_sources` + `triggers` + `callbacks` in `pack_config.json`, as
    `packs/gents/pipeline` and `packs/gents/ocr` do, over a `graphs/*.plan.json`.

---

## 1. Pipeline overview

Entry points (`shelf/README.md`, "Source paths"):

| Source | Path | Inference |
|---|---|---|
| EPUB | `books import-epub` -> `epubimport.Import` -> terminal book | none |
| PDF with text layer | ingest -> `repair-pdf-text` (operator) -> downstream | structure only |
| Image-only scan | ingest -> extract -> OCR -> metadata/ToC -> link -> finalize -> structure | OCR + structure |

Job type `process-book` (`shelf/internal/jobs/process_book/process_book.go`). Variants (`Config.ApplyVariant`):
`standard` (all stages), `photo-book` and `text-only` (OCR + metadata), `ocr-only` (OCR only).
`ResetFrom` cascades through `shelf/internal/jobs/common/op_registry.go` (`CascadesTo`):
metadata (leaf); toc_finder -> toc_extract -> toc_link -> toc_finalize -> structure; plus `ocr`.

Stage gating, as written in `job/state.go:MaybeStartBookOperations`:

```
extract(page) -> ocr(page, each provider)
   |  pages 1..20 consecutive OCR-complete (OcrThresholdForMetadata=20) ---> metadata
   |  pages 1..30 consecutive OCR-complete (ConsecutiveFrontMatterRequired=30) ---> toc_finder
toc_finder done && toc_found ---> toc_extract
toc_extract done && ALL pages OCR-complete ---> link_toc (fan-out per TocEntry, 8 in flight)
link_toc all resolved ---> finalize: pattern -> discover (fan-out) -> validate/gap (fan-out) -> resort
finalize complete ---> structure: build -> extract -> classify (chunks of 64) -> polish (per chapter) -> finalize
CheckCompletion ---> Book.status = complete | degraded (quarantined pages) ; failures -> failed
```

Scheduler priorities (`shelf/internal/jobs/priority_queue.go:PriorityForStage`): all book-level and
agent stages are `PriorityHigh`, and `ocr`/`extract` are `PriorityNormal`. The queue is fair across
jobs. Provider concurrency is a per-provider pool (`rate_limit`, `max_concurrency`, ADR 006). The
gents equivalent is inference-profile/slot concurrency against the two vLLM hosts (about 32 each).

Retry constants (`job/types.go`, `job/finalize.go`, `job/structure.go`):
`MaxBookOpRetries=3`, `MaxPageOpRetries=10`, `MaxOCRPageRetries=0` (the provider owns its retries),
`MaxFinalizeRetries=3`, `MaxStructureRetries=3`. Provider HTTP retries: OpenAI-compatible LLM
7 attempts, 2 s delay, 500 s timeout, 150 rps (`providers/openai_compat.go`). Chandra OCR 5 retries,
16 concurrent, 50 rps, 12384 max tokens (`providers/chandra_ocr.go`). Structured-output self-repair:
up to 2 extra turns with a repair prompt when JSON fails to parse or fails schema validation
(`providers/openrouter_chat.go`, `providers/structured_output.go:maxStructuredRepairAttempts`).

---

## 2. Stage by stage

Collections are DefraDB types from `shelf/internal/schema/schemas/*.graphql`. "Async" means fire-and-forget
through `defra.Sink`.

### 2.0 Ingest (PDF)
- Code: `shelf/internal/ingest/job.go` (`Job.Start`), `shelf/internal/ingest/stitch.go` (`GroupParts`, `StitchPDF` for numbered parts, `--stitch`).
- Input: one or more PDF paths, title, author.
- Work: copies PDFs to `~/.shelf/books/<id>/originals/`, counts pages with pdfcpu (`api.PageCount`), and optionally stitches the parts into one PDF.
- Writes: `Book {title, author, page_count, status:"ingested", created_at}`. It does **not** create `Page` rows and does **not** hash the source.
- Concurrency/retries: synchronous, none.
- Done when: the Book doc exists and the files are copied.
- Kind: deterministic, so plugin/callback.

### 2.0b EPUB direct import
- Code: `shelf/internal/epubimport/import.go` (`Import`, `findExisting` dedupe by SHA-256, `purgeIncompleteImport`), `shelf/internal/epubimport/parser.go` (`Parse`, nav/NCX, spine, `classifyChapter` keyword rules -> matter_type/content_type/audio_include).
- Writes in one shot: `Book` (source_format=epub, source_sha256, metadata_complete=true, structure fields), one `Page` per chapter (ocr_markdown=chapter markdown, ocr_complete=true), `OcrResult{provider:"epub"}`, `ToC` (all stage flags complete, `structure_summary.source="epub_navigation"`), `TocEntry` (sort_order=(i+1)*100, source epub_navigation), `Chapter`, `Paragraph` (batched).
- Done when: Book status reaches complete synchronously. Re-importing the same bytes returns the existing book.
- Kind: deterministic. The OCR pack also reads EPUB to Markdown, but it does not produce this structured shape.

### 2.0c PDF text-layer repair (operator)
- Code: `shelf/internal/jobs/common/repair_pdf_text.go` (`RepairPDFTextPages`), `repair_ocr_text.go`. These replace image OCR for chosen pages with the PDF's embedded text. The OCR pack's `ocr: "auto"` mode makes this mostly unnecessary.

### 2.1 Extract (page raster)
- Code: `job/extract.go`, `common/extract.go:CreateExtractWorkUnit`, `shelf/internal/ingest/handler.go:ExtractPageHandler`.
- Input: `PDFList` from `common/pdf.go:LoadPDFsFromOriginals` (cumulative page ranges, `-N.pdf` numeric sort).
- Work: CPU unit runs `pdftoppm -png -r 300 -singlefile -f N -l N` and writes `page_%04d.png` under source images.
- Writes: `Page.extract_complete=true` (async). `Page` rows are created first by `common/pages.go:CreateMissingPages` (`_bookID`, `page_num`) inside `Job.Start`.
- Concurrency: CPU pool. Retries: up to 10 (`MaxPageOpRetries`).
- Done when: image is on disk and the flag is set, which emits the OCR units for that page.
- Kind: deterministic. In gents, the OCR pack rasterises internally (`pix.rs`, `pdfocr.rs`). A separate raster is needed only if vision tools stay (Finding 4).

### 2.2 OCR
- Code: `job/ocr.go:HandleOcrComplete`, `common/ocr.go` (`CreateOcrWorkUnit`, `PersistOCRResult`, `SaveExtractedImages`), providers `shelf/internal/providers/chandra_ocr.go` (prompt `DefaultChandraOCRPrompt`, HTML layout blocks with `data-label`), `chandra_parse.go` (HTML -> Markdown; `Page-Header`/`Page-Footer` pulled into headers/footers; `Section-Header` -> `## `; `Blank-Page` dropped; images -> `chandra-page-NNNN-image-NN.jpg`), `chandra_markdown.go`, `mistral.go` (alt provider, also returns header/footer and images).
- Input: page PNG bytes, one unit per (page, provider). Multiple providers are possible ("consensus blending" in the README). In current code the first non-empty provider text becomes `ocr_markdown`; no blending code was found.
- Writes: `OcrResult {_pageID, provider, text, provider_metadata}` (async create); `Page.header/footer` (async); `Page.ocr_complete=true` once all providers are done; then `Page.ocr_markdown` and `Page.headings` (JSON of `common/headings.go:ExtractHeadings`, regex `^(#{1,6})\s+(.+)$` with alnum filter). Extracted images go to disk, and the markdown refs are rewritten to `/api/books/<id>/pages/<n>/extracted-images/<img>`.
- Concurrency: provider pool (Chandra default 16, 50 rps). Retries: 0 at workflow level (`MaxOCRPageRetries=0`) and 5 in the provider. A failed page stays incomplete and the job fails visibly with the page number.
- Quarantine: `common/quarantine_ocr.go:QuarantineOCRPages` (operator) sets `ocr_quarantined=true` plus a reason and nulls the OCR fields. A book with quarantined pages ends `degraded`.
- Done when (per page): `ocr_complete=true` or quarantined. Book-level gates count consecutive pages from 1.
- Kind: model (OCR VLM) wrapped in deterministic parse. Target: the existing `gents/ocr` pack plus an adapter (P-ocr-adapter).

### 2.3 Metadata
- Code: `job/metadata.go`, `common/metadata_ops.go` (`CreateMetadataWorkUnit`, `LoadPagesForMetadataFromState`, `SaveMetadataResult`), prompt package `shelf/internal/prompts/metadata/`.
- Gate: pages 1..20 consecutively OCR-complete (`OcrThresholdForMetadata`).
- Input: the first 20 non-empty pages joined as `--- Page N ---\n<markdown>` (`metadata.PrepareBookText`).
- LLM: single structured call. See 3.1.
- Writes: `Book {title, subtitle, author, authors, isbn, publisher, publication_year, description, subjects, cover_page, metadata_complete:true}` (sync). Note that `lccn`, `language` and `contributors` are parsed but **not** persisted by `SaveMetadataResult`. `language` is in the schema but dropped.
- Retries: 3 (`MaxBookOpRetries`) on retriable provider errors and on handler parse errors.
- Done when: `metadata_complete=true`.
- Kind: model judgment (behavior) plus deterministic text prep and writer.

### 2.4 ToC finder (agent)
- Code: `job/toc_finder.go` (`CreateTocFinderWorkUnit`, `HandleTocFinderComplete`), factory `shelf/internal/agents/toc_finder_factory.go`, tools `shelf/internal/agents/toc_finder/tools/*`.
- Gate: pages 1..30 consecutively OCR-complete.
- Setup: creates a `ToC` row (`toc_found:false, finder_started:true, created_at`) if missing, then links `Book._tocID`.
- Agent: see 3.2. MaxIterations 25.
- Writes: `ToC {toc_found, start_page, end_page, structure_summary(JSON), finder_complete}` through `BookState.PersistTocFinderResultAsync` (`common/state_persist_toc_discovery.go`). AgentState rows for resume (create, checkpoint after every LLM result, delete when done).
- Retries: if `toc_found=false`, the finder is restarted fresh up to 3 times. After that the **job fails** ("this book may not have a ToC"). Provider errors: 3.
- Done when: `finder_complete` and `toc_found=true`.
- Operator override: `common/repair_toc_range.go:RepairTocRange` (`finder_override*` fields).

### 2.5 ToC extract
- Code: `job/toc_extract.go`, `common/toc.go` (`CreateTocExtractWorkUnit`, `LoadTocPagesFromState`, `LoadTocStructureSummary`, `SaveTocExtractResult`, `normalizeTocExtractEntries`), prompt package `shelf/internal/prompts/extract_toc/`.
- Gate: finder done and `toc_found`.
- Input: OCR markdown of pages start..end plus `structure_summary` from the finder.
- LLM: single structured call. See 3.3.
- Post-processing (deterministic): `normalizeTocExtractEntries` splits one known failure shape, a level>=2 title holding >=3 `title, NN.` anchors with strictly increasing pages, and strips a `Title: page` suffix at level 1 when that shape appears. Then it builds a replace-set: it deletes active TocEntries for the ToC and upserts each entry with `unique_key = "<tocID>:<uuid-generation>:<i>"` (this avoids Defra tombstone DocID reuse), `sort_order=i`, `link_retries=0`, `link_failed=false`, `link_excluded=false`. On a DocID collision it falls back to finding the entry by (toc, sort_order). Then it sets `ToC.extract_complete=true` and flushes the sink before reloading entries.
- Retries: 3 (provider and handler).
- Done when: `extract_complete=true` and entries are reloaded into memory.

### 2.6 ToC link (agent per entry)
- Code: `job/link_toc.go` (`CreateLinkTocWorkUnits`, `maxConcurrentLinkTocEntries=8`, `fillLinkTocConcurrency`), `job/link_toc_agents.go` (agent creation/resume, `maxLinkTocRetryHintBytes=2000`, `appendLinkTocRetryHint`), `job/link_toc_execution.go` (`HandleLinkTocComplete`, `validateTocLinkEvidence`, `failTocLinkEntry`), `job/link_toc_structure.go` (`tocEntryBookStructure`, `deriveBackMatterStart`, `deriveBackMatterTypes`, `tocEntryIsBackMatter`, `backMatterLabelFromText`), `common/toc.go:SaveTocEntryResult`, `common/toc_link_retry.go:PersistTocEntryLinkState`.
- Gate: extract done **and all pages OCR-complete**.
- Input: each unlinked `TocEntry` (LoadBook loads only entries without `actual_page`).
- Agent: see 3.4. 8 entries in flight per book. New ones are started as each resolves.
- Validation: twice. First inside the `write_result` tool, which rejects and keeps the agent going. Then post hoc in the job (`validateTocLinkEvidence`, same `ValidateCandidatePage` plus a back-matter check).
- Writes: `TocEntry._actual_pageID`, clears `link_failed*`/`link_excluded*` (versioned update). `Book.toc_link_entries_total/done`. On a reject: `TocEntry.link_retries`, `link_failure_reason` (fed back to the next attempt as `PREVIOUS REJECTION FEEDBACK`).
- Retries: 3 per entry with a fresh agent and an accumulated retry hint. When they run out, the entry gets `link_failed=true` and the reason, `ToC.link_failed`, and the **job fails closed** (no unresolved entry may reach complete).
- Restart: `job.go:reconcileTocLinkRecovery` treats DB links as the truth, deletes stale AgentStates, and resets finalize and structure if a link reopens.
- Done when: done == total, which sets `ToC.link_complete` and starts finalize.
- Operator repairs: `common/resolve_toc_entry.go` (link to an operator-verified page), `repair_toc_entry.go` (reset one entry's budget), `insert_toc_entry.go` (add a missing, already-linked entry), `exclude_toc_entry.go` (mark a non-content entry excluded).

### 2.7 Finalize ToC (pattern -> discover -> validate -> done)
- Code: `job/finalize.go` (`StartFinalizePhase`, `transitionToFinalizeValidate`, `completeFinalizePhase`, `resortEntriesByPage`, `MinGapSize=15`), `job/finalize_pattern.go`, `job/finalize_discover.go`, `job/finalize_validate.go`, `job/finalize_helpers.go`.
- Body range: from page-pattern boundaries if present (`buildPagePatternContext` is currently a stub that returns empty), otherwise min..max linked page, otherwise 1..N. Stored in memory.
- **Pattern** (one LLM call, see 3.5). Input: linked entries, candidate headings from `Page.headings` levels 1-2 in the body range (`loadCandidateHeadings`), detected chapters, and chapter start pages. Output is sanitized deterministically:
  - `sanitizeDiscoveredPatterns` requires `pattern_type=="sequential"`, non-empty fields, `heading_format` containing `{n}` plus a word anchor, level 1..6, and a generated sequence of length 1..500.
  - `discoveredPatternHasCandidateSupport` requires that >=2 sequence identifiers (1 for single-item sequences) appear in candidate headings that carry the format's anchor tokens, and that no identifier appears on more than one page.
  - `sanitizeExcludedRanges` keeps a range only if it starts in the second half of the book and its reason names a back-matter label.
  - `generateEntriesToFind` expands the ranges (arabic or roman, `generateSequence`), skips identifiers already in the ToC (by level+identifier), and estimates the page by interpolating between linked neighbours (`estimatePageLocation`). Search window is +-20 clamped to the body.
  - Persisted to `Book.pattern_analysis_json` plus `finalize_*` counters. On retry, `loadExistingPatternResults` reuses it.
  - Retries 3. After that it skips to discover.
- **Discover** (agent per EntryToFind, see 3.6). All entries start at once, with no per-book cap. A found page is deduped by `discoveredEntryAlreadyLinked`, then upserted as a `TocEntry {source:"discovered", sort_order=page*1000, _actual_pageID}` (`saveDiscoveredEntry`). Retries 3 per entry. Not-found is accepted.
- **Validate/gap**: runs only if pattern analysis produced EntriesToFind (`GetEntriesToFindCount()==0` skips it). `findFinalizeGaps` looks for linked-page spans >15 pages: body start -> first entry, between consecutive entries (skipping excluded ranges), last entry -> body end. Gap agent per gap (see 3.7). `applyGapFix`: `add_entry` upserts a `TocEntry {source:"validated", unique_key "<toc>:validated:<gapKey>", sort_order=page*1000}`; `correct_entry` repoints `_actual_pageID`; `flag_for_review` and `no_fix_needed` only log.
- **Done**: `resortEntriesByPage` rewrites `sort_order=(i+1)*100` by actual page (unlinked last). Then `ToC.finalize_complete` is persisted (3 attempts, 100 ms backoff) and structure starts.

### 2.8 Structure (build -> extract -> classify -> polish -> finalize)
- Code: `job/structure.go` (`StartStructurePhase`, `defraStructureWriteWithRetry` 4 attempts on transaction conflict), `job/structure_build.go`, `job/structure_extract.go`, `job/structure_classify.go`, `job/structure_polish.go`, `job/structure_completion.go`, helpers `common/structure_text.go`, `common/structure_classify.go`, `common/structure_prompts.go`, persistence `common/state_persist_chapters.go`.
- **Build** (deterministic): linked entries sorted by sort_order become `ChapterState {entry_id:"ch_%03d", title, level, level_name, entry_number, sort_order, source:"toc", toc_entry_id, start_page, end_page = next.start-1 (last = TotalPages), matter_type:"body"}`. Parent is the most recent chapter at level-1. Upserted as `Chapter` by `unique_key` (`generateChapterUniqueKey`; schema comment: `"{book_id}:{toc_entry_id}"` or `"{book_id}:orphan:{sort_order}"`). Stale chapters are deleted.
- **Extract** (deterministic): for each page in range, `StripHeaderFooter(ocr_markdown, header, footer)` then `CleanPageText` (drops page-number lines), then `MergeChapterPages` (`determineJoin` handles hyphenation and paragraph joins across page breaks), then `CountWords`. Writes `Chapter.mechanical_text, word_count, extract_complete`. Unchanged chapters reuse their prior polish (`reuseUnchangedPolish`, key = `structureChapterReuseKey`).
- **Classify** (LLM, chunks of 64 chapters, all chunks in parallel, see 3.8). `validateStructureClassifyCoverage` requires that all four maps (classifications, content_types, audio_include, reasoning) have exactly the chunk's entry_ids, with nothing missing or extra. Retries 3 per chunk, then the job fails. Writes `Chapter.matter_type, classification_reasoning, content_type, audio_include, audio_include_reasoning`.
- **Polish** (LLM per chapter where `audio_include=true`, all at once, see 3.9). Chapters with `audio_include=false` get `polished_text = mechanical_text` and `edits_applied_json="[]"` with no call. `ApplyEdits` does a first-occurrence `strings.Replace` per edit. Retries 3. After that it falls back to mechanical text with `polish_failed=true`, **and the structure then fails** ("mechanical fallback is not certifiable"). Writes `Chapter.polished_text, edits_applied_json, word_count, polish_complete|polish_failed, polish_retries`.
- **Finalize** (deterministic): `validatePersistedStructureChapters` re-queries `Chapter` and requires extract_complete, polish_complete (if polished), and non-empty matter_type, content_type and audio_include. Then `Book {structure_complete:true, total_chapters, total_words}`.
- Done when: `structure_complete=true`. Then `CheckCompletion` sets `Book.status` to `complete`, or `degraded` with a reason naming the first quarantined page. Failures go through `FailBook` (status `failed` plus status_reason, from `NoWorkFailure`).

### 2.9 Export (downstream; covers the "browser download" ask)
- Code: `shelf/internal/epub/builder.go` (EPUB 3 zip: mimetype, container, OPF, nav.xhtml, NCX, CSS, one XHTML per chapter), `epub/xhtml.go` (`markdownToXHTMLWithIDs`), `epub/media_overlay.go` and `smil.go` (Storyteller overlays), endpoints `shelf/internal/server/endpoints/books_export_epub.go` and `books_export_storyteller.go`.
- Input: `Book {title, author, language, publisher, isbn}` and `Chapter {entry_id, title, level, level_name, entry_number, matter_type, polished_text, sort_order}`.
- Kind: deterministic, a plugin that returns EPUB bytes as base64. A "browser download" is then a UI concern on the gents side.

---

## 3. LLM agents and prompts

Shared agent runtime: `shelf/internal/agent/agent.go`, `shelf/internal/agents/helpers.go`.
- Tool calls run synchronously in-process (`ExecuteToolLoop`). Only LLM turns are dispatched as work units.
- `MaxIterations` (default 15) gives a failed Result when exceeded.
- If the model answers without a tool call and the task is not complete, the runtime appends "You must call one of the available tools now: ...".
- `RequireToolUse` sets `tool_choice:"required"`.
- Only the **current** page image is attached, to the last message. History images are stripped and old tool results compacted (`compactOldToolResultsForRequest`).
- The agent counts as complete when the tool set's `IsComplete()` becomes true, which is when a `write_*` tool has accepted a result.
- Every LLM result is checkpointed to `AgentState` (`job/agent_checkpoint.go`) for crash resume. Gents sessions replace this.

Prompt resolution: `shelf/internal/prompts/resolver.go` gives a book override from `BookPromptOverride`,
otherwise the embedded default. Embedded prompts are synced to the `Prompt` collection with
`embedded_hash`. Every call records `prompt_key` and `prompt_cid` in `Metric`/`LLMCall`. Prompt keys
used by process-book: `shelf/internal/jobs/process_book/job/prompts.go`.

### 3.1 Metadata (one-shot structured)
- System: `shelf/internal/prompts/metadata/system.tmpl` (key `stages.metadata.system`).
- User: `shelf/internal/prompts/metadata/user.tmpl` (key `stages.metadata.user`, var `.BookText`).
- Tools: none (but see Finding 7).
- Output schema: `shelf/internal/prompts/metadata/schema.go:ExtractionSchema`, strict `book_metadata`. Required: `title`, `authors[]`, `language`, `confidence`. Nullable: subtitle, isbn, lccn, publisher, publication_year, description, cover_page. Also `subjects[]` and `contributors[{name,role}]`.
- Params: temperature 0.1, max_tokens 4096.
- Validation: provider schema validation plus up to 2 repair turns, then `metadata.ParseResult` (json.Unmarshal). No semantic checks.

### 3.2 ToC finder (agent)
- System: `shelf/internal/agents/toc_finder/system.tmpl` (298 lines, key `agents.toc_finder.system`).
- User: `shelf/internal/agents/toc_finder/user.tmpl` (key `agents.toc_finder.user`; vars ScanID, BookTitle, TotalPages, PreviousAttempt{AttemptNumber, Strategy, PagesChecked, Reasoning, StructureNotes}). Note: the factory calls `BuildUserPrompt(..., nil)`, so retry context is never actually supplied, and the user-prompt override is not applied.
- Factory: `shelf/internal/agents/toc_finder_factory.go` (MaxIterations 25, no forced tool use).
- Tools (`shelf/internal/agents/toc_finder/tools/`):
  - `get_frontmatter_grep_report` `{}` (`grep_report.go`): regex keyword categories (toc / front_matter / structure / back_matter) over pages 1..min(50, N); returns categorized_pages, page_details, a summary with clusters (`identifyClusters`). Cached. Deterministic.
  - `load_page_image` `{page_num:int (req), current_page_observations:string}` (`load_page_image.go`): **rejects** the call if a page is loaded and no observations are given. Records the observation and swaps the single image. Vision.
  - `load_ocr_text` `{}` (`load_ocr_text.go`): OCR markdown of the currently loaded page.
  - `write_toc_result` (`write_toc_result.go`) `{toc_found:bool, toc_page_range:{start_page,end_page}, confidence:0..1, search_strategy_used, reasoning, structure_summary:{total_levels 1..3, level_patterns{"1"/"2"/"3": {visual, numbering, has_page_numbers, semantic_type}}, consistency_notes[]}}`; required toc_found, confidence, search_strategy_used, reasoning.
- Output: `shelf/internal/agents/toc_finder/schema.go:Result`. Observations are compiled into `StructureNotes`.
- Validation: type-safe parsing only. There is no range check that start<=end<=N. Semantic gate: `toc_found=false` restarts the finder (3 attempts) and then fails the job.

### 3.3 ToC extract (one-shot structured)
- System: `shelf/internal/prompts/extract_toc/system.tmpl` (key `stages.extract_toc.system`).
- User: `shelf/internal/prompts/extract_toc/user.tmpl` (key `stages.extract_toc.user`; vars TocPages[{PageNum, OCRText}], TotalPages, StructureSummary).
- Output schema: `shelf/internal/prompts/extract_toc/schema.go:ExtractionSchema`, strict `toc_extraction`: `entries[{entry_number?, title, level 1..3, level_name?, printed_page_number?}]`.
- Params: temperature 0.1, max_tokens 32768.
- Validation: schema plus repair, then `normalizeTocExtractEntries` (2.5), which errors if a collapsed row's anchors do not cover the whole title or pages are not increasing.

### 3.4 ToC entry finder / link (agent per entry)
- System: `shelf/internal/agents/toc_entry_finder/system.tmpl` (118 lines, key `agents.toc_entry_finder.system`).
- User: built in Go, `shelf/internal/agents/toc_entry_finder/prompt.go:BuildUserPrompt`. The search term is level_name + entry_number + title. The expected scan window is printed_page+10..+35, clamped. It adds the retry hint, a decision rule, and back-matter context. `shelf/internal/agents/toc_entry_finder/user.tmpl` (key `agents.toc_entry_finder.user`) is a template version registered for overrides. The live path uses the Go builder.
- Factory: `shelf/internal/agents/toc_entry_finder_factory.go` (MaxIterations 25, `RequireToolUse`, `MaxToolCallsPerTurn` 4).
- Tools (`shelf/internal/agents/toc_entry_finder/tools/`):
  - `get_heading_pages` `{start_page?, end_page?}` (`get_heading_pages.go`): pages with level 1-2 headings, ranked by `headingPageResultScore` (target_title_match, target_title_prefix_match, entry_number_match, in_expected_scan_window).
  - `grep_text` `{query (req)}` (`grep_text.go`): regex/literal over all pages. Returns matches and snippets, clusters (`identifyClusters`), approximate title matches (Levenshtein-bounded, `ocr_evidence_text.go`), and a summary that flags back-matter.
  - `get_page_ocr` `{page_num (req)}` (`get_page_ocr.go`): compact OCR plus `PageEvidence`. If the evidence passes the validator it returns `write_result_ready:true` and ready-made `write_result_args`.
  - `load_page_image` `{page_num, current_page_observations}` (vision).
  - `write_result` `{scan_page:int, reasoning (req)}` (`write_result.go`).
- Output: `shelf/internal/agents/toc_entry_finder/schema.go:Result {scan_page?, reasoning}`.
- **Validation (deterministic, the core of link quality)**: `ocr_evidence.go:ValidateCandidatePage`. A page is rejected if:
  - it is outside 1..N;
  - it is inside the detected ToC page range;
  - it is in back matter (default start 0.9*N, or derived from pattern exclusions and back-matter labels) for a non-back-matter target.

  It is accepted if any of the following holds:
  - title_in_section_header, title_prefix_in_section_header or entry_number_in_section_header;
  - title_in_page_header **and** this page starts the title-header cluster (the previous page lacks it);
  - title_at_page_lead and not a contents-looking page and starts the lead cluster.

  Everything else is rejected with a specific reason: entry number only in body, title only in body, or not found. The evidence is built by `AnalyzePageEvidence`, `ocr_evidence_extract.go` (labeled-block regex, markdown heading lines, previous-page boundary evidence, printed page from headers, `expected_printed_page_missing`) and `ocr_evidence_text.go` (normalization, roman/number words, Levenshtein). A missing `scan_page` is also rejected, because linking requires a page.

### 3.5 Pattern analyzer (one-shot structured)
- System: `shelf/internal/agents/pattern_analyzer/system.tmpl` (key `agents.pattern_analyzer.system`).
- User: `shelf/internal/agents/pattern_analyzer/user.tmpl` (key `agents.pattern_analyzer.user`; vars LinkedEntries, Candidates, CandidatesTruncated, DetectedChapters, ChapterStartPages, BodyStart, BodyEnd, TotalPages), rendered by `prompt.go:BuildUserPrompt`.
- Output schema: `shelf/internal/agents/pattern_analyzer/prompt.go:JSONSchema`, strict `pattern_analysis`: `discovered_patterns[{pattern_type enum[sequential], level_name, range_start, range_end, level 1..6, heading_format, reasoning}]`, `excluded_page_ranges[{start_page, end_page, reason}]`, `reasoning`.
- Params: max_tokens = clamp(96 x entries, 4096, 32768) (`MaxOutputTokens`). No temperature is set.
- Validation: the deterministic sanitizers in 2.7. Rejects are logged and dropped, not retried.

### 3.6 Chapter finder (agent per missing sequence item)
- System: `shelf/internal/agents/chapter_finder/system.tmpl` (key `agents.chapter_finder.system`).
- User: Go builder `shelf/internal/agents/chapter_finder/prompt.go:BuildUserPrompt` (search term from heading_format with `{n}`, expected page and window, excluded ranges, a tip for a bare `{n}`).
- Factory: `shelf/internal/agents/chapter_finder_factory.go` (MaxIterations 15).
- Tools (`shelf/internal/agents/chapter_finder/tools/`): `get_heading_pages`, `grep_text` (marks excluded-range hits), `get_page_ocr` (plain OCR, no evidence object), `load_page_image`, `write_result {scan_page?, reasoning (req)}`.
- Output: `chapter_finder.Result {scan_page?, reasoning}`.
- Validation: range check only (out-of-range is dropped to "not found"), then the dedupe `discoveredEntryAlreadyLinked`. **No evidence validator**, so this is weaker than 3.4. Consider reusing P-link-evidence.

### 3.7 Gap investigator (agent per gap)
- System: `shelf/internal/agents/gap_investigator/system.tmpl` (key `agents.gap_investigator.system`).
- User: Go builder `shelf/internal/agents/gap_investigator/prompt.go:BuildUserPrompt` (gap pages and size, body range, position hint <10% / >90%, prev/next entry).
- Factory: `shelf/internal/agents/gap_investigator_factory.go` (MaxIterations 20).
- Tools (`shelf/internal/agents/gap_investigator/tools/`): `get_gap_context {}` (surrounding entries, body range, hints), `get_page_ocr {page_num}`, `load_page_image`, and `write_fix {fix_type enum[add_entry, correct_entry, no_fix_needed, flag_for_review] (req), reasoning (req), scan_page, title, level, level_name, entry_doc_id, new_scan_page}`. `add_entry` requires scan_page and `correct_entry` requires entry_doc_id; both are enforced in the tool.
- Output: `gap_investigator.Result`.
- Validation: required-field checks only. `applyGapFix` silently ignores scan_page 0 and does not range-check scan_page against the book.

### 3.8 Structure classify (one-shot structured, chunked)
- System: Go const `shelf/internal/jobs/common/structure_prompts.go:ClassifySystemPrompt` (key `stages.common_structure.classify.system`).
- User: Go builder `shelf/internal/jobs/common/structure_classify.go:BuildClassifyPrompt`. One line per chapter: title, pages, level, word_count, id, `content_signals` (`classifyContentSignals`: list density, reference keywords), and a 1000-char snippet (`buildClassifySnippet`).
- Output schema: `structure_prompts.go:ClassifyJSONSchema`, strict `entry_classifications`, four maps keyed by entry_id: classifications enum[front_matter, body, back_matter]; content_types enum of 20; audio_include bool; reasoning string.
- Params: max_tokens = clamp(96 x entries, 4096, 32768).
- Validation: `job/structure_classify.go:validateStructureClassifyCoverage` (exact key-set equality across all four maps).

### 3.9 Structure polish (one-shot structured, per chapter)
- System: Go const `structure_prompts.go:PolishSystemPrompt` (key `stages.common_structure.polish.system`).
- User: Go builder `structure_prompts.go:BuildPolishPrompt` (title plus mechanical text truncated at 120000 chars).
- Output schema: `structure_prompts.go:PolishJSONSchema`, strict `text_edits`: `edits[<=50]{old_text<=500, new_text<=500, reason<=200}`.
- Params: max_tokens = clamp(2048 + chars/2, 4096, 24576).
- Validation: schema only. Edits whose `old_text` is not found are silently no-ops in `ApplyEdits`. There is no check that edits preserve meaning.

### 3.10 OCR model prompt (for completeness)
- `shelf/internal/providers/chandra_ocr.go:DefaultChandraOCRPrompt` (Chandra `ocr_layout`). The OCR pack has its own copy in `packs/packs/gents/ocr/plugins/ocr/source/remote.rs`.

---

## 4. Data model (fields that matter for the pipeline)

DefraDB constraint: no NonNull (`!`) fields (`shelf/CLAUDE.md`). Relationships use `_<rel>ID` fields.

| Collection | File | Pipeline-relevant fields |
|---|---|---|
| Book | `schemas/book.graphql` | title, subtitle, author, authors, page_count, status (ingested/processing/complete/degraded/failed), status_reason; source_format, source_filename, source_sha256, source_identifier, source_imported_at; isbn, lccn, publisher, publication_year, language, description, subjects, cover_page; metadata_{started,complete,failed,retries}; structure_{started,complete,failed,retries,phase}; structure_chapters_{total,extracted,polished}, structure_polish_failed; total_chapters, total_paragraphs, total_words; pattern_analysis_json; finalize_{entries_total,entries_complete,entries_found,gaps_total,gaps_complete,gaps_fixes}; toc_link_entries_{total,done}; rels pages, toc(@primary), chapters, agent_states |
| Page | `schemas/page.graphql` | book, page_num, ocr_markdown, headings(JSON [{level,text,line_number}]), header, footer, extract_complete, ocr_complete, ocr_quarantined, ocr_quarantine_reason, ocr_results |
| OcrResult | `schemas/ocrresult.graphql` | page, provider, text, confidence, provider_metadata(JSON, images), created_at |
| ToC | `schemas/toc.graphql` | book, created_at, toc_found, start_page, end_page, structure_summary(JSON), finder_override{,_reason,_at}; {finder,extract,link,finalize}_{started,complete,failed,retries}; finalize_phase; entries |
| TocEntry | `schemas/tocentry.graphql` | toc, unique_key, entry_number, title, level, level_name, printed_page_number, actual_page(Page), link_retries, link_failed, link_failure_reason, link_failed_at, link_repair_reason, link_repaired_at, link_excluded, link_exclusion_reason, link_excluded_at, source (extracted/discovered/validated/epub_navigation), sort_order |
| Chapter | `schemas/chapter.graphql` | book, toc_entry, unique_key(@index), entry_id, sort_order, title, level, level_name, entry_number, start_page, end_page, matter_type, classification_reasoning, content_type, audio_include, audio_include_reasoning, parent_id, source, mechanical_text, polished_text, word_count, edits_applied_json, extract_complete, polish_complete, polish_failed, polish_retries, paragraphs |
| Paragraph | `schemas/paragraph.graphql` | chapter, sort_order, start_page, raw_text, polished_text, word_count, edits_applied (EPUB import only) |
| AgentState | `schemas/agentstate.graphql` | agent_id, agent_type, entry_doc_id, iteration, complete, messages_json, pending_tool_calls, tool_results, result_json, book (resume only; replaced by gents sessions) |
| AgentRun | `schemas/agentrun.graphql` | debug trace (replace with gents telemetry) |
| Prompt / BookPromptOverride | `schemas/prompt.graphql`, `bookpromptoverride.graphql` | key, text, variables, embedded_hash / book_id, prompt_key, text (replace with pack prompt assets; per-book overrides are an open question) |
| LLMCall / Metric | `schemas/llmcall.graphql`, `metric.graphql` | cost and trace per call (replace with gents run telemetry; local inference is zero cost, ADR 011) |
| Job / Config | `schemas/job.graphql`, `config.graphql` | Shelf scheduler and config (drop) |
| Audio* / Voice | `schemas/audio.graphql`, `voice.graphql` | TTS, out of scope |

In-memory-only state that must become documents in gents (`shelf/internal/jobs/common/state_*.go`):
`EntryToFind` (`state_finalize.go`), `FinalizeGap`, `FinalizePatternResult` (persisted only as
`Book.pattern_analysis_json`), body range (`SetBodyRange`), the link progress counters, and the
classify chunks. Each fan-out (link entry, discover entry, gap, classify chunk, polish chapter)
needs a document type so that a trigger can fire per item.

---

## 5. Research MCP read surface

Code: `shelf/internal/researchmcp/` (`tools.go`, `client.go`, `types.go`, `http.go`).
`shelf mcp --shelf-url ... --port ...` serves streamable-HTTP MCP at `/mcp` and `/healthz`. It is
read-only and talks only to Shelf's public HTTP API: `GET /api/books/{id}` and
`GET /api/books/{id}/chapters?include_paragraphs=true`.

Snapshot (`client.go:loadSnapshot`):
- Chapters are sorted by sort_order and paragraphs by sort_order.
- A passage is one per paragraph (polished_text, else raw_text, trimmed, empties skipped). If a chapter has no paragraphs, the passage is the chapter's trimmed polished_text. Scans therefore produce one passage per chapter (Finding 5).
- `content_hash = "sha256:" + sha256(text)`.
- `structure_digest = "sha256:" + SHA-256` over the NUL-separated stream: `shelf-research-v1`, book id, source_sha256, then per chapter (`chapter`, id, sort_order, title, start_page, end_page) and per passage (`passage`, chapter_id, paragraph_id, text).
- `requireResearchReady` requires structure_complete && !structure_failed && >=1 passage.

Tools:
| Tool | Input | Bounds | Output |
|---|---|---|---|
| `shelf_get_book` | book_id | none | book metadata, structure_digest, chapter/passage counts, research_ready |
| `shelf_list_structure` | book_id, matter_type?, offset, limit | limit 50 default, 200 max | chapter views (no text), next_offset |
| `shelf_search_passages` | book_id, query, regex?, case_sensitive?, chapter_ids?, matter_type?, top_k, snippet_chars | top_k 10/50; snippet 500/1200 runes | matches with rune offsets, snippet window, content_hash, truncated |
| `shelf_read_passage` | book_id, chapter_id (req), paragraph_id?, start_char, context_before, max_chars | before 300/2000; max 4000/12000 runes | text window, has_before/has_after, content_hash |
| `shelf_validate_quote` | book_id, quote, chapter_id?, paragraph_id?, expected_source_sha256?, expected_structure_digest? | 20 occurrences | exact_match, version_match, occurrences with 180-rune context |

Exact-quote validation logic (`tools.go:validateQuote`):
1. Load the snapshot and require research-ready.
2. `quote = strings.TrimSpace(quote)`. An empty quote is an error.
3. `version_match` = (expected_source_sha256 empty or equal) AND (expected_structure_digest empty or equal). On a mismatch it returns `exact_match=false` with "source version mismatch; refresh the assignment before citing" and does **no** search.
4. Filter to chapter_id/paragraph_id if given. For each passage, run repeated `strings.Index` on the raw bytes: case-sensitive, whitespace-sensitive, no Unicode normalization, no smart-quote folding. Occurrence offsets are converted to rune indices, with ±180 runes of context.
5. Stop at 20 occurrences (`truncated=true`). `exact_match = len(occurrences) > 0`.

Search (`compileMatcher`): literal via `regexp.QuoteMeta` unless `regex=true` (RE2); `(?i)` unless case_sensitive.

Port note: everything here is deterministic, and the inputs are the `Chapter`/`Paragraph` documents.
In a pack it is a read-only plugin, or a set of `datastore_tool_surfaces` `kind:"query"` reads plus
a pure `validate_quote` plugin that receives the passage texts. The digest algorithm must be kept
**byte-for-byte**, or existing citations pinned to old digests stop validating.

---

## 6. Deterministic code (plugin candidates) versus model judgment (behavior candidates)

**Deterministic, so afterburner plugins or callbacks:**
- Ingest/raster/stitch/hash (`ingest/*`, `common/pdf.go`). Mostly covered by the `gents/ocr` pack, except a source hash and page rasters for vision.
- Chandra HTML -> Markdown, header/footer split, heading extraction (`providers/chandra_parse.go`, `common/headings.go`). The OCR pack covers the Markdown but drops the header/footer split.
- Front-matter grep report (`toc_finder/tools/grep_report.go`).
- ToC extract normalization and replace-set write (`common/toc.go`).
- Back-matter derivation (`job/link_toc_structure.go`).
- Page search tools: grep with clusters, heading ranking, compact OCR (`toc_entry_finder/tools/{grep_text,get_heading_pages,get_page_ocr}.go`, `chapter_finder/tools/*`).
- **Link evidence validator** (`toc_entry_finder/tools/ocr_evidence*.go`, `job/link_toc_execution.go:validateTocLinkEvidence`).
- Pattern sanitizers, sequence generation, page estimation (`job/finalize_helpers.go`, `job/finalize_pattern.go:generateEntriesToFind`).
- Gap detection (`job/finalize_validate.go:findFinalizeGaps`), gap fix/discovered-entry writes, resort (`job/finalize.go`, `finalize_helpers.go`).
- Chapter skeleton, hierarchy, text extraction/merge/clean (`job/structure_build.go`, `job/structure_extract.go`, `common/structure_text.go`).
- Classify prompt builder and coverage validator; polish ApplyEdits/reuse; structure completion validation (`job/structure_*.go`).
- Stage gates, counters, retry budgets, terminal status (`job/state.go`, `job/job.go`).
- EPUB import parser and classifier (`epubimport/*`); EPUB export (`epub/*`).
- Research snapshot, digest, search, read, validate_quote (`researchmcp/*`).
- Operator repairs (`common/{repair_*,resolve_toc_entry,insert_toc_entry,exclude_toc_entry,quarantine_ocr}.go`).

**Model judgment, so behaviors:**
- metadata (one-shot), toc_finder (agent, vision), toc_extract (one-shot), toc_entry_finder (agent, per entry), pattern_analyzer (one-shot), chapter_finder (agent, per item), gap_investigator (agent, per gap), structure_classify (one-shot, chunked), structure_polish (one-shot, per chapter).
- Of these, metadata, toc_extract, pattern_analyzer, classify and polish are pure prompt -> JSON with no tools. In gents they can be behaviors with an empty tool set whose output is written through a single datastore write surface, or (if gents supports it) structured-output tasks.

---

## 7. Open questions

1. Does GLM-5.3-Flash-NVFP4 on the vLLM hosts accept image inputs? If not, `load_page_image` is dropped from all four agents and `toc_finder` must work from OCR text plus the grep report alone. That changes its 298-line prompt.
2. Where do page rasters live in gents if vision stays? The OCR pack has no page-image output, and plugins cannot write files.
3. In gents v0.20.0, can a plugin be a **model tool** (`tools[].integrations.plugins`) and a **callback** node at the same time, and can a model-tool plugin get DefraDB rows as input? The packs README and the OCR README disagree. The answer decides between "agent calls plugin with page text it fetched through a datastore query" and "plugin is handed the corpus".
4. Does gents support per-behavior structured output (json_schema response_format with repair), or must one-shot stages "call a write tool once"? Shelf depends on strict schemas plus up to 2 self-repair turns.
5. Can a trigger express "fire when N documents with field X exist" (a count gate), or do we need gate callbacks that re-count on every `Page`/`TocEntry` update? Does gents expose `updated` event_kind, or only `created`?
6. Trigger retry semantics: is there a per-trigger retry budget, and a way to pass a retry hint (Shelf's `link_failure_reason` -> `PREVIOUS REJECTION FEEDBACK`) into the next task run?
7. Per-trigger concurrency caps (Shelf: 8 link agents per book; others unbounded and limited by the provider pool). Is `concurrency: "parallel"` bounded per profile/slot?
8. Should the pack adapt `OcrPage` into a `Page` collection with Shelf's field names (so validators port unchanged), or should the validators be rewritten against `OcrPage.markdown`? This includes rebuilding header/footer furniture without `data-label` (Finding 3).
9. Do we migrate existing Shelf DefraDB books into the pack schema, or only process new ones?
10. Keep per-book prompt overrides (`BookPromptOverride`)? Gents prompts are pack assets.
11. Does the research MCP move into the pack (gents-native read surface), or keep running as a separate MCP pointed at the gents GraphQL? The digest must stay compatible either way.

---

## 8. WORK-LIST

Target root: `packs/packs/gents/structured_book/` (placeholder name). In the Target column, `cfg` means `pack_config.json`.
Size: S under 1 day, M 1-3 days, L more than 3 days. "Mechanical" in Notes marks prompt copies that a smaller model can do.

| id | kind | source paths (shelf/internal/...) | target in pack | size | dependencies |
|---|---|---|---|---|---|
| S-book | schema | `schema/schemas/book.graphql` | `schemas/book.graphql` (pipeline subset + `run_id @index` correlation + source_sha256) | S | none |
| S-page | schema | `schema/schemas/page.graphql`, `ocrresult.graphql` | `schemas/book_page.graphql` (page_num, ocr_markdown, headings, header, footer, ocr_complete, quarantine) | S | S-book |
| S-toc | schema | `schema/schemas/toc.graphql`, `tocentry.graphql` | `schemas/toc.graphql`, `schemas/toc_entry.graphql` | S | S-book, S-page |
| S-chapter | schema | `schema/schemas/chapter.graphql`, `paragraph.graphql` | `schemas/chapter.graphql`, `schemas/paragraph.graphql` | S | S-book, S-toc |
| S-stage | schema | `jobs/common/state_finalize.go`, `state_toc.go`, `job/types.go` (WorkUnitInfo) | `schemas/{book_job,toc_link_task,pattern_result,entry_to_find,toc_gap,classify_chunk,polish_task,stage_gate}.graphql`: one doc type per fan-out item and gate | M | S-book..S-chapter |
| P-ocr-adapter | plugin | `jobs/process_book/job/ocr.go`, `jobs/common/ocr.go`, `jobs/common/headings.go`, `providers/chandra_parse.go` | `plugins/page_adapter/` + cfg callback `OcrPage` created -> `Page` (headings JSON, header/footer recovery) | M | S-page, gents/ocr pack |
| P-ingest | plugin | `ingest/job.go`, `ingest/stitch.go`, `jobs/common/pdf.go` | `plugins/book_ingest/` (hash source, page_count, stitch parts, create Book + `OcrJob`) | M | S-book, gents/ocr |
| P-gate | plugin | `jobs/process_book/job/state.go` (`MaybeStartBookOperations`, `CheckCompletion`), `job/job.go` (`NoWorkFailure`, `reconcileTocLinkRecovery`) | `plugins/stage_gate/` callbacks: consecutive-OCR(20/30), all-OCR, all-linked, all-polished, terminal status complete/degraded/failed | L | S-stage, P-ocr-adapter |
| P-grep-report | plugin | `agents/toc_finder/tools/grep_report.go` | `plugins/frontmatter_grep/` (model tool for toc_finder) | S | S-page |
| P-page-tools | plugin | `agents/toc_entry_finder/tools/{grep_text,get_heading_pages,get_page_ocr,ocr_evidence_text}.go`, `agents/chapter_finder/tools/{grep_text,get_heading_pages,get_page_ocr}.go`, `agents/gap_investigator/tools/{get_page_ocr,get_gap_context}.go` | `plugins/page_search/` (grep+clusters, heading ranking, compact OCR, gap context) and/or `datastore_tool_surfaces` query reads over Page | L | S-page, Open question 3 |
| P-link-evidence | plugin | `agents/toc_entry_finder/tools/{ocr_evidence,ocr_evidence_extract,ocr_evidence_text,write_result}.go`, `job/link_toc_execution.go:validateTocLinkEvidence`, `job/link_toc_structure.go` | `plugins/link_evidence/` (AnalyzePageEvidence + ValidateCandidatePage + back-matter derivation), called by the write tool and post hoc | L | P-ocr-adapter (labels/furniture), S-toc |
| P-toc-normalize | plugin | `jobs/common/toc.go` (`normalizeTocExtractEntries`, `SaveTocExtractResult`, `findTocEntryByIdentity`) | `plugins/toc_entries_writer/` callback: extract result -> replace-set TocEntry rows | S | S-toc |
| P-pattern | plugin | `job/finalize_helpers.go` (sanitizers, generateSequence, roman, estimatePageLocation, discoveredEntryAlreadyLinked), `job/finalize_pattern.go` (`generateEntriesToFind`, `loadCandidateHeadings`) | `plugins/finalize_pattern/` callback: pattern result -> `EntryToFind` docs | M | S-stage, B-pattern |
| P-gaps | plugin | `job/finalize_validate.go:findFinalizeGaps`, `job/finalize_helpers.go:applyGapFix`, `job/finalize_discover.go:saveDiscoveredEntry`, `job/finalize.go:resortEntriesByPage` | `plugins/finalize_gaps/` (emit `TocGap` docs; apply fixes; resort) | M | S-stage, S-toc |
| P-skeleton | plugin | `job/structure_build.go`, `job/structure_extract.go`, `jobs/common/structure_text.go` | `plugins/chapter_builder/` callback: linked entries -> Chapter rows with mechanical_text | M | S-chapter, P-gaps |
| P-classify-io | plugin | `jobs/common/structure_classify.go` (`BuildClassifyPrompt`, signals, snippet), `job/structure_classify.go:validateStructureClassifyCoverage`, `persistClassifyResults` | `plugins/classify_io/` (chunk docs of 64 with prompt text; coverage-validate and write results) | M | P-skeleton |
| P-polish-io | plugin | `jobs/common/structure_text.go:ApplyEdits`, `job/structure_polish.go` (reuse, skip non-audio, failure fallback), `jobs/common/structure_prompts.go:PolishMaxOutputTokens` | `plugins/polish_io/` | S | P-classify-io |
| P-structure-final | plugin | `job/structure_completion.go` | `plugins/structure_finalize/` (validate persisted chapters, stats, structure_complete) | S | P-polish-io, P-gate |
| P-metadata-io | plugin | `jobs/common/metadata_ops.go`, `prompts/metadata/workunit.go:PrepareBookText` | `plugins/metadata_io/` (prepare text; persist all parsed fields incl. language/lccn) | S | S-book |
| P-epub-import | plugin | `epubimport/import.go`, `epubimport/parser.go` | `plugins/epub_import/` (direct terminal import, writes Paragraph) | M | S-book..S-chapter |
| P-epub-export | plugin | `epub/*`, `server/endpoints/books_export_epub.go`, `books_export_storyteller.go` | `plugins/epub_export/` (EPUB bytes base64; browser download handled in gents UI) | L | S-chapter |
| P-research | plugin | `researchmcp/{client,tools,types}.go` | `plugins/research_read/` (snapshot, byte-identical digest, search, read, validate_quote) + query surfaces | M | S-chapter |
| P-repairs | plugin | `jobs/common/{repair_ocr,repair_ocr_text,repair_pdf_text,repair_toc_range,repair_toc_entry,resolve_toc_entry,insert_toc_entry,exclude_toc_entry,quarantine_ocr,reset,reset_ops,op_registry}.go` | `plugins/operator_repairs/` (validated operator mutations + reset cascade) | L | all schemas |
| B-metadata | behavior | `prompts/metadata/system.tmpl`, `schema.go` | `agent_behaviors/metadata/system_prompt.md` + write surface `write_book_metadata` | S | S-book. Mechanical prompt copy; fix Finding 7 |
| B-toc-finder | behavior | `agents/toc_finder/system.tmpl`, `tools/write_toc_result.go`, `toc_finder_factory.go` | `agent_behaviors/toc_finder/system_prompt.md`; tools: P-grep-report, Page query, image tool (Open question 1), write surface `write_toc_result` | M | P-grep-report, S-toc |
| B-toc-extract | behavior | `prompts/extract_toc/system.tmpl`, `schema.go` | `agent_behaviors/toc_extract/system_prompt.md` + write surface (entries JSON) | S | S-toc. Mechanical |
| B-toc-entry-finder | behavior | `agents/toc_entry_finder/system.tmpl`, `schema.go`, `toc_entry_finder_factory.go` | `agent_behaviors/toc_entry_finder/system_prompt.md`; tools: P-page-tools, P-link-evidence-gated write | L | P-page-tools, P-link-evidence |
| B-pattern | behavior | `agents/pattern_analyzer/system.tmpl`, `prompt.go:JSONSchema` | `agent_behaviors/pattern_analyzer/system_prompt.md` + write surface | S | S-stage. Mechanical |
| B-chapter-finder | behavior | `agents/chapter_finder/system.tmpl`, `tools/write_result.go`, `chapter_finder_factory.go` | `agent_behaviors/chapter_finder/system_prompt.md`; tools: P-page-tools, write surface (consider P-link-evidence gate) | M | P-page-tools, P-pattern |
| B-gap | behavior | `agents/gap_investigator/system.tmpl`, `tools/write_fix.go`, `gap_investigator_factory.go` | `agent_behaviors/gap_investigator/system_prompt.md`; tools: gap context + Page query + write surface `write_gap_fix` | M | P-page-tools, P-gaps |
| B-classify | behavior | `jobs/common/structure_prompts.go:ClassifySystemPrompt`, `ClassifyJSONSchema` | `agent_behaviors/structure_classify/system_prompt.md` (Go const -> file) + write surface | S | P-classify-io. Mechanical |
| B-polish | behavior | `jobs/common/structure_prompts.go:PolishSystemPrompt`, `PolishJSONSchema` | `agent_behaviors/structure_polish/system_prompt.md` + write surface `write_polish_edits` | S | P-polish-io. Mechanical |
| T-metadata | task | `prompts/metadata/user.tmpl` | `tasks/metadata_task/prompt.md` (`{{ doc.book_text }}`) | S | B-metadata. Mechanical |
| T-toc-finder | task | `agents/toc_finder/user.tmpl`, `toc_finder/prompt.go:BuildUserPrompt` | `tasks/toc_finder_task/prompt.md` (wire PreviousAttempt for real) | S | B-toc-finder |
| T-toc-extract | task | `prompts/extract_toc/user.tmpl` | `tasks/toc_extract_task/prompt.md` (needs ToC page texts + structure_summary on trigger doc) | S | B-toc-extract, P-gate |
| T-toc-entry | task | `agents/toc_entry_finder/prompt.go:BuildUserPrompt`, `user.tmpl` | `tasks/toc_entry_task/prompt.md` (Go builder -> template; scan window, retry hint, back-matter context precomputed by plugin) | M | B-toc-entry-finder, P-link-evidence |
| T-pattern | task | `agents/pattern_analyzer/user.tmpl` | `tasks/pattern_task/prompt.md` | S | B-pattern, P-pattern |
| T-chapter-finder | task | `agents/chapter_finder/prompt.go:BuildUserPrompt` | `tasks/chapter_finder_task/prompt.md` | S | B-chapter-finder |
| T-gap | task | `agents/gap_investigator/prompt.go:BuildUserPrompt` | `tasks/gap_task/prompt.md` | S | B-gap |
| T-classify | task | `jobs/common/structure_classify.go:BuildClassifyPrompt` | `tasks/classify_task/prompt.md` (renders plugin-built prompt field) | S | B-classify, P-classify-io |
| T-polish | task | `jobs/common/structure_prompts.go:BuildPolishPrompt` | `tasks/polish_task/prompt.md` | S | B-polish, P-polish-io |
| TR-ingest | trigger | `jobs/process_book/process_book.go:NewJob`, `job/job.go:Start` | cfg event_source `BookJob` created -> P-ingest callback -> `OcrJob` | S | P-ingest |
| TR-ocr-page | trigger | `job/ocr.go:HandleOcrComplete` | cfg event_source `OcrPage` created -> P-ocr-adapter -> P-gate | S | P-ocr-adapter, P-gate |
| TR-front-matter | trigger | `job/state.go` (metadata at 20, toc_finder at 30) | cfg event_source `StageGate{gate:"front_matter_20"/"front_matter_30"}` -> T-metadata / T-toc-finder | M | P-gate, T-metadata, T-toc-finder |
| TR-toc-extract | trigger | `job/state.go`, `job/toc_finder.go:HandleTocFinderComplete` | cfg `ToC` toc_found -> T-toc-extract; not-found -> retry up to 3 then fail Book | M | B-toc-finder, Open question 5/6 |
| TR-link | trigger | `job/link_toc.go`, `job/link_toc_execution.go`, `job/link_toc_agents.go` | cfg `TocLinkTask` created -> T-toc-entry (parallel, cap 8 per book, 3 retries with hint) | M | P-gate (all-OCR), T-toc-entry, Open question 6/7 |
| TR-finalize | trigger | `job/finalize.go`, `finalize_discover.go`, `finalize_validate.go` | cfg: all-linked gate -> T-pattern -> P-pattern -> `EntryToFind` -> T-chapter-finder; `TocGap` -> T-gap; drained -> resort -> finalize gate | L | P-pattern, P-gaps, B-chapter-finder, B-gap |
| TR-structure | trigger | `job/structure.go`, `structure_classify.go`, `structure_polish.go`, `structure_completion.go` | cfg: finalize-done -> P-skeleton -> `ClassifyChunk` -> T-classify -> `PolishTask` -> T-polish -> P-structure-final | M | P-skeleton..P-structure-final |
| TR-epub | trigger | `server/endpoints/books_import_epub*` (import), `books_export_epub.go` | cfg: `EpubImportJob` -> P-epub-import; `EpubExportJob` -> P-epub-export | S | P-epub-import, P-epub-export |
| G-pipeline | graph | `jobs/process_book/job/*` (whole DAG) | optional `graphs/structured_book.plan.json`; deferred, since gents installs only built-in graphs today (Finding 10) | L | all TR-* |
| X-manifest | task | `jobs/process_book/process_book.go` (variants), `providers/registry_config.go` | `manifest.json` (assets, plugins, schemas, inference_slots `book_worker`/`book_vision` bound to the two vLLM endpoints) + `pack_config.json` skeleton + `tests/install.json` | M | everything above |
| X-scenario | task | `jobs/process_book/job/*_test.go`, `jobs/common/*_test.go` (fixtures and golden behaviour) | `tests/` + `scenarios/` end-to-end on one small scan; port validator unit tests with each plugin | L | X-manifest |
| X-drop | task | `schemas/{agentstate,agentrun,llmcall,metric,job,config,prompt,bookpromptoverride}.graphql`, `agent/observability/*` | not ported; replaced by gents sessions/telemetry (confirm Open question 10) | S | none |
