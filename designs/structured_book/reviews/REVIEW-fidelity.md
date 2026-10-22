# Fidelity review of DESIGN.md (structured_book)

Lens: does DESIGN.md keep every Shelf stage, prompt, output contract and
validation rule from `shelf-inventory.md`? This review lists everything the
design drops, merges or changes without saying so. Each item names the Shelf
source that was checked and gives a fix.

`S = shelf/internal`. "Recorded" means the design already lists the change in
C1-C6, §2.5 "Not portable" or §5 "Not ported". Recorded items are not repeated
here.

---

## Blocking

### F1. Transient failures are never retried, and a Task that dies leaves the book stuck or wrongly unblocked

- **Shelf:** `S/jobs/process_book/job/job.go:308-420`. When a work unit
  fails (`!result.Success`), Shelf retries it up to `MaxBookOpRetries=3` for
  metadata, toc_finder, toc_extract and link (and gives link a fresh agent).
  It does the same for pattern (`finalize_pattern.go:115`), discover
  (`finalize_discover.go:274`) and gap (`finalize_validate.go:343`), all at
  `MaxFinalizeRetries=3`. Classify and polish provider failures go to the
  structure handler's own budget (`deferFailedToHandler`).
- **Design:** the only retries are post-hoc *validation* rejects
  (`result:<stage>` → `StageTask{attempt+1}`), §0.6 and §2.8. A Task that
  errors, times out or ends without writing creates no `*Result`, so:
  - **Single-item stages hang forever.** metadata, toc_finder, toc_extract
    and pattern have no `ItemOutcome`/batch group, so no timeout covers them.
    The book stays `processing` and is never marked `failed`. In Shelf the
    zero-work invariant marks it failed.
  - **Gates open early with members missing.** If toc_finder or toc_extract
    dies, `Signal{link, member:"toc"}` never arrives. After 21600 s the `link`
    group closes with `min_count: 1` and only the `ocr` member, `gate-open`
    emits `StageStart{link}`, and the book continues as if it had no ToC.
    Shelf would fail. `finish` has the same problem: if metadata dies, the
    finish gate times out with only `structure` and can report `complete`.
  - **Link fails the book on one transient error.** A dead link Task becomes
    a timed-out item, then `failed`, then `link_closed` fails the book closed.
    Shelf would first retry 3 times.
- **Fix:**
  1. `gate_open` and `stage_close` must check that the group holds every
     member they expect (by `member` name for gates, against `tasks/<stage>/`
     for stages). If any is missing, write `BookStatus{failed, reason:"<stage>
     <member> never reported"}`. They must never move forward on a partial
     group, except for the `front` gate, where a partial group is harmless.
  2. Send every model stage through the item → batch → stage machinery, with
     batch size 1 for the single-item stages, so the group timeout covers
     them.
  3. Bring back retries for Task failures. In X-00, spike an event source on
     the runtime `FireOutcome` collection, filtered on
     `trigger_id _like "sb-task-%"` and a non-success `terminal_state`. It is
     unbound and goes to a `sb-task-failed` callback. Correlate it by setting
     `session_id_template: "sb:{{ doc.run_id }}:{{ doc.stage }}:{{ doc.task_key }}"`
     on each `sb-task-*` trigger, so `FireOutcome.session_id` carries the key.
     The design's "FireOutcome has no correlation" (fact row 9) does not rule
     this out. The callback emits a `Signal{gate:"task_failed"}`, and a bound
     stage creates `StageTask{attempt+1}` under the same budget, or the
     terminal outcome when the budget is used up. If the spike says no, at
     least shorten the group timeouts for single-item stages (for example
     1800 s) and treat a timeout as a used-up budget. Write the fallback down
     as a change.

### F2. A missing ToC no longer fails the book, by default

- **Shelf:** `toc_finder.go:241-251` fails the job after 3 `toc_found=false`
  attempts ("this book may not have a ToC"). A toc_extract that runs out of
  retries leaves `GetTocFound && !TocExtractIsComplete`, so `CheckCompletion`
  never finishes and the book ends `failed` (`state.go:208-212`). With zero
  linked entries, the structure build returns `"no linked ToC entries found"`
  (`structure_build.go:32-33`).
- **Design:** `BookJob.toc_policy` defaults to `fallback`. Toc not found, or
  toc_extract out of retries, becomes `Toc{none}` + `Signal{link}`, then E=0,
  then pattern. If pattern finds nothing, `gap_closed` runs and then
  `structure` runs with zero entries. Nothing says what happens next. The
  changes table does not list this, and the e2e test even accepts
  `source: none`.
- **Fix:** make `require` the default, so the default matches Shelf. Keep
  `fallback` as an opt-in and list it in "Changes from Shelf". For
  `fallback`, define the zero-entry case: if there are no linked or discovered
  entries after `gap_closed`, either fail with Shelf's
  `"no linked ToC entries found"`, or (opt-in only) build one `ch_001`
  chapter covering pages 1..N with `source:"fallback"`. Add an SB-S7/SB-S8
  test case for each choice.

### F3. OCR failures are quarantined automatically, so the book ends degraded instead of failed

- **Shelf:** `job.go:370-389`: "No exhausted OCR failure is a successful
  blank page … deterministic content failures fail visibly and remain
  incomplete". The book ends `failed`. Only an operator can quarantine a page
  (`common/quarantine_ocr.go`), and only quarantined pages give `degraded`
  (`state.go:263-272`). The comment at `state.go:186` says plainly that this
  is to avoid "laundering degraded output into complete".
- **Design:** `page_assemble` and `ocr_done` quarantine failed or missing
  chunks themselves, and the book ends `degraded` (§2.8 diagram, SB-S2
  acceptance "failed chunk quarantines its range"). One bad page in an OCR
  chunk of at least 20 pages silently drops the whole chunk. Nothing records
  this change.
- **Fix:** by default, a failed or missing chunk gives
  `BookStatus{failed, reason:"ocr chunk <c> (pages a-b) failed: <error>"}`.
  Add `BookJob.ocr_failure_policy: fail | quarantine` (default `fail`). Make
  quarantine per page where `OcrPage` rows exist, and per chunk only when
  there are no rows. List the policy under "Changes from Shelf". Change the
  SB-S2 acceptance case to "failed chunk → failed book (default)" and
  "→ quarantine (policy)".

### F4. Metadata that runs out of retries degrades the book; Shelf fails it

- **Shelf:** `markBookOpRetryExhausted(OpMetadata)` leaves metadata failed.
  `CheckCompletion` returns early on `!MetadataIsComplete()`
  (`state.go:196-199`), so the job is failed.
- **Design:** the §2.8 budget table says `metadata | 3 | Degraded`, and
  SB-S4 says "attempt 3 → degraded Signal". The finish line also says
  `degraded(… polish/metadata fallback)`. That contradicts the polish row
  (fail, as Shelf) and SB-S8.
- **Fix:** metadata out of retries gives `BookStatus{failed}`. Remove
  "polish/metadata fallback" from the finish line. `degraded` means
  quarantined pages only (and only under the F3 opt-in policy).

### F5. Strict-schema validation was replaced with required-field and coverage checks only

- **Shelf:** every one-shot call and every write tool's arguments go through
  full JSON-Schema validation (`S/providers/structured_output.go:validateStructuredJSON`,
  with 2 repair turns). That enforces enums, ranges, `maxItems`,
  `maxLength`, nested `required` and `additionalProperties:false`. The
  stage handlers rely on it. Some examples:
  - toc_extract `level` 1..3;
  - pattern `pattern_type` enum, `level` 1..6;
  - classify `classifications` enum of 3 and `content_types` enum of 20;
  - polish `edits` ≤50, `old_text`/`new_text` ≤500, `reason` ≤200;
  - toc_finder `confidence` 0..1, `structure_summary.total_levels` 1..3,
    `level_patterns` keys "1"/"2"/"3".
- **Design:** C1 folds nested objects into `*_json` strings. Surfaces enforce
  only `required`, and `output_schema_ref` is not enforced (fact row 7). The
  work-list checks only metadata required fields (SB-S4), classify coverage
  (SB-S8) and normalization (SB-S5). A `*_json` string that is not valid
  JSON, or that has an out-of-enum `content_type`, a level-4 ToC entry or 80
  polish edits, is accepted or fails somewhere later.
- **Fix:**
  1. Add `core/schemas/` with Shelf's 9 schemas copied verbatim (the Go
     `mustMarshal` maps, dumped as JSON by the SB-C6..C8 fixture generator).
  2. Each `result:<stage>` first rebuilds the logical object from the flat
     fields plus the parsed `*_json` strings. It then validates the object
     against that schema with the `jsonschema` crate, built for wasm32-wasip1
     with default features off (check this in SB-01).
  3. On a failure, the retry `feedback` is Shelf's
     `structuredRepairPrompt(schema, lastOutput, issue)` text, ported. Count
     these failures in the stage budget.
  4. Add one reject case per constraint to the SB-S4..S8 acceptance.

### F6. The `load_page` "observations" contract cannot be kept by a stateless, read-only plugin

- **Shelf:** `S/agents/toc_finder/tools/load_page_image.go:36-75` keeps
  `currentPageNum` and `pageObservations` across calls. It rejects a page
  load when a page is already loaded and no observations are given.
  `load_ocr_text` reads "the currently loaded page".
  `write_toc_result.go:88,104-108` builds `PagesChecked` and `StructureNotes`
  from that state, and the toc_finder retry prompt shows them as "Previous
  Observations" (`user.tmpl:15-17`).
- **Design:** `book_search` is sealed and bound **read-only**, and every call
  is a new process. It cannot know whether a page is loaded, so it cannot
  reject the call or collect observations. `TocFinderResult.structure_notes`
  has no producer, and the "PreviousAttempt now really wired" promise (§2.5)
  has no data to wire. C3 nevertheless says "the observation-before-next-page
  rule is kept".
- **Fix:** make the contract explicit in the tool arguments, and record it
  as change C7:
  - `load_page` takes `{page_num, previous_page?, current_page_observations?}`.
    It rejects the call with Shelf's message when `previous_page` is set and
    `current_page_observations` is empty. It returns the page text, as
    `load_ocr_text` did.
  - `write_toc_result` gets two extra fields, `pages_checked: Int` and
    `structure_notes_json` (`{page: note}`), which the model fills from its
    own calls. The prompt says so.
  - `result:toc_finder` stores both fields and passes them into the next
    attempt's `PreviousAttempt`.
  - Alternative: bind `book_search` read_write, write
    `tasks/toc_finder/<key>.observations.jsonl`, and have `result:toc_finder`
    read that file. This keeps Shelf's tool surface, but it needs R5 to allow
    a read_write bind from a model tool.

### F7. `validate_quote` with `digest` changes Shelf's version semantics

- **Shelf:** `researchmcp/tools.go:validateQuote` always loads the
  **current** snapshot. If `expected_structure_digest` (or
  `expected_source_sha256`) is set and differs, it returns
  `exact_match=false, version_match=false`, "source version mismatch; refresh
  the assignment before citing", and does no search.
- **Design:** decision 10 keeps `canonical/<digest-hex>.json` immutable "so
  citations pinned to an old digest still validate", and `book_research`
  takes `digest` to select one. A caller that passes `digest=old` and
  `expected_structure_digest=old` gets `version_match:true` against a stale
  text, which Shelf forbids on purpose.
- **Fix:** `validate_quote`, `search_passages` and `read_passage` always use
  `canonical/book.json` (the latest), as Shelf does. Either remove `digest`,
  or allow it only on a new, clearly named `read_passage_at_version` op that
  never returns `version_match`. Correct the decision 10 wording. Add an SB-P2
  case: old digest passed → `version_match:false`, no search.

### F8. Variant gating: metadata runs in ocr-only, `photo-book` is gone, and `finish` can fire twice

- **Shelf:** `process_book.go:82-131`. `photo-book` and `text-only` both run
  OCR + metadata. `ocr-only` runs no LLM at all (`EnableMetadata=false`).
- **Design:**
  - `BookJob.variant` drops `photo-book` without a note.
  - The diagram starts `WorkItem{metadata_prepare}` on "chunk with page 1"
    with no variant check. Under `ocr-only` (finish expected 1), metadata then
    emits a second `Signal{finish}`. The first one opens the gate, and the
    second one starts a new group, which closes on timeout and writes a
    second `BookStatus`.
- **Fix:**
  - `page_assemble` emits `metadata_prepare` only when the variant ≠
    `ocr-only`.
  - Accept `photo-book` as an alias of `text-only` (same flags in Shelf), and
    say so.
  - `gate_open` for `finish` refuses to fire twice: it writes
    `signals/finish.opened` with O_EXCL, and on `EEXIST` it emits nothing.
  - Add the variant × finish-count cases to the SB-S3 acceptance.

---

## Non-blocking (fix during the build; each one is an unrecorded change)

1. **The retry budget table does not match Shelf.**
   - `gap` is 1 in the design and 3 in Shelf (`finalize_validate.go:343`).
   - `pattern` has no row. Shelf retries it 3 times on failure, then skips to
     discover with 0 entries, so validate/gap are skipped too
     (`finalize_pattern.go:115-142`).
   - `discover` retries "on invalid output", but Shelf drops an out-of-range
     page straight to not-found without retrying (inventory 3.6). Shelf's
     retries are for agent or provider failure.
   - **Fix:** gap 3, add `pattern | 3 | skip to discover (0 entries)`, and
     make discover's invalid output count as not-found. Count Task failures
     (F1) in all of them.

2. **The task template text is an unlisted prompt change.**
   - `tasks/*/prompt.md` adds a `---` footer ("Book folder…", "Task
     file…", "Finish by calling … exactly once"). None of C1-C6 covers it.
     `{{ "<write tool>" }}` is a minijinja string literal and renders as
     `<write tool>`.
   - For link, `appendLinkTocRetryHint` already puts `PREVIOUS REJECTION
     FEEDBACK` inside the user prompt (SB-C7). The template's
     `{% if doc.feedback %}` block would then print it a second time.
   - **Fix:** list the footer as C8. Hard-code the tool name in each of the 9
     prompt files. Define, per stage, whether retry text goes into
     `user_prompt` (link and toc_finder, as Shelf) or into `feedback`
     (validators that are new in the port), and never both. Add a golden test
     for the rendered link retry prompt.

3. **The link reject loop moved from inside the session to across sessions, with a smaller budget.**
   - In Shelf, `write_result` rejects inside the tool, and the agent keeps
     working for up to 25 iterations (`toc_entry_finder_factory.go:13`).
     Only after that does it get up to 3 fresh agents.
   - In the design, every reject uses up one of 3 whole attempts.
   - C5 covers the prompt wording, not the change in budget.
   - **Fix:** record the change. Make `get_page_ocr`'s `write_result_ready`
     the in-session pre-check, which it already is in Shelf. Consider adding
     a `book_search` op `check_candidate {page_num}` that returns exactly
     the `ValidateCandidatePage` reject reason, so the agent can recover
     inside the session as it did in Shelf.

4. **Agent loop limits are gone, with no replacement.** `MaxIterations` (25,
   25, 15, 20), `RequireToolUse`, `MaxToolCallsPerTurn 4` and the "You must
   call one of the available tools now" nudge are listed as not portable, but
   nothing replaces them. Without an iteration cap, an agent can loop until
   the group timeout. **Fix:** set `Task.goal_token_budget` (the task-level
   key the guide lists), or the nearest per-run turn or token cap the spike
   finds. Set it per behavior from Shelf's iteration count × a measured
   per-turn token cost, and record it.

5. **Context handling for the agents changed.** Shelf strips the images in
   history and compacts old tool results on every request
   (`compactOldToolResultsForRequest`; toc_finder: "observations recorded
   and removed from context"). The design keeps every `load_page` and
   `get_page_ocr` text result until the 0.85 compaction. On GLM, with 30+
   page loads, that is a different prompt. **Fix:** record it, and check the
   context length in the e2e run. If it is too long, have `load_page` return
   only the page that was asked for, and tell the agent in the prompt not to
   reload pages.

6. **Shelf's sampling settings are only half carried over.** The README note
   covers `max_output_tokens`, but not temperature 0.1 for metadata and
   toc_extract. **Fix:** the README asks for a `book_reader` profile with
   temperature 0.1, and notes that pattern, classify and polish then run at
   0.1 too, whereas Shelf left those at the provider default.

7. **The resort is not persisted.** Shelf's `resortEntriesByPage` writes
   `sort_order=(i+1)*100` back to each `TocEntry` (`finalize.go:285-293`).
   The design writes only `toc/final.json`, so the `TocEntry` docs keep the
   `i` or `page*1000` values. **Fix:** `gap_closed` emits a new `TocEntry`
   revision for every entry whose sort_order changed.

8. **Paragraphs and polish do not line up.** The design adds a paragraph
   split. That is new, and the design says so. But edits are applied to the
   chapter's text, while the page map is built on `mechanical_text`, so
   `Paragraph.start_page` and `raw_text`/`polished_text` can drift once
   edits change lengths. Non-audio chapters also need
   `polished_text = mechanical_text` and `edits_applied_json="[]"` written
   explicitly (Shelf, inventory 2.8). **Fix:** split `mechanical_text` into
   paragraphs first. Apply each edit to the first paragraph that contains its
   `old_text`, in order, which is the same first-occurrence rule. Then join
   for the chapter's `polished_text`. Add a unit test showing the chapter text
   matches Shelf's `ApplyEdits` output byte for byte.

9. **The "validate persisted chapters" step no longer reads persisted data.**
   - Shelf re-queries `Chapter` and requires `extract_complete`,
     `polish_complete` and non-empty classification fields. That check exists
     because Shelf's writes are async (ADR 010). The design checks the
     workspace instead.
   - The design's `Chapter` also drops `extract_complete`, `polish_complete`
     and `polish_retries`. Shelf's research chapter view reports
     `polish_complete` (`researchmcp/types.go:42`).
   - **Fix:** add those three fields. In X-00, confirm that a callback's
     output documents are written all-or-nothing. If they are, state that as
     the replacement. If not, `commit_closed` needs a read-back path.

10. **Ids in the digest stream changed.** Shelf's research `book.id` and
    `chapter.id` are DefraDB `_docID`s (`server/endpoints/books_chapters.go:193-197`),
    not `entry_id`. The design puts `bk_<sha>` and `ch_%03d` into the same
    digest slots. The algorithm is the same, but the identifiers are not, so
    no Shelf digest can be reproduced. That is fine, since migration is out
    of scope, but write it down. Note also that `ch_%03d` is positional: after
    a forced re-run with a different ToC, `ch_005` can name a different
    chapter. The digest catches that, but it should be documented.

11. **Stale runs are not cleaned up.** Shelf deletes stale chapters on a
    rebuild. In the design, every forced run adds a new set of
    `Chapter`/`Paragraph`/`TocEntry` rows with the same `book_id`. **Fix:**
    document that consumers filter by the `run_id` of the latest
    `BookStructure{status:"complete"}`, and have `book_research` read only
    the canonical file. Add a `defs.json` note.

12. **Multi-PDF ingest and stitching are dropped.** Shelf takes several
    PDFs (`ingest/stitch.go:GroupParts/StitchPDF`, and `common/pdf.go` with
    cumulative page ranges and `-N.pdf` ordering). `BookJob.source_path` is a
    single path, and SB-S1 has no stitch case. **Fix:** add
    `source_paths: [String]`, and either refuse more than one path with a
    message (recorded), or create one `OcrJob` per part and offset the page
    numbers in `page_assemble`.

13. **`reuseUnchangedPolish` is dropped** (`structure_polish.go:17-60`). A
    forced re-run polishes every chapter again. **Fix:** record it, or reuse
    the polish from the previous canonical file when `structureChapterReuseKey`
    matches.

14. **Research read bounds and the readiness gate are incomplete.** The SB-P2
    acceptance lists the top_k, snippet and read limits, but not
    `list_structure` limit 50/200 or `read_passage` `context_before` 300/2000.
    It also leaves out `requireResearchReady` (`structure_complete &&
    !structure_failed && ≥1 passage`, `client.go:125-133`) and the
    `get_book` field set (`author`, `status_reason`, `source_filename`, …).
    **Fix:** add all of these to the SB-P2 acceptance.

15. **Small output-contract changes that are not listed:**
    - `BookMetadata` drops Shelf's singular `author = authors[0]`
      (`metadata_ops.go:106-109`), which `get_book` and the EPUB export read.
      Add it.
    - SB-B4 makes `scan_page` required on `write_result`. Shelf has it
      optional (`Result{scan_page?}`) and rejects it later in the validator.
      Keep it optional so the agent can report "not found" with reasoning,
      and let `result:link` reject that as Shelf does.
    - toc_finder gets a new `start<=end<=N` range check, and its retry now
      carries `PreviousAttempt` (Shelf passes `nil`). Both are fine, but add
      them to "Changes from Shelf". The golden fixture has to come from
      rendering `user.tmpl` directly with data, because Shelf's live path
      never fills it.
    - Metadata input is limited to chunk 1. Shelf takes the first 20
      non-empty pages from any OCR'd page (`metadata_ops.go:70-90`). Make
      `metadata_prepare` wait for page 20's chunk, or read later page files
      when chunk 1 has fewer than 20 non-empty pages.
    - The progress counters are dropped:
      `Book.toc_link_entries_{total,done}`, `finalize_*` and
      `structure_chapters_*`. Record this, or emit them on `BookStatus`.
    - EPUB import no longer writes one page row per chapter. Record this.

---

## Kept intact (checked)

These match Shelf:
- the stage order;
- the link gate (extract plus all OCR);
- the 20/30-page front gates (as chunk counts);
- the toc_extract normalization and replace-set (as revisions);
- link fail-closed;
- the pattern sanitizers and `generateEntriesToFind`;
- validate/gap skipped when EntriesToFind=0;
- `MinGapSize=15`;
- the discover dedupe and `page*1000` sort order;
- the applyGapFix kinds;
- the chapter skeleton, keys and parent rule;
- classify chunks of 64 with exact coverage;
- polish only when audio_include=true, with fail-closed fallback;
- `validate_quote` matching rules;
- the digest algorithm.

The system prompts are static text (no `{{` in any `system.tmpl`), and the
two Go-const prompts are plain raw strings (`structure_prompts.go:72,108`).
Copying them verbatim with only C1-C6 applied is therefore sound.

---

## Round 1 re-review

Reviewed: `DESIGN.md` revision 1, against Shelf (`S = shelf/internal`) and
gents `origin/main` as fetched today (`d4df8a02b`). The design's pin
`15c5452b8` is an ancestor of that commit, and the tag `a5d02f106` was checked
as well. Lens: every Shelf stage, prompt, output contract, failure rule and
validation rule is kept, or the change is recorded.

### Status of F1-F8

| Id | Status | Notes |
| --- | --- | --- |
| F1 | **Fixed in design, broken at runtime** | Gate completeness now lives in the bound `stage:<gate>` handlers. Single-item stages run as batches of 1 with openers. The finish watchdog covers hangs. The Task-failure retry (TaskFailed marker joined with StageTask) is sound as a mechanism. But the Tasks it depends on are never admitted as written: see R1-B1. |
| F2 | Fixed | `require` is the default. `fallback`/`whole_book` are opt-in (D1). The zero-entry case is defined in `gap_closed`. This matches Shelf: an E=0 extract goes on to finalize, as `state.go:143-149` does. |
| F3 | Fixed | `ocr_failure_policy: fail` is the default (D2), with per-page quarantine as an opt-in. One leftover is non-blocking (N1-7). |
| F4 | Fixed | Metadata out of retries fails the book. `degraded` now means only quarantined pages. |
| F5 | Fixed in substance | `core/schemas` + `core/validate.rs` (SB-C9) + `structuredRepairPrompt`. The retry budget still differs from Shelf (N1-3), and two rebuild details are missing (N1-4). |
| F6 | Fixed | C7 adds the explicit `previous_page`/`current_page_observations` contract and model-reported `pages_checked`/`structure_notes_json`. |
| F7 | Fixed | The latest canonical is always used. `digest` is accepted only on `read_passage_at_version`, which never reports `version_match`. |
| F8 | **Fixed in design, new hole** | Metadata is gated on variant, `photo-book` is an alias (D16), and a finish can fire only once. But the `status.json` marker that guards the single finish can leave a book with no terminal status at all: see R1-B2. |

### Blocking

#### R1-B1. Every `emit_outcome` Task is refused: `StageTask` has no `handoff_id`

- **gents (main and tag):** an `emit_outcome` Task must receive a document that carries a `handoff_id` string.
  - `config_client/desired_state.rs::validate_outcome_source_fields` refuses to publish a Trigger that delivers a collection with no `handoff_id: String` field to an `emit_outcome` Task: "…so every fire would be refused". `CallbackResult` and `WorkspaceReceipt` are the only exceptions.
  - Fire admission also refuses each document that has no value: `trigger_engine/mod.rs:640`, `ensure!(… "emit_outcome requires a source handoff_id")`.
  - `FireOutcome.source_handoff_id` is copied from that field (`durable.rs:197-211`).
  - The same check is present at `a5d02f106`.
- **Design:** decision 6 and §2.5 set `emit_outcome: true` on all 9 Tasks. The `StageTask` SDL (§2.4) has no `handoff_id`. F-7 lists the FireOutcome fields but leaves out `source_handoff_id`.
- **Effect:** one of two things happens:
  - `pack install` fails at config publication.
  - Or, if the collection is not introspected yet, every fire is rejected at "prepare fire", so no model stage ever runs.

  Either way, every book ends as a watchdog failure after 72 h, or the pack never installs. This is not a spike question: the code answers it.
- **Fix:**
  1. Add `handoff_id: String @index` to `StageTask`. Set `handoff_id = task_ref` on every StageTask (unique per attempt). Set it to `"-"` on TaskFailed rows, which no trigger delivers.
  2. `task_failed` correlates on `FireOutcome.source_handoff_id`, which is the `task_ref` exactly, instead of parsing `session_id`. List `source_handoff_id` in `sb-task-failed`'s `input_fields`. `FireOutcome.attempt` also mirrors `StageTask.attempt`, because admission copies the doc's `attempt`.
  3. Add a `defs.json` assertion: every collection delivered to an `emit_outcome` Task declares `handoff_id: String`.
  4. Correct F-7.
  5. R17 then needs only the runtime half: a callback binding on `FireOutcome`, and which `terminal_state` values occur.

#### R1-B2. The `status.json` marker can leave a failed or finished book with no terminal `BookStatus`, forever

- **Shelf:** `state.go:261-264` persists the terminal status **synchronously**, because "a dropped async write here would leave the book stuck in 'processing'". Every run reaches exactly one terminal status. That is the guarantee F1 was about.
- **Design:** decision 4.4 and §2.8 say the marker is "created `O_EXCL` by whoever writes the first terminal `BookStatus`". A handler writes the terminal status "only if the run has no terminal marker yet", and `stage:finish` does nothing when the marker exists.
  - The marker is a file. The plugin creates it **during** the invocation, before the host commits that invocation's outputs.
  - The commit can still fail after the file exists. The design itself relies on unique-ref conflicts failing a whole transaction (F-19, "duplicate producer fails its own transaction"). It can also hit a DB error or an output-size refusal.
  - Then `max_attempts: 3` re-runs the handler. The handler finds the marker, so by the rule it writes nothing. The watchdog later finds the marker and writes nothing too.
  - The result: no `BookStatus{failed|complete}` ever, and the book stays `processing`. The e2e test polls for 6 h and sees nothing.
- **Fix:** make the database the authority, and keep the file only as a hint.
  1. Add `terminal_ref: String @index(unique: true)` to `BookStatus`, set to `"<run_id>/terminal"` on every terminal row. DefraDB then enforces one terminal status per run. A second terminal write fails its own transaction harmlessly.
  2. `status.json` stores `{status, reason, stage, writer_ref}`. A handler whose deterministic `writer_ref` (its `cause_ref`/source ref) matches the marker **re-emits** the same terminal `BookStatus` on retry instead of skipping.
  3. `stage:finish` always emits a terminal row. If the marker exists, it emits the marker's content. Otherwise it emits complete, degraded or missing-member failed. If a terminal row already exists, the unique ref drops the duplicate. This turns the watchdog into the repair path.
  4. Add SB-S3 cases: "marker written, commit failed → retry re-emits", and "marker present, no row → watchdog emits marker's status".
  5. `degraded`/`complete` from `stage:finish` keep the same unique ref, so a late failure and a completion can never both land.

### Non-blocking (each one is an unrecorded change or a gap in the spec)

1. **The finish `expected` must be identical in all four producers.** F-18 says members must agree on `expected`, and a mismatch quiesces the group for good. Here the finish group is also the watchdog, so a mismatch means no terminal status either.
   - The diagram gives `expected` only for the opener (`members(variant)+1`), not for the `metadata`, `ocr` and `structure` Signals.
   - **Fix:** store `finish_expected` in `RunFile`. Add one `finish_signal(member)` builder in `contract.rs` that every producer calls. Add an SB-S3 case per variant.
2. **The front gate allows a partial group; Shelf requires the exact prefix.**
   - `state.go:57-61`: "Require that exact prefix rather than any N pages". `ConsecutivePagesComplete(20)` gates metadata and `(30)` gates toc_finder (`state_book_pages.go:127-137`). Decision 4.3 and §2.8 say "front: continue with fewer pages".
   - That contradicts D8 ("at least pages 1-30"), and it can run metadata/toc_finder on a hole in the front matter.
   - **Fix:** under `ocr_failure_policy: fail`, `stage:front` with a missing member does nothing; the `ocr` gate will fail the book. Under `quarantine`, it continues only when the missing pages are marked quarantined. Record this in D8.
3. **The schema-repair budget is smaller than Shelf's.**
   - Shelf allows up to 2 repair turns per call (`structured_output.go`) inside each of 3 attempts, so a metadata or classify call has up to 9 generations before the book fails.
   - The design spends one of 3 whole attempts per schema failure. Metadata and classify failures fail the book, so on GLM this changes outcomes.
   - **Fix:** give schema repair its own budget of 2 per attempt. Carry it as `repair` in `task_ref`, i.e. `<…>/<attempt>.<repair>`. Or record the change as a D-row.
4. **The F5 rebuild needs two rules spelled out.**
   1. gents maps an empty nullable list to null (`defra_write/input.rs:104-106`, "The shared mutation renderer maps empty lists to null"). So `authors: []` comes back as `null`, and a schema requiring an array would reject a valid answer. Map null → `[]` for every list field before validating.
   2. Port-only fields must be stripped before validating against Shelf's `additionalProperties:false` schema. These are C7's `pages_checked`/`structure_notes_json`, and the CTX fields.

   Add both to SB-C9.
5. **The paragraph-first ApplyEdits does not equal Shelf's ApplyEdits.**
   - Shelf does `strings.Replace(text, old, new, 1)` in order over the whole chapter. The design applies each edit to "the first paragraph that contains its `old_text`". Its cross-paragraph rule covers only an `old_text` that no single paragraph contains.
   - If an occurrence that spans a paragraph break comes **before** the first in-paragraph occurrence, Shelf edits the spanning one and the design edits the later one. The chapter text then differs. Splitting on blank lines and re-joining also has to round-trip runs of 3 or more newlines exactly.
   - **Fix:** compute `Chapter.polished_text = ApplyEdits(mechanical_text)` on the full text, which is the Shelf contract. Derive the polished paragraphs by splitting that result. Pair them with the raw paragraphs only where the split counts match, and otherwise give `polished_text` per paragraph with `raw_text` from the mechanical split, without a 1:1 promise. Add the counter-example to the SB-S8b tests.
6. **The retry-text placement leaves link and toc_finder schema repair undefined.**
   - §2.5 says link and toc_finder put retry text in `user_prompt` and "leave `feedback` empty". Every other stage puts schema-repair text in `feedback`. So a schema reject on `write_result` or `write_toc_result` has no defined place.
   - Separately, `task_retry` for link re-issues "the same `user_prompt`". Shelf's `createLinkTocRetryUnit(ctx, info, linkTocWorkFailureReason(result))` (`job.go:349-352`) feeds the failure reason back on provider failures too.
   - **Fix:** route schema repair for these two stages through the same hint builder (`appendLinkTocRetryHint` for link; `PreviousAttempt.Reasoning` for toc_finder). Have `task_retry` for link append the `FireOutcome.reason` as the hint.
7. **`OcrDocument{complete:false}` is a failure mode that exists only in the port.**
   - When extract stops at `EXTRACT_MAX_BYTES`, the tail pages are missing, and under the default policy the book fails. Shelf has no such cap.
   - **Fix:** record it as a D-row, and have the failure reason name the cap ("ocr chunk c truncated at 1.5 MB, pages x-b missing") so an operator can tell it from a model failure.
8. **The 72 h watchdog is a new deadline.**
   - Shelf has no wall-clock limit. A slow but healthy book fails with "pipeline did not finish", and its late `structure` Signal can no longer complete it. A large book can run past 72 h: the callbacks are serial (F-17), there are 255 sequential batches in the worst case, and OCR alone takes about 16 h.
   - **Fix:** record it as a D-row. Derive the finish and stage timeouts from `page_count` and `chunks_total`, or make them a `BookJob` field with the 72 h default.
9. **Work keeps running after a failure.**
   - When OCR, metadata or link fails, the other branches still release batches. For example, structure classify and polish run after metadata has failed. Shelf's `FailBook` stops the job.
   - **Fix:** every `stage:*` and `*_next` release handler first checks the terminal marker (or `terminal_ref`, R1-B2) and releases nothing once the book is terminal. This is cost, not correctness.
10. **Research readiness after a failed forced re-run.**
    - Shelf's `requireResearchReady` refuses while `structure_failed`. In the design, `book_research` reads only `canonical/book.json`, which a failed re-run never replaces. So the previous good canonical keeps answering with `version_match: true`.
    - Serving the last good version is defensible, but it is a change. **Fix:** record it, or have a failed run write `books/<sha>/latest_run.json{status}` and have `requireResearchReady` check it.
11. **Retries are broader than Shelf's.**
    - Shelf retries metadata, toc_finder and toc_extract only when `jobs.IsRetriableError(result.Error)` holds (`job.go:312-345`); a non-retriable error exhausts at once. `task_retry` retries every non-success `terminal_state`.
    - Harmless, but record it in D14.
12. **The `sb-task-join` groups never close.** Every StageTask that succeeds leaves a 1-member group with no timeout, so thousands accumulate per book. Group recovery visits one grouped binding per tick. Add this to R17: measure group-state growth over the e2e run. If it is a problem, set `timeout_secs` large (for example 7 days) with `min_count: 2`, so that a group that never fills expires instead of firing.

### Checked and still intact

These match Shelf:
- the stage order;
- the link gate {toc, ocr};
- the variant flags (text-only = OCR + metadata; ocr-only has no LLM);
- per-stage budgets: metadata/toc_finder/toc_extract/link 3 (`MaxBookOpRetries`); pattern/discover/gap 3 (`MaxFinalizeRetries`); classify/polish 3 (`MaxStructureRetries`);
- pattern exhausted → discover with 0 entries → validate and gap skipped (`finalize_pattern.go:115-142`, `finalize.go:144-150`);
- discover and gap out of retries → counted complete, no fix (`finalize_discover.go:274-293`, `finalize_validate.go:343-358`);
- link fail-closed;
- classify out of retries → failed;
- polish → mechanical fallback with `polish_failed` → the structure fails;
- non-audio passthrough;
- the zero-entry → `"no linked ToC entries found"` path under `require`;
- the `validate_quote` version check runs before the search.

The revision log's FID-* mappings match the body of the design, except for the two holes above.

---

## Round 2 re-review

Reviewed: `DESIGN.md` revision 2, against Shelf (`S = shelf/internal`), gents
`origin/main` (fetched again today: still `d4df8a02b`, no new commits) and
packs `origin/master` `cd00bb8` (`gents/ocr`). Lens: as before. Every Shelf
stage, prompt, output contract, failure rule and validation rule must be kept,
or the change recorded. Only items that would make the build or the runtime
fail, or would silently break a Shelf guarantee, are blocking.

### Status of earlier blocking items

| Id | Status | Evidence checked |
| --- | --- | --- |
| F1 | **Fixed in substance** | Gates check completeness in the bound `stage:<gate>` handlers. Single-item stages run as batches of 1 with openers, so the 4 h batch timeout covers them. Task failures come back as `FireOutcome` → `TaskFailed` → `task_retry` join → bound `stage:task_retry`. The finish watchdog covers a run where nothing reports. The runtime half of R17 is still a spike, and the timeout fallback (D14, `sb-items-single`) is written down. Two edges are still open (N2-1, N2-2) |
| F2 | Fixed | `require` is the default. The zero-entry case is owned by `gap_closed`, and an E=0 extract goes on to pattern, as `state.go:143-149` does |
| F3 | Fixed | `fail` is the default and `quarantine` is opt-in, per page. D22's reason text is wrong for one cause of `complete:false` (N2-6) |
| F4 | Fixed | Metadata out of retries fails the book. `degraded` means quarantined pages only |
| F5 | Fixed | SB-C9 validates against the dumped schemas with 2 repairs per attempt, which keeps Shelf's 9 generations. One rebuild rule is still missing (N2-9) |
| F6 | Fixed | C7 is in the arguments, and the model reports `pages_checked`/`structure_notes_json` |
| F7 | Fixed | The latest canonical is always used, and `read_passage_at_version` never reports `version_match` |
| F8 | Fixed | Metadata runs only when the variant is not `ocr-only`. `photo-book` is an alias (D16). Unique `signal_ref` plus the terminal protocol: a late member makes a new 1-member group that times out, and `stage:finish` then re-emits the marker, which collides harmlessly |
| R1-B1 | **Fixed, checked in code** | `StageTask.handoff_id: String`. `validate_outcome_source_fields` (`desired_state.rs:565-626`) accepts it. Admission (`trigger_engine/mod.rs:626-643`) copies `handoff_id` and the integer `attempt`. `stage_outcome` (`durable.rs:163-215`) writes `source_handoff_id` and `attempt` on every request terminal: `completed`, `failed`, `interrupted`, `dead` or `superseded`, from `lifecycle/claim.rs`, `execution_lease.rs` and `request_admission.rs` |
| R1-B2 | **Fixed, checked in code** | A unique `status_ref`, plus terminal rows written alone. `txn.execute` turns any GraphQL error into `Err` (`config_client/txn.rs:925-929`), and `commit_success` runs every create in one transaction (`callback/plugin.rs:253-318`). So a second terminal row fails only its own invocation, and that invocation holds nothing else. Re-emit by the same writer and the always-emit `stage:finish` close the "marker without a row" hole. What remains is recorded (N2-4) |

### Blocking

#### R2-B1. Ingest's `chunks_total` and front `expected` have no defined source, and any mismatch with `gents/ocr`'s plan fails or hangs every such book

- **Where the counts come from.** The `ocr` gate needs exactly
  `expected = chunks_total`, and the `front` gate needs the number of chunks
  that cross pages 1..min(30,N). Ingest computes both in `RunFile` before OCR
  runs (§1.1, §2.8, SB-S1). Shelf counted the pages it rasterised itself, so
  in Shelf they could not disagree.
- **`gents/ocr`'s own rules** (packs `ocr/plugins/ocr/source`):
  - PDF pages come from `pdf::page_count` (`plan.rs:79`). That is a lazy PDF
    backend of about 2,300 lines (`pdf.rs`, `pdflazy.rs`, `pdfobj.rs`,
    `pdfdecode.rs`).
  - Ranges are `per = max(20, ceil(count/64))` (`plan.rs:58-75`).
  - A folder is walked **recursively**, sorted by byte-wise file name.
    Hidden entries are skipped, and every other file becomes its own chunk,
    whatever its type (`main.rs:418-455`, `graph.rs:73-97`).
  - Chunk indices start at 0.
  - If the page count fails, the plan still emits one chunk with no `pages`
    (`graph.rs:74-77`).
- **What the design leaves out:**
  - SB-S1 lists no PDF page counter. Its source is `S/jobs/common/pdf.go`
    (pdfcpu), and SB-01's crate list copies only the zip/deflate and XML
    readers from the ocr crate.
  - The SB-S2 acceptance checks the range maths ("equals plan.rs"), but not
    the page count it is fed.
  - For image folders, nothing says the copied `source/` folder must hold
    exactly the files ingest counted.
- **Effect:** any disagreement breaks the book through F-18 (members must
  match `expected`).
  - **Too many members** (OCR plans more chunks than ingest counted, for
    example after a stray non-image or a nested folder): the `ocr` group
    quiesces for good. Link never starts, and the watchdog fails the book
    72 h later with "pipeline did not finish".
  - **Too few members:** the group times out after 72 h, and `stage:ocr`
    fails the book with missing members, under either policy.
  - Either way, a book Shelf would complete is failed. For image folders,
    the byte-wise sort also puts `p10.png` before `p2.png`, so page numbers
    come out scrambled unless ingest controls the names.
- **Fix:**
  1. Ingest computes `page_count` with the ocr crate's own
     `pdf::page_count`. Copy `pdf.rs` and its dependencies into
     `plugins/book/source/ocrcount/` with a sync script and a byte-for-byte
     drift `#[test]` against packs `ocr/source`, the same pattern as
     decision 9. Ingest also uses `plan.rs::ranges` verbatim. Add a SB-S1
     case: for the e2e PDF and the ocr pack's fixture PDFs,
     `chunks_total` equals the number of OcrChunks `ocr-plan` emits.
  2. For image folders, ingest copies **only** the files it counts, flat,
     renamed `NNNN.<ext>` in its own order, and also sets `OcrJob.files` in
     that order. `page_assemble` maps `OcrChunk.source` to a page through
     `RunFile.files[]`, never through the chunk index.
  3. If ingest cannot count the pages (the ocr plan would then emit one
     page-less chunk), it fails the book at once with the counter's error.
     It must not write a guessed `chunks_total`.

### Non-blocking

1. **Who releases `toc_extract` is undefined.**
   - §2.7 has `result:toc_extract` and `stage:toc_extract_closed`, but no
     `stage:toc_extract`. The diagram has `toc_finder_closed` write
     "`StageTask{toc_extract}` (1 item)" directly.
   - If no item opener and no stage opener `BatchOutcome` are written, then
     with R17 working the lone failed `ItemOutcome` waits 4 h. The stage
     group then waits 72 h for its missing opener. That ties the finish
     watchdog, so the book fails late with "pipeline did not finish"
     instead of with Shelf's toc_policy reason.
   - **Fix:** `toc_finder_closed` calls the SB-C1 fan-out helper, with item
     opener, batch opener and task file. Add a SB-S5 case: toc_extract Task
     lost → `toc_extract_closed` → `require` failure, within one batch
     timeout.
2. **Where the polish "skipped" outcomes go in batches is undefined.**
   - `classify_closed` writes `ItemOutcome{skipped}` for every chapter that
     is not audio, at once. If those share `batch_ref`s with polish items
     that are released later, the later batch's clock starts at the first
     skipped member (F-18), not at its release.
   - A batch released more than 4 h after `classify_closed` then times out
     before its polish Tasks run. `_next` marks them `closed`,
     `polish_closed` records them as never reported, and the book fails.
     Shelf has no such path.
   - **Fix:** put skipped items in batches of their own, each with an
     opener, so they fill at once. Or emit each skipped outcome only when
     its batch is released. Add a SB-S8b case with more than 32 audio
     chapters mixed with non-audio ones.
3. **`stage:task_retry` can act on a stale failure.**
   - It checks only `results/` and `closed`. Suppose attempt `1.0` had a
     schema reject (so `1.1` is already running) and then the Task ended
     non-success. `task_retry` issues `2.0` alongside `1.1`.
   - Unique `outcome_ref`/`spawn_ref` keep this from corrupting anything,
     but it can spend budget twice, which shrinks the effective budget for
     link, where running out fails the book.
   - **Fix:** do nothing unless the failed `task_ref` is the newest entry in
     the task file's attempt history.
4. **The watchdog itself has no watchdog.** If `stage:finish`'s own commit
   fails 3 times, or the server restarts during it or during the finish
   `gate_open` (F-20: an interrupted call is not run again), no terminal row
   is ever written. This is the "stuck after restart" case the README
   already lists. Name `stage:finish` there explicitly, along with the
   repair command (a crafted `StageStart{finish}`).
5. **A progress `status_ref` can collide inside one invocation.**
   - The ref is `<run_id>/progress/<writer_ref>`, and `writer_ref` is per
     invocation. Several ports emit `BookStatus` with cardinality many
     (`page_assemble`, `stage`, `work`, `result`). Two progress rows from
     one call get the same ref, and the whole transaction fails. In
     `page_assemble` that loses the chunk's `BookPage`s and `Signal`s.
   - **Fix:** `<run_id>/progress/<writer_ref>/<n>`.
6. **D22 names only one cause of `complete:false`.**
   - `gents/ocr` also stops at its **time** budget ("the OCR time budget of
     this call ran out", `ctx.rs:16`, `main.rs:208`). It then sets `next`
     with a cursor, and `graph.rs:136` writes `complete: false`. The pack
     never continues from that cursor.
   - On slow local OCR this can fail books routinely under `fail`, with the
     wrong reason "truncated at 1.5 MB". The e2e "must" accepts a failed
     book, so the run would not catch it.
   - **Fix:** take the reason from the document's `warnings`/`error`.
     Record the time-budget cause in D22. Have X-00a measure
     `complete:false` on the e2e PDF with the operator's OCR profile.
7. **Two variant rules are inconsistent.**
   - `finish_expected = members(variant)+1`, but the members table has a
     separate `epub` row. An EPUB with `variant: text-only` or `ocr-only`
     would wait for an `ocr` member that never comes. Define
     `members(format, variant)`, with EPUB ignoring the variant (Shelf's
     import has none), or refuse those combinations at ingest.
   - §2.5 says toc_finder carries Task-failure text as
     `PreviousAttempt.Reasoning`, while §2.6 has `stage:task_retry` (in
     `gates.rs`, which never calls C6) reuse the same `user_prompt` for
     every stage except link. Shelf passes `nil` (`createBookOpRetryUnit`),
     so "same prompt" is the faithful one. Correct §2.5 and the SB-S5
     table.
8. **Parsing `task_ref` needs a character rule.** `TaskFailed.run_id` is
   derived from `<run_id>/<stage>/<item_ref>/<a>.<r>`. `from_fetch` builds
   `run_id = "sb-" + FetchedSource.run_id` from a foreign value. Ingest
   must refuse a `/` in `run_id`, and `item_ref` must never contain one, or
   the parse must work from the right with a fixed number of segments.
9. **The SB-C9 rebuild needs one more rule.** Gents returns an omitted
   optional field as `null`. Shelf's schemas mark some optional properties
   as non-nullable, for example toc_finder's `toc_page_range` and
   `structure_summary` objects when `toc_found=false`
   (`toc_finder/tools/write_toc_result.go:24-49`).
   Rebuild the object only from non-null fields, and leave a property out
   when its schema type does not allow null. Otherwise a valid "not found"
   answer spends repairs and attempts. Add a case for it.
10. **`TocLinkResult.entry_ref` has no defined producer.** Shelf's
    `write_result` is `{scan_page?, reasoning}`. The surface fills only the
    5 CTX fields, so `entry_ref` would be a field the model writes, which
    C1 does not record. Drop it: `result:link` takes the entry from
    `tasks/link/<item_ref>.json`. Or list it as port-only and strip it.
11. **D8's wording is wrong in one case.** "The same set once the prefix is
    complete" is false when pages 1..30 hold fewer than 20 non-empty pages,
    for example scans with blank versos or plates. Shelf would then read
    later pages that are already OCR'd, and the design reads fewer. Record
    that difference.
12. **Wording.** "Every `stage:*` handler … releases nothing once the book
    is terminal" must exempt `stage:finish` (which always emits) and
    `stage:task_retry` (harmless, but listed twice). Ingest refusals made
    before a run folder exists (bad prefix, more than one path, more than
    256 images) need a defined marker location, or no finish opener;
    otherwise the 72 h `stage:finish` binds a folder that does not exist,
    and that call fails (harmlessly).

### Checked and still intact

These still match Shelf, re-read in source for this round:
- The stage order.
- The variant flags.
- The front prefix rule (`state.go:57-61`).
- The link gate {toc, ocr}.
- The budgets: 3 for every stage, including Shelf's `MaxBookOpRetries` for
  link Task failures (`job.go:347`).
- Classify and polish provider failures are charged to their own stage
  budget (`deferFailedToHandler`).
- toc_finder not found after 3 attempts gives Shelf's message under
  `require` (`toc_finder.go:241-251`).
- Link fails closed.
- Pattern out of retries gives 0 entries.
- Discover and gap out of retries are counted complete.
- Classify exact coverage.
- Polish uses a mechanical fallback and the book then fails; non-audio
  chapters are passed through.
- `ApplyEdits` runs on the whole chapter text.
- The `validate_quote` version check runs before the search.
- The digest algorithm.

The strict schemas list exactly the `required` sets the design keeps
(`metadata/schema.go:76`, `extract_toc/schema.go:40,46`,
`structure_prompts.go:197,221,226`, `pattern_analyzer/prompt.go:160,173,179`).
The revision log's round-2 rows match the body, except for N2-7.

**Verdict:** one blocking item (R2-B1). Every earlier blocking item (F1-F8,
R1-B1, R1-B2) is fixed in substance.

---

## Round 3 re-review

Reviewed: `DESIGN.md` revision 3, against Shelf (`S = shelf/internal`), gents
`origin/main` (fetched again today: still `d4df8a02b`) and packs
`origin/master`. Packs master has moved from `cd00bb8` to `3605ae8`
(`470c291`, "Declare correlation_field on every scenario trigger's event
source"). That commit touches only `background_continuation`, `lsp_rust` and
`pipeline`, and `gents/ocr` is unchanged, so F-42 and F-45 still hold. X-01
should record the new base SHA. The lens is the same as before. Only items
that would make the build or the runtime fail, or would silently break a
Shelf guarantee, are blocking.

### Status of earlier blocking items

| Id | Status | Evidence checked |
| --- | --- | --- |
| F1 | Fixed in substance | No change since round 2. The toc_extract release through the C1 fan-out helper (FID-R2-N1) closes the last single-item edge. R17 is still the runtime spike, and the D14 fallback is written down |
| F2 | Fixed | `require` is the default. `gap_closed` owns the zero-entry branch alone (SB-S7b). `structure` never branches on the policy |
| F3 | Fixed | `fail` is the default. D22 now names both causes of `complete:false` (byte cap and time budget, packs `ctx.rs:16`, `graph.rs:120-140`). The diagram text still shows only the 1.5 MB wording (N3-6) |
| F4 | Fixed | Metadata out of retries fails the book. `degraded` means quarantined pages only |
| F5 | Fixed in substance | Null-list → `[]`, null non-list fields left out, CTX and port-only fields stripped, 2 repairs per attempt (`maxStructuredRepairAttempts = 2`, `providers/structured_output.go:14`, re-read). One rebuild rule is still unstated: flat-to-nested renames (N3-2) |
| F6 | Fixed | C7 |
| F7 | Fixed | The latest canonical is always used, and `read_passage_at_version` never reports `version_match` |
| F8 | Fixed | `members(format, variant)`, EPUB ignores the variant, and there is one `finish_signal` builder |
| R1-B1 | Fixed | `StageTask.handoff_id`, and `sb-fire` is keyed on `source_handoff_id` |
| R1-B2 | Fixed | The unique `status_ref`. Progress refs now carry `/<n>` (decision 4.4, §1.1). The §2.4 SDL comment still says `<run_id>/progress/<writer_ref>` (N3-6) |
| R2-B1 | **Fixed in substance** | `ocrcount` copies `gents/ocr`'s own counter and `ranges` (`plan.rs:16-18, 58-75`, re-read). `OcrJob.files` fixes the order: `main.rs` reads `files` in the order given and walks the folder only when `files` is absent (re-read). `page_assemble` maps `source` through `RunFile.files`. The `stage:ocr_plan` check turns any remaining mismatch into a failure within 30 min. That works because `evaluate_group` builds the members from a live query on the correlation (`event_delivery.rs:330-420`), and every OcrChunk of a plan is written in one transaction. Three gaps remain, all non-blocking (N3-3, N3-4, N3-5) |

### Blocking

#### R3-B1. Four bindings project `workspace_original`, which is not a field of their source collection, so ingest, page assembly and every bound stage stall

- **Design:** the §2.6 `input_fields` table lists `workspace_original` for
  four bindings, and none of their source collections has that field in its
  §2.4 SDL:
  - `sb-ingest` (BookJob);
  - `sb-page-assemble` (BookChunk);
  - `sb-stage` (StageStart);
  - `sb-work` (WorkItem).

  §2.6 itself states the rule that breaks this: "a name the source
  collection lacks stalls that binding's arrival cursor for good". §2.10 adds
  a `defs.json` assertion that "every name exists in the source collection's
  SDL", and SB-W1's acceptance requires "`input_fields` equal the §2.6
  table". The two cannot both pass.
- **gents (main):**
  - The per-document path is `materialize_for_binding` → `fetch_source_doc`
    (`callback/scan.rs:529, 846-862`). It refuses any projected field that the
    collection's introspected fields lack: `ensure!(available.contains(field)
    || field == "_docID", "callback input field {field} is not a safe scalar
    source field")`.
  - That error goes up through `?` in the arrival loop (`scan.rs:389-390`)
    before `checkpoint`. The cursor never moves on, and the same document
    fails again on every tick.
  - Config publication does not catch this. `validate_callback_binding`
    (`callback/documents.rs:205-231`) checks only the name syntax, duplicates
    and secret names.
  - The host does not need the field in the source. `PluginRunner` inserts
    `original_field` into the arguments itself, after the projection, on
    every bound call (`plugin.rs:510-536`: `object.insert(field,
    bound.original())`).
- **Effect:**
  - Install passes.
  - At runtime, the first `BookJob` stalls `sb-ingest` for good. No book ever
    starts, and no terminal `BookStatus` is written, because ingest never
    writes the finish opener.
  - If ingest is fixed alone, `sb-page-assemble`, `sb-stage` and `sb-work`
    stall the same way. That is every bound deterministic stage.
  - In the build, SB-T1's `runtime_pages.json` (BookChunk →
    `sb-page-assemble`) never produces `BookPage`s, and the `defs.json`
    assertion contradicts SB-W1. So the build fails too, whichever of the two
    the integrator follows.
- **Fix:**
  1. Remove `workspace_original` from all four rows of the §2.6 table. The
     host fills it on every bound call because `book_pipeline` declares it as
     `original_field`.
  2. Say this in §2.6, next to the projection rule: "a bind_dir
     `original_field` is host-filled and is never listed in `input_fields`".
  3. Add it to `defs.json`: no `input_fields` entry equals a plugin's
     `bind_dir.original_field`, and every entry exists in the source SDL.
  4. Add an SB-T1 runtime case that seeds a `BookJob` and expects a `BookRun`
     (or the lone refusal row). It would have caught this.

### Non-blocking

1. **A missing `gents/ocr` plan still ends in the 72 h watchdog.**
   - `stage:ocr_plan` runs only when at least one `OcrChunk` exists. A group
     with no members never times out (F-18, `evaluate_group` returns `Empty`).
   - If `ocr-plan` errors, for example "the job holds more files than one
     plan lists" (`graph.rs:66-70`) or a source it cannot open, it writes no
     OcrChunk. Its callbacks have no `max_attempts` (F-45), so it does not
     try again.
   - The front, ocr and link gates then get no members. The book fails only
     at 72 h with "pipeline did not finish: missing …". Shelf fails at once
     when rasterising fails.
   - **Fix:** record it in D25, and have the README name it. Better: ingest
     writes an opener `Signal{gate:"ocr", member:"opener"}`, with the ocr
     `expected` raised by 1. The `ocr` gate then times out with only the
     opener and fails with "gents/ocr planned no chunks".
2. **SB-C9 needs a per-stage field map, not only null rules.** Shelf's
   schemas are nested where the port's surfaces are flat, or renamed:
   - toc_finder's `toc_page_range: {start_page, end_page}`
     (`toc_finder/tools/write_toc_result.go:24-31`; required inside the
     object) arrives as flat `start_page`/`end_page`;
   - gap's `entry_doc_id` arrives as `entry_ref` (C6).

   If the rebuild copies flat fields as they are, a valid `toc_found=true`
   answer has no `toc_page_range`, and that range is never checked against
   the schema. **Fix:** give each stage in `contract.rs` an explicit
   flat→logical map: `start_page`+`end_page` → `toc_page_range` (left out
   when both are null), `entry_ref` → `entry_doc_id`. Add an SB-C9 case for
   each.
3. **The image types ingest counts must equal the image kinds `gents/ocr`
   reads as one page.**
   - D26 says that "only files ingest recognises as images are copied". If
     ingest accepts a file that `detect::detect` refuses (an unsupported
     format), the plan still emits one chunk for it, so the counts agree,
     but its extract fails. Under `fail`, the book then fails.
   - A multi-page TIFF is a bigger problem. Ingest counts it as 1 page, and
     ocr's `split_pages` treats any non-`PAGED` format as one page
     (`graph.rs:31`), so the extra pages are lost silently.
   - **Fix:** ingest accepts exactly ocr's single-image kinds (copy the list
     from `detect.rs` into `ocrcount`, under the drift test), and refuses a
     multi-frame TIFF with a message.
4. **The plan check compares `source`, but `RunFile` stores no PDF source
   name.**
   - For a single file, `OcrChunk.source` is the file name of the bound link.
     `for_file` keeps the name (`plugin/bound.rs:128-140`), so it is
     `source.<ext>`. `RunFile.files[]` covers image folders only.
   - **Fix:** store `RunFile.source_name` (or `files = ["source.pdf"]` for a
     PDF), and say which field `stage:ocr_plan` compares.
5. **`canonical/book.json` can be written before metadata exists.**
   - Structure does not wait for metadata. Shelf's does not either, and
     Shelf's `get_book` reads the live book row (`researchmcp/client.go:50-62`).
   - Here `commit_closed` freezes the `BookMetadataFile` at promote time. A
     book whose metadata finishes after structure keeps a canonical file with
     no title or author for good, while the run ends `complete`.
   - The digest has no metadata in it, so citations are unaffected.
   - **Fix:** either `get_book` reads `runs/<run_id>/` metadata for the
     canonical file's `run_id` (the bind is on the book folder, so it can),
     or `stage:finish` re-promotes the canonical file with the metadata.
     Record whichever is chosen.
6. **Wording drift.**
   - The §2.4 `BookStatus.status_ref` comment lacks `/<n>`.
   - The §2.8 diagram still says "ocr chunk c truncated at 1.5 MB". D22's
     format is `"ocr chunk <c> incomplete (<cause>): pages <x>-<b> missing"`.
   - The diagram lists "page count fails" among the refusals made before a
     run folder exists, but places the count after the copy into
     `runs/<run_id>/`. Count the original first, or say that the copied
     folder is left behind.
7. **`writer_ref`'s "source ref" is not defined for StageStart-sourced
   handlers.**
   - StageStart has no unique ref. Per-document ports leave `cause_ref`
     unset, and `sb-stage` does not project `_docID`.
   - Any deterministic choice works, because a mismatch only defers the row
     to the `stage:finish` repair. But SB-C1 needs one.
   - **Fix:** add `_docID` to the bound bindings' `input_fields` (allowed,
     `scan.rs:855-858`) and use it as the source ref.
8. **No port should set `required: true`.** `output_documents` fails a call
   whose `required` port is empty (`callback/plugin.rs:119-124`), and the
   default is `false` (`graph_pipeline/types.rs:27-29`).
   - The design calls only some ports "(optional)".
   - Suppose an implementer sets `required` on, for example, `sb-ingest`'s
     `BookRun one`. The pre-folder refusal (a lone `BookStatus`) then fails
     3 times, and the book gets no row at all.
   - **Fix:** state "no port is `required`" in §2.6, and assert it in
     `defs.json`.

### Checked and still intact

These match Shelf, re-read in source for this round:
- The stage order and the variant flags.
- The front exact-prefix rule (`state.go:55-61`).
- The link gate {toc, ocr}.
- Budgets of 3 per stage, with pattern exhausted → 0 entries, and discover
  and gap exhausted → counted complete.
- Link fails closed.
- Classify exact coverage. Polish runs only for audio chapters and fails
  closed. `ApplyEdits` runs on the whole chapter text with first-occurrence
  `strings.Replace` (`common/structure_text.go:163-173`).
- `validate_quote`'s version check runs before the search.
- The `shelf-research-v1` digest (`researchmcp/client.go:72-100`). It hashes
  book id, source sha, chapters and passages only, so metadata timing
  (N3-5) cannot change it.
- `requireResearchReady` (`client.go:125-133`).

The round-3 revision-log rows for FID-R2-B1 and FID-R2-N1..N12 match the body
of the design.

**Verdict:** one blocking item (R3-B1). It is a one-line fix per binding, but
as written it stops every book at ingest, and the build's `defs.json` and
SB-W1 acceptance contradict each other. Every earlier blocking item (F1-F8,
R1-B1, R1-B2, R2-B1) is fixed in substance.
