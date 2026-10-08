# Review: DESIGN.md §6 implementation work-list

Lens: are the tasks atomic, file-disjoint and correctly ordered by `depends_on`, with checkable
acceptance criteria and sensible model tiers? Reviewed against `shelf-inventory.md`,
`gents-pack-guide.md` and the source code: gents `main` at `e8774ade3`, packs `main` at `470c291`,
and shelf. Every claim below marked "verified" was checked in source during this review.

**Verdict: not ready to dispatch.** The task boundaries are mostly good, but five structural
problems would stall the parallel phase within the first hour:

- B1: no task can run its acceptance command until SB-W2.
- B2: parallel agents share one working tree and one crate.
- B3: the golden-fixture plan cannot compile.
- B4: the workspace-file contracts between tasks are unowned.
- B5: two pipeline bugs, the image-folder metadata gate and the liveness hole.

Each item gives the fix. Section 8 is the revised task delta.

---

## 1. Blocking problems

### B1. `gents pack test` refuses the pack until SB-W2, so almost no acceptance criterion can run before it

- **Verified.** `gents-cli/src/commands/pack/test.rs::test` first runs `check_dir` and stops on
  any problem: `ensure!(check.problems.is_empty(), "... failed the check")`.
- `check_files` also fails on every declared asset that is missing. It fails on every present
  file that is undeclared too, except under a plugin's `source` dir and dotfiles.
- So until SB-W2 writes `pack_config.json` and the final `assets`, every one of these is
  uncheckable: "plugin cases: ..." in SB-S1..S10, SB-P1 and SB-P2, the SB-02 line "`gents pack
  check` reports no schema errors (other errors allowed)", and the BD-C*/BD-F1 canned cases.
- The S tasks could fall back to `cargo test`, but the JSON plugin cases they are told to write
  would then go unexecuted until W2. W2 would discover ~60 failing cases owned by ~20 finished
  tasks. That is the worst possible place to find them.

**Fix.**
- Make SB-01 (and BD-01) deliver a pack that is **check-green and test-green on day one**:
  - every asset in the final §2.1 list exists as a valid placeholder. Each SDL file gets
    `type X { run_id: String @index }`, each prompt gets a one-line stub, and `tests/*.json`
    gets `{"defs":"","jq":[]}`-style trivially-true cases;
  - `pack_config.json` is the singletons only (`agent_principal`, `compactions`) with empty
    arrays elsewhere;
  - all 9 behaviors are already declared in `manifest.inference_slots`.
- Later tasks replace placeholders in place and never add or remove manifest entries.
- Acceptance for every later task becomes "`gents pack test P` passes in your worktree, plus
  your new cases are listed as passed in its JSON output".
- Also remove the SB-02 escape clause "other errors allowed".

### B2. Parallel tasks share one branch, one working tree and one Cargo crate

- The rules say "Work in the packs branch `feat/structured-book-packs`". SB-C1..C8 and
  SB-S1..S10 all compile as **one crate** (`book_pipeline`).
- One agent's half-written `stages/finalize.rs` breaks `cargo build` and `gents pack test` for
  every other agent.
- **Verified:** `gents pack build` also mirrors the source into `<pack>/target/plugins/<name>/`.
  Concurrent builds in one tree overwrite each other's mirror.

**Fix.**
- Add to the rules: "each task runs in its own worktree:
  `git -C <packs> worktree add ../wt/<id> -b sb/<id> feat/structured-book-packs`. Commit only
  your target paths; the integrator merges `sb/<id>` branches in `depends_on` order."
- Commit `Cargo.lock` in SB-01 and forbid committing changes to it. Builds regenerate it, so it
  is the one file every task would otherwise touch.
- File-disjointness then makes the merges conflict-free, which is the point of the design.

### B3. The golden-fixture plan cannot compile: Shelf code is `internal/` and mostly unexported

- SB-C5, C6, C7, C8, S2 and S10, plus §2.10 item 1, say goldens come from "a throwaway Go
  program under the scratchpad that imports `shelf/internal/...`".
- **Verified:** `shelf/go.mod` is `module github.com/jackzampolin/shelf`. Go refuses to import
  `.../internal/...` from outside that module tree.
- Many of the targets are also unexported (`validateTocLinkEvidence`, `generateEntriesToFind`,
  `findFinalizeGaps`, `appendLinkTocRetryHint`). So they cannot be called even from a `cmd/`
  inside the module.
- Six tasks would each rediscover this and each invent a different fixture input format.

**Fix.** Add one task, **SB-G0 "Golden generator"** (sonnet, M, depends_on X-00):
- It writes generator files as **`_test.go` files placed in the target packages through
  `go test -overlay overlay.json`**. Nothing on disk in shelf changes, and the files can call
  unexported functions.
- The generator source lives in `H/goldens/` with a `run.sh`.
- It emits `core/fixtures/{prompts/front,prompts/finalize,prompts/structure,canonical,evidence,finalize,epub}/*.json`
  in one documented `{input, output}` shape. Inputs are the Go structs' JSON.
- C5..C8, S2, S5, S7 and S10 then depend on SB-G0. They own only the Rust side and the
  committed fixture subset they need.
- Acceptance: `H/goldens/run.sh` regenerates byte-identical fixtures, and `git -C shelf status`
  is clean afterwards.

### B4. The on-disk workspace files are the inter-task API, and nobody owns their format

- §1 fixes only `pages/<NNNN>.json`. These are written by one task and read by another, with no
  schema anywhere:

  | File | Writer | Reader |
  | --- | --- | --- |
  | `tasks/<stage>/<item_key>.json` | S5, S6, S7 | C3 (ValidateCandidatePage needs the entry and the back-matter range), C4/P1 (`get_page_ocr` returns `write_result_ready`), S3 (missing-item diff) |
  | `toc/entries.json` | S5 | S6 |
  | `toc/final.json` | S7 | S8 |
  | `chapters/<id>.json` in its 4 states | S8 | S9 |
  | `results/<stage>/*.json` | S5..S8 | S3 |
  | `run.json` | S1 | S2, S3 |
  | `Signal.payload` and `StageStart.payload` | all | gates (S3) |

- The callback **output port names** are also unfixed. §2.7 says each module returns "an object
  keyed by output port name". The port names are invented by SB-W1, which runs in parallel with
  the S tasks that must emit them.

**Fix.** Move all of this into SB-01 as Rust types, and make it the one contract every task
compiles against:

- **`core/contract.rs`**, owned by SB-01 and read-only for everyone else, holds:
  - serde structs for every workspace file above, including a `TaskFile` enum per stage;
  - payload structs for each `gate` and `stage`;
  - `const` output port names, using one rule: port name = snake_case collection
    (`stage_task`, `item_outcome`, ...);
  - the stage-name table (see G3).
- SB-W1 reads the port names from that file.
- Add C3, C4 and S3 to the readers that must deserialize `TaskFile` in their tests.

### B5. Two pipeline bugs that tasks will faithfully implement

- **Image folders break the metadata and front gates.**
  - **Verified** in `ocr/source/graph.rs::plan`: non-paged inputs (images) get **one chunk per
    file** (`_ => vec![""]`). Only paged formats use `ranges(count, per)`.
  - So "chunk with page 1 → metadata_prepare" gives the metadata prompt **one** page, where
    Shelf uses `MetadataPageCount = 20` (`jobs/common/metadata_ops.go:16`).
  - `Signal{front, expected=ceil(min(30,N)/per)}` is wrong for images; it should be `min(30,N)`.
  - **Fix:**
    - drop `work:metadata_prepare` and its WorkItem hop;
    - have the **front gate open both stages**: 30 ≥ 20, so `gate_open(front)` emits
      `StageStart{toc_finder}` and `StageStart{metadata}`;
    - compute `expected` from `run.json` as "number of chunks whose range intersects 1..30" for
      both PDFs and image folders;
    - move `metadata` start into `toc.rs` or `metadata.rs`;
    - add an image-folder case for the front `expected` to S2's acceptance.
- **Liveness hole: groups with zero arrivals never time out.**
  - **Verified** in `trigger_engine/event_source.rs::reconcile_due_and_rotating_groups`: the
    timeout is measured from `timer.first_seen`, the first member's arrival.
  - Decision 5's claim that "Group `timeout_secs` makes a lost item close as `failed`" therefore
    holds only if at least one item in the batch landed.
  - If every Task in a batch dies, which is exactly what the dead-endpoint fast-fail of guide
    §5.6 does when a backend drops, the book hangs forever.
  - The same is true of every single-item model stage: metadata, toc_finder, toc_extract and
    pattern have no group at all. A dead toc_finder Task leaves `Signal{link}` at 1 of 2 forever.
  - **Fix:**
    - every fan-out writes one **sentinel** `ItemOutcome{status:"sentinel"}` per batch, with
      `batch_size = n+1`, so the timer starts at fan-out;
    - every single-item model stage runs as a batch of 1 plus the sentinel, so all model stages
      share one close path;
    - put the helper in `core/docs.rs` (C1);
    - add "all items lost → stage closes failed after timeout" and "single toc_finder Task lost
      → policy path" cases to S3;
    - state the per-gate partial-group policy (what `gate_open` does with 1 of 2 `link`
      members) in DESIGN, because S3 cannot invent it.

---

## 2. Gaps (missing tasks or missing scope)

| # | Gap | Fix |
| --- | --- | --- |
| G1 | **Foreign-collection sources fire on every OCR job and every download, not just ours.** `sb-ocr-chunk` and `sb-ocr-doc` are unfiltered. A user's ad-hoc `OcrJob` on `~/Documents/x.pdf` becomes a ChunkSlot, then a BookChunk with `workspace = dirname(path)`, then a **read_write** `page_assemble` that writes `pages/` into the user's folder. Every `OcrDocument.markdown` (up to 1.5 MB) is also duplicated into ChunkSlot. Likewise every standalone `FetchedSource` auto-starts the full LLM pipeline. | Filter `sb-ocr-chunk` on `path: {_like: "%/books/%/runs/%/source%"}`. Make `chunk_join` and `page_assemble` no-ops when the path slot is missing or the workspace has no `run.json`, and add a case for it. Make book creation opt-in: a `DownloadJob.structure: Boolean` copied to `FetchedSource`, filtered in `sb-fetched`. Add R14 to X-00: is `_like` accepted in an event-source filter on a foreign collection? |
| G2 | **X-00 does not test the one capability everything rests on:** a **callback** (not a model tool) with `bind_dir` `read_write` on a document-chosen path inside an allowed folder, under a headless `gents server`. R5 covers only the model-tool case. | Add R15. Also R16: does a `cardinality: many` port accept around 800 BookPage docs (about 4 MiB) in one call? |
| G3 | **No behavior→stage→ids table.** Behavior dirs are `toc_entry_finder`, `pattern_analyzer`, `chapter_finder`, `gap_investigator`, `structure_*`, but the stages are `link`, `pattern`, `discover`, `gap`, `classify`, `polish`. The names `sb-<stage>-writes`, `sb-<stage>-tools`, `sb-<stage>-task`, `sb-task-<stage>` and `sb-r-<stage>` are derived from the stage. Only SB-B6 mentions its stage. Haiku agents will name things after the directory. | Add a 9-row table to §2.5: dir, behavior_id, stage, task_id, surface_id, tools_id, write tool, Result collection, event source and trigger ids. Make SB-T1's `defs.json` assert it. |
| G4 | **Fragment format is unspecified for haiku.** "Fragment ... holds behavior, context, tools, surface, task, event source and trigger" leaves the JSON shape to each agent. `merge.sh` (SB-01) cannot be written without it either. | SB-01 commits `.build/fragments/_example.json`, a complete metadata fragment, plus `.build/check-fragment.sh <file>`. The check covers jq validity, the 10 CTX fills, `output_obligation`, slot id and the stage name from G3. B1..B9 acceptance becomes "`check-fragment.sh` exits 0". |
| G5 | **Pack singletons are owned by nobody.** `agent_principal`, `compactions[sb-agent-compaction]` (referenced by 4 agent contexts) and the `.build/notes` → `H/build-notes` copy are not in any fragment. | Put the singletons in SB-01's day-one `pack_config.json` (see B1). `merge.sh` concatenates arrays into it. |
| G6 | **`sync-core` arrives after the tasks that need it.** SB-P1 and SB-P2 must copy `core/` into their crates, because path dependencies do not survive the build mirror, but the script is created in SB-W2. P1 and P2 would hand-copy whatever stubs existed at their start. The claim "CI fails on a diff" also has no owner: no task edits `.github/workflows/ci.yml`. | Create `scripts/sync-core.sh` (with `--check`) in SB-01. P1 and P2 run it before building. W2 runs `--check`. Either add `ci.yml` to W2's target paths explicitly or drop the CI claim. |
| G7 | **Pack-level `tests/*.json` vs manifest ownership.** SB-T1 and BD-T1 create `tests/install.json` etc., but those must be listed in `assets` (verified: the ocr manifest lists `tests/*.json`), and the manifest is owned by SB-01/W2. Either W2's check fails on declared-but-missing files, or T1 must edit the manifest. | B1's placeholders solve it: SB-01 declares and stubs `tests/{install,defs,runtime_pages}.json`, and T1 only replaces their content. |
| G8 | **Makefile edits are wrong and redundant.** W2 targets `Makefile (targets test-structured_book, sync-core)`. **Verified:** the packs `Makefile` already has the pattern rule `$(addprefix test-,$(PACKS)): test-%`, with `PACKS` from `wildcard packs/gents/*`. Defining `test-structured_book` explicitly overrides the recipe with a make warning. BD-T1's `make test-browser_download` already works with no edit. | Only `sync-core` goes in the Makefile, owned by SB-01 (see G6). |
| G9 | **Packs README index row.** `packs/README.md` has a table of packs (`| [ocr](packs/gents/ocr/README.md) | ...`, line 364). No task adds the two new rows. If SB-D1 and BD-D1 both add them, they overlap on one file. | Give both rows to SB-D1 only, and add `packs/README.md` to its target paths. |
| G10 | **`runtime_pages.json` mechanics.** **Verified:** `scripts/test-pack.sh` runtime cases run in a throwaway git repository, substitute `${REPOSITORY}`, and are read-only unless `"access": "read_write"`. The seeded `BookChunk.workspace` must therefore be `${REPOSITORY}/books/<sha>/runs/<id>`, with the fixture copied in through `"copy"`. | Put this in SB-T1's acceptance, or the task will try an absolute fixture path and fail. |
| G11 | **Truncated OCR chunks.** **Verified** in `ocr/source/graph.rs::extract`: an `OcrDocument` can have `complete: false` with a `cursor` and no continuation (the extract hit `EXTRACT_MAX_BYTES`). The pages after the cutoff silently do not exist. | Add the case "complete:false chunk → missing tail pages quarantined" to SB-S2. Port `split_pages` from `graph.rs`, which is listed only as a "source" today. |
| G12 | **Oversized chapters.** S9's acceptance says "each chapter commit stays under 4 MiB (truncation test)", but no rule says what happens when one chapter's `mechanical_text + polished_text + paragraphs` exceeds 4 MiB, so the test cannot be written. | Specify it: Chapter docs carry only `polished_text` (`mechanical_text` stays in `chapters/*.json`), and a commit item covers at most N paragraphs, with continuation `WorkItem{commit, part}`. |
| G13 | **Spike outcome has no re-plan gate.** R2, R4, R5 and R6 each change task boundaries (R5-no rewrites P1 and C4; R6-no rewrites BD-F1). | Add **X-01 "Amend DESIGN from SPIKE.md"** (opus, S, depends_on X-00). SB-01 and BD-01 depend on X-01, not X-00. |
| G14 | **WorkItem stages have no task files**, so S3's "missing-item diff against `tasks/<stage>/`" cannot work for `commit`. | The C1 fan-out helper writes a `tasks/<stage>/<item_key>.json` marker for every item, model or deterministic. |

---

## 3. Overlaps (two tasks write one file)

| File | Tasks | Fix |
| --- | --- | --- |
| `P/plugins/book_search/source/main.rs`, `P/plugins/book_research/source/main.rs` | SB-01 creates them. SB-P1 and SB-P2 own `source/**`, but the rule "Do not edit `main.rs`" forbids it | Exempt P1 and P2 from the `main.rs` rule by name, or have SB-01 not create those stubs |
| `P/plugins/*/source/core/**` (the copies) | SB-P1 and SB-P2 (hand copy), then SB-W2 (sync) | `sync-core.sh` is the only writer (G6), and P1/P2 do not commit `core/` edits |
| `P/manifest.json` | SB-01, SB-W2, and implicitly SB-T1 (G7) | SB-01 writes the final shape and W2 only adds plugin `input_schema` tweaks if needed |
| `packs/Makefile` | SB-W2 (and BD-T1 implicitly) | G8 |
| `packs/README.md` | SB-D1 and BD-D1 (implicitly) | G9 |
| `P/README.md` | SB-01 (skeleton) and SB-D1 | Sequential, fine; listed so the merge order is explicit |

---

## 4. Ordering errors in `depends_on`

| Task | Missing dependency | Why (evidence) |
| --- | --- | --- |
| SB-C3 | SB-C2 | Evidence is "adapted ... to Markdown headings plus recovered header and footer". That recovery is C2's `furniture.rs` |
| SB-C4 | SB-C2, SB-C3 | **Verified:** `toc_entry_finder/tools/get_page_ocr.go:45` calls `AnalyzePageEvidence` and sets `write_result_ready` from it, and grep and heading ranking use `ExtractHeadings` |
| SB-C7 | SB-C3 | The toc_entry_finder user prompt carries back-matter context, derived in `link_toc_structure.go`, which is C3's `evidence.rs` scope |
| SB-P1 | SB-C1 | It reads `pages/` and `tasks/` through C1's page store and `TaskFile` |
| SB-P2 | SB-C1 | It reads the `books/<sha>/canonical/` layout from C1 |
| SB-S9 | SB-S8 (or the B4 contract) | It reads `chapters/*.json` in the "polished" state that S8 defines |
| SB-S6 | SB-S5 (or the B4 contract) | It reads `toc/entries.json` written by S5 |
| SB-S3 | SB-S1 (or the B4 contract) | `finish` reads the variant and `chunks_total` from `run.json` |
| SB-W1 | SB-01 | Port names and the stage table (B4, G3) |
| BD-C2, BD-C3 | BD-C1, or move the HTTP round helper into BD-01 | Multi-request resolvers (IA metadata then files.xml; OpenAlex then landing) need the `http_calls`/`http_results`/`state` round loop. It is described only under BD-C1's "framework", which runs in parallel with them. **Preferred:** BD-01 owns `http.rs` (round protocol plus a canned-result test helper) and the resolver registry |
| SB-01, BD-01 | X-01 | G13 |

With B4 in place, the S5→S6, S8→S9 and S1→S3 edges become unnecessary, since they compile
against `contract.rs`. Without B4 they are hard edges.

---

## 5. Tasks too large for one agent

Line counts are from the Shelf source, measured with `wc -l`.

| Task | Size evidence | Split (file-disjoint; the §2.7 dispatch table must change in SB-01 before work starts) |
| --- | --- | --- |
| **SB-C4** (L, sonnet) | 5 tools across 4 agents, ~1,800 Go lines. chapter_finder's `grep_text.go` (303) is a different variant from toc_entry_finder's (346) | **C4a** `core/search_link.rs`: toc_entry_finder grep_text, get_heading_pages, get_page_ocr with evidence (sonnet, M). **C4b** `core/search_front.rs`: toc_finder grep report and `load_page`, chapter_finder variants, gap context (sonnet, M) |
| **SB-S7** (L, opus) | `finalize.go` 392, `finalize_pattern.go` 426, `finalize_discover.go` 420, `finalize_validate.go` 416, `finalize_helpers.go` 591: ~2,250 lines into one file | **S7a** `stages/finalize_pattern.rs`: `stage:pattern`, `result:pattern`, sanitizers, generateSequence, roman numerals, estimatePageLocation, generateEntriesToFind (opus, M). **S7b** `stages/finalize_gaps.rs`: `result:discover`, `discover_closed`, findFinalizeGaps, `result:gap`, applyGapFix, `gap_closed`, resortEntriesByPage (sonnet, M) |
| **SB-S8** (L, sonnet) | `structure*.go` (job) ~1,800 lines plus `common/structure*` ~590 | **S8a** `stages/structure_build.rs`: skeleton, parents, end pages, extract and merge, reuse key (sonnet, M). **S8b** `stages/structure_llm.rs`: classify fan-out and coverage, polish plan, ApplyEdits, failure semantics (opus, M; this holds the fail-closed rules) |
| **SB-W2** (M, opus) | Merge, manifest, sync-core, Makefile, build, check and test green over ~30 upstream tasks. "`gents pack test P` passes" means fixing other tasks' files, which the rules forbid | **W2a** merge fragments into `pack_config.json` and run check (sonnet, S). **W2b** integration triage (opus, M), explicitly allowed to edit any `P/**` file, logging each cross-task fix in `H/build-notes/structured_book/integration.md`. With B1 in place most breakage is caught earlier, so W2b shrinks |
| **BD-F1** (L, opus) | Ranged rounds, redirects, cursor, politeness, sniff, landing-page parse, hashing, checksums, dedup, placement, provenance | **F1a** `transfer.rs`: rounds, Range, redirects, 16 MiB/840 s cursor, part file, Retry-After, spacing (opus, M). **F1b** `finalize.rs`: sniff, `citation_pdf_url`, SHA-256 re-read, md5/sha1/size check, dedup, placement, FetchedSource (sonnet, M) |
| **X-00** (M, opus) | 10 runtime questions across 3 packs, a gents build, and a vision curl to vLLM | **X-00a** structured_book runtime: R1-R3, R5, R7-R9, R14-R16 (opus, M). **X-00b** browser_download runtime: R4, R6 (sonnet, S). **X-00c** R10 vision probe (haiku, S; one curl, no email in the request) |
| SB-S10 (L, sonnet) | `epubimport/parser.go` 765 plus `import.go` 590. It also needs zip (deflate) and XML crates in wasm | Keep it whole, but SB-01 must decide the crates (see A3). Point at `ocr/source/{epub.rs,xml.rs}` as a reusable, already-wasm-proven implementation |
| SB-C3 (L, opus) | ~800 lines of evidence logic plus 17 tests (`ocr_evidence_test.go`) | Keep it whole (opus is right). Keep it on the critical path; see §4 |

---

## 6. Acceptance criteria that cannot be checked as written

| Task | Problem | Replacement |
| --- | --- | --- |
| SB-01 | `jq . manifest.json` passing is a near-empty check | "`gents pack test P` passes (B1); the dispatch test covers every op in §2.7; `scripts/sync-core.sh --check` exits 0" |
| SB-02 | "No schema errors (other errors allowed)" | "`gents pack test P` passes; `ls schemas | wc -l` = 30, and the names equal the enumerated list". **Enumerate the 30 file names in §2.4**; today it says only "`book_job.graphql` ... `book_structure.graphql`" |
| SB-B1..B9 | "`diff` shows only C1 and C4" is a judgement call for haiku and the reviewer alike | SB-01 ships `.build/check-prompt.sh <shelf-src> <port> <C-list>`. It diffs, then requires every changed hunk to match that C-rule's regex (C1: inside the final `## Output`; C2: lines containing `book_search`/`op:`; C3: `load_page`/`page text`; C4: deleted lines matching `web search`; C6: `entry_key`). Acceptance: the script exits 0 and `check-fragment.sh` exits 0 |
| SB-B8, SB-B9 | "Checked with a scripted extraction" names no script | The same `check-prompt.sh` with a `--go-const ClassifySystemPrompt` mode |
| SB-S7 | "Shelf finalize tests ported". Only `finalize_dedupe_test.go` (190 lines) exists | Name it, and list the new cases that cover pattern sanitizers and gap detection from the inventory's §2.7 rules |
| SB-S8 | "Shelf structure tests ported". The candidates are `structure_polish_reuse_test.go`, `structure_response_format_test.go` (594 lines, mostly about provider response_format, which does not apply) and `common/structure_helpers_test.go` | Name `structure_polish_reuse_test.go` and `common/structure_helpers_test.go`, and state that response_format tests are not ported |
| SB-S3, SB-S4, SB-S6 | Generic "cases" | Port the existing tests by name: S3 `completion_failure_test.go`, `fail_book_test.go`; S4 `metadata_test.go`, `metadata_gate_test.go`; S6 `link_toc_test.go` (495 lines), `toc_link_skip_test.go` |
| SB-P2 | "Cases mirror Shelf's research tests". `researchmcp/` has only `http_test.go`, which is transport | Enumerate: one case per op; bounds clamps; validate_quote trim, empty, version mismatch (no search), the 20-occurrence cap and multi-byte rune offsets; and the digest-pinned `canonical/<digest>.json` lookup |
| SB-S10 | "Yields the same chapter list as Shelf": who runs Shelf? | That golden comes from SB-G0 (B3) |
| BD-B1 | "Prompt states the rules" is subjective | jq over the fragment: the tools allow-list equals exactly `web_search`, `web_scrape_url` and `write_source_search_result`; `grep -c` for each of the 4 rule sentences; no host from a shadow-library denylist appears in the prompt |
| X-E2E | "Every criterion green **or** each failure recorded" always passes | Split §4 into **must**: FetchedSource ok with a matching sha256, exactly one OcrJob, BookPage count, a BookStructure digest that matches recompute, the validate_quote true/false/version triple, the dedup rerun and the no-email grep. Everything else is **should**, and only those may be recorded-and-filed. The no-email check should be an email regex, not a bare `@`, because book text can contain `@` |

---

## 7. Model tiers

- **Haiku on SB-B4 (toc_entry_finder) is wrong.** It is sized M and applies four change rules
  (C1, C2, C3, C5), including a semantic rewrite of the in-tool reject loop (C5) in the most
  load-bearing prompt in the pipeline. Make it **sonnet**, the same as SB-B2, which is the same
  kind of work.
- **Keep haiku on B1, B3, B5, B6, B7, B8 and B9**, which matches the user's "stick haiku agents
  on moving the agent definitions", but **only with G4 plus the check scripts in §6**.
  - Without a worked example and an exit-code check, haiku will produce plausible-looking
    fragments with wrong ids (G3).
  - B6 and B7 should also get the exact list of tool-call sites to rewrite. SB-01 can generate
    it with `grep -n 'load_page_image\|get_page_ocr\|grep_text\|get_heading_pages\|get_gap_context'`
    over each `system.tmpl`.
- **SB-02 can drop to haiku.** It is a transcription of §2.4 once the file names are enumerated.
- **SB-S8b** (classify, polish and the fail-closed rules) should be **opus**, like S6, because
  it carries the same "never certify a fallback" semantics.
- **BD-W1** (merge plus integration for a networked pack) should be **opus**, or be split like
  W2a/W2b.
- SB-D1 and BD-D1 are fine on sonnet. Haiku would also do if the notes are complete.
- Opus on X-00a, SB-C3, SB-S3, SB-S6, S7a, BD-F1a and W2b is justified. SB-01 stays opus,
  because it now carries the contracts from B1, B4, G3 and G4.

---

## 8. Revised task delta (apply to §6)

New tasks:

```
X-00a  spike: structured_book runtime (R1-3,R5,R7-9,R14-16)     opus   M  —
X-00b  spike: browser_download runtime (R4,R6)                   sonnet S  —
X-00c  spike: GLM vision probe (R10)                             haiku  S  —
X-01   amend DESIGN from SPIKE.md                                opus   S  X-00a,X-00b,X-00c
SB-G0  golden generator via go test -overlay (H/goldens/**)      sonnet M  X-01
```

Changed tasks:

- **SB-01:**
  - deps X-01;
  - adds the B1 day-one green pack, B4 `core/contract.rs`, the G3 stage table and the G4
    example plus check scripts;
  - adds `scripts/sync-core.sh` and the Makefile `sync-core` target (G6, G8);
  - commits `Cargo.lock`;
  - decides the full crate list in `Cargo.toml` (see A3).
- **BD-01:** deps X-01. Adds `http.rs`, the round helper and test harness, plus the resolver
  registry. Delivers a check-green placeholder pack.
- **SB-02:** haiku. Acceptance as in §6.
- **SB-C3:** add dep SB-C2.
- **SB-C4:** split into C4a and C4b, each with deps C1, C2 and C3.
- **SB-C5..C8:** add dep SB-G0. C7 also adds dep C3.
- **SB-S2:**
  - drops `metadata_prepare`;
  - adds the image-folder front-gate case (B5);
  - adds the truncated-chunk case (G11);
  - adds the foreign-run no-op case (G1).
- **SB-S3:** adds the sentinel and liveness cases (B5).
- **SB-S4:** owns `stage:metadata` (started by the front gate).
- **SB-S7:** split into S7a and S7b.
- **SB-S8:** split into S8a (sonnet) and S8b (opus).
- **SB-S9:** adds the oversized-chapter rule (G12).
- **SB-P1:** deps add C1, C4a and C4b. May edit its own `main.rs`.
- **SB-P2:** deps add C1. May edit its own `main.rs`.
- **SB-B4:** sonnet.
- **SB-B\*:** acceptance is the two check scripts exiting 0.
- **SB-W1:** deps add SB-01.
- **SB-W2:** split into W2a (sonnet) and W2b (opus, may edit any `P/**`). The Makefile
  test-target edit is removed.
- **SB-T1:** replaces placeholder tests only. Follows the G10 runtime mechanics.
- **SB-D1:** also adds both rows to `packs/README.md`.
- **BD-C2, BD-C3:** deps BD-01 only, once `http.rs` moves into BD-01.
- **BD-F1:** split into F1a (opus) and F1b (sonnet).
- **BD-W1:** opus.
- **X-E2E:** must/should split as in §6.

Rules section additions:
- Use one worktree per task (B2).
- Acceptance always includes "`gents pack test <pack>` passes in your worktree".
- Never commit `Cargo.lock` or `core/` copies.

---

## Appendix: smaller items

- **A1.** §2.2 says "event_sources: 9 model-stage sources + 20 pipeline sources". With B5
  (metadata moves to a StageStart path and the WorkItem hop is dropped) the counts change. Make
  SB-T1's `defs.json` assert the counts from the G3 table, not hard-coded numbers in prose.
- **A2.** The dispatch rule "Object with sha256 and resolver/source_id → from_fetch" must match
  the `sb-fetched` `input_fields`, which list `source_id` but no `resolver`. Fix the rule to
  `source_id` only, and test it in SB-01.
- **A3.** SB-01's "if you need a new crate, stop and report" will trigger on day one unless the
  crate list is decided upfront. The needs are:
  - `serde` and `serde_json` (everything);
  - `sha2` (S1, C5, F1b);
  - `regex` (C2, C4, P2's RE2-compatible search; Go `regexp` is RE2, and Rust `regex` matches it
    closely);
  - `unicode-segmentation`, or plain `chars()`, for rune offsets;
  - a zip/deflate and an XML reader for S10. Copy the ocr pack's choices from
    `ocr/plugins/ocr/Cargo.toml`, which already build for `wasm32-wasip1`.

  List them in SB-01's acceptance.
- **A4.** Retry feedback for toc_extract, classify and polish: §2.5 says that when Shelf has no
  hint text, "the prompt says what the validator rejected". That text must be written somewhere.
  Add "reject → feedback string" tables to S5 and S8b acceptance, so the B-task templates and
  S-task strings agree.
- **A5.** The digest's `book id` is now `bk_<sha[0..24]>`, where Shelf used a DefraDB doc id. The
  algorithm stays byte-for-byte, but **no existing Shelf citation will validate** against a
  re-processed book. That is correct given "migrating existing Shelf books" is out of scope, but
  the README task (SB-D1) should say it plainly.

---

## Round 1 re-review

Reviewed `DESIGN.md` revision 1 against gents `origin/main` at **`d4df8a02b`**
(fetched 2026-10-08; two commits past the design's `15c5452b8`), packs
`origin/master` at `cd00bb8`, and Shelf (clean tree, `go1.26.1`). The lens is
the same as before: are tasks atomic, isolated, ordered and checkable, are the
hand-off formats defined, and can the golden plan be built? "Verified" means I
read the code or ran it during this review.

**Verdict: still not ready to dispatch.** B2 to B5 are fixed in substance.
B1's mechanism fails as written, and that failure comes from my own round-0
fix. Seven new blocking items each fail `gents pack check`/`test`, fail
`make test-*`, or break a runtime guarantee. Each one has a small fix.

### Status of round-0 blocking items

| Id | Status | Notes |
| --- | --- | --- |
| B1 | **Not fixed: the mechanism fails** | See N1. A day-one pack with 9 slot behaviors and an empty `agent_behaviors` array fails `pack check`. My round-0 text said to do exactly that, and it was wrong. Also, `gents pack test` runs `check_dir` **before** it builds (`test.rs:44-52`), and `check_dir` reports "build the plugins ... with gents pack build" for a documents pack whose `.afb` is missing (`check.rs:80-88`). The `.afb` files are gitignored, so every fresh worktree fails the stated acceptance until it runs `gents pack build P` first |
| B2 | Fixed, with process gaps | One worktree per task. But nothing says *when* a worktree is cut. A worktree cut before its `depends_on` branches are merged builds without them. Nobody owns "the integrator" (W2a merges fragments, not branches). `H` is not a git repository, so X-00a, X-00b and X-00c all write `H/SPIKE.md` at once with no isolation (see W-1, W-2) |
| B3 | Fixed; buildable | **Verified:** `go test -overlay` adds a new `_test.go` to an `internal/` package and can call unexported functions (scratch module test). Shelf's `process_book/job` tests compile and run, and `git status` stays clean. Two caveats are in W-7 |
| B4 | Fixed, with contract gaps | `core/contract.rs` exists and the stage table is fixed. Four contract holes are in W-4 |
| B5 | Fixed | The front gate opens metadata and toc_finder. Image `expected = min(30,N)`. Openers are present (group validation accepts `{"expected_count":2}` with no timeout and `{"expected_count":{source_field},"timeout_secs","min_count":1}`). One liveness hole remains in the commit continuation (N7). The 1,800 s fallback cannot be built as written (W-6) |

### New blocking problems

#### N1. The day-one scaffold cannot pass `pack check`: every slot behavior must exist in `pack_config`

- **Verified.** `G/pack/inference.rs:306` (`validate_pack_inference_authoring`, called from
  `pack/loader.rs:31`, which runs inside `check.rs::load_config`) does
  `ensure!(declared.len() == config.agent_behaviors.len(), "must assign every behavior to exactly one inference slot")`.
- So a manifest with 9 slot behaviors plus `"agent_behaviors": []` fails the check, and
  BD-01 (2 behaviors) fails the same way.
- A behavior needs a `context_id` (see `ocr/pack_config.json`), so contexts are needed too.
- `core/mod.rs` and `stages/mod.rs` belong to SB-01, but §2.7 lists only the stage and tool
  modules. If the 13 C-task modules (`workspace`, `docs`, `text`, `furniture`, `evidence`,
  `evidence_text`, `search_link`, `search_front`, `canonical`, `prompts_front`,
  `prompts_finalize`, `prompts_structure`, `validate`) are declared without stub files,
  the build fails. If they are not declared, no C task may add them.

**Fix.**
- SB-01 and BD-01 ship one placeholder behavior and one context per slot behavior, with
  the final ids from the §2.5 table.
- SB-01 also ships a stub file for every `core/*` module.
- `merge.sh` **replaces by id** (`behavior_id`, `context_id`, ...) instead of
  concatenating. Otherwise W2a's merge creates duplicate ids.
- The acceptance everywhere becomes `gents pack build P && gents pack test P`, or simply
  `make -C K test-structured_book`, which builds first (`test-pack.sh:116`).

#### N2. Filtered event sources on foreign collections fail `gents pack check`

- **Verified.** `check.rs:228-258` (`check_filters`) runs every filter as
  `{ <Collection>(filter: ..., limit: 1) }` in a throwaway node. That node has only the
  runtime schemas and **this pack's** `schemas/` (`schema.rs::apply_pack_schemas_if_present`).
- `OcrChunk`, `OcrDocument` and `FetchedSource` do not exist there. So `sb-ocr-chunk`,
  `sb-ocr-doc` (`run_id: {_like: "sb-%"}`) and `sb-fetched` (`structure`/`status`) each
  report "event source ... filter is not valid on ...".
- The feasibility review's R1 answer held only because the sources were unfiltered then.
  The WL-G1 fix (mine) added filters.
- This fails W2a's acceptance and everything after it. R14 is moot: the check refuses
  the filter before any runtime question arises.

**Fix.**
- Leave all three foreign sources **unfiltered**.
- Filter in the plugin instead: `chunk_slot`, `chunk_text` and `from_fetch` return **no
  output** (their ports must be `optional`) when `run_id` does not start with `sb-`, or
  when `structure != true` or `status ∉ {ok, duplicate}`.
- The cost is one serial callback per foreign OCR chunk or document. Record it in the
  README.
- Add cases to S1 and S2.
- Rewrite R14 as "an unfiltered foreign source plus a no-output callback stores
  nothing".
- R1 must also be run the way `test-pack.sh` runs installs: structured_book **alone** in
  a fresh home, with no ocr and no browser_download (`install_documents`,
  `runtime_case`; `store_dependencies` only pre-stores declared dependencies).
  "Mandatory install order" does not help CI. If install refuses, then `install.json`
  and `runtime_pages.json` cannot pass at all, and the fallback must be the bridge-pack
  split for all three foreign sources.

#### N3. Several plugin entries sharing one `source` run every case under every entry; bound cases fail

- **Verified.** `test.rs:94-140` loops over `manifest.plugins`. For each entry it reads
  `<source>/tests/*.json` and runs every case against that entry's artifact, budget and
  `bind_dir`.
- With `book_pipeline`, `book_search` and `book_research` all on `plugins/book`, each
  case runs three times:
  - A `bind` case puts the bound path into **that entry's** `input_field` (`plugin.rs:491-525`).
    For `book_search` and `book_research` that is `book`; for `book_pipeline` it is
    `workspace` plus `workspace_original`. So every bound pipeline case fails under the
    two tool entries, and every bound search or research case fails under
    `book_pipeline`.
  - The tool entries' limits (60 s, `max_output_mib: 1`) also fail heavy pipeline cases,
    for example S9's "3 parts under 4 MiB each" and S2's "812-page chunk math".
- browser_download is worse. `download_resolve` declares no `bind_dir`, so every bound
  fetch or finalize case fails there with "does not declare bind_dir; it cannot be
  bound" (`plugin.rs:491-496`). The two entries also have different network grants, so
  one case is admitted under one entry and refused under the other.
- R18 ("two entries, one source, build and install") would pass and still miss this.
- Also, `build_plugin` stages and compiles the crate once per entry, so the build cost is
  three times. That costs time only.

**Fix.**
- structured_book gets two sources:
  - `plugins/book` for `book_pipeline`;
  - `plugins/book_tools` for `book_search` and `book_research`, with the same
    `input_field: "book"`, the same `access: read` and the same `limits`, so their cases
    behave identically.
- browser_download gets two sources: `plugins/download_resolve` and
  `plugins/download_fetch`.
- Shared code is copied by `scripts/sync-core.sh` (with `--check`), owned by SB-01 and
  BD-01. This makes the old fallback the primary layout. Note that `build.rs::mirror`
  cannot carry a symlinked directory: `file_type()` is not a directory, and
  `fs::read` of a directory fails.
- Update §0.9, §2.1, §2.7 and the target paths of P1 and P2, and drop R18.

#### N4. Canned `http_results` and `state` plugin cases are impossible under `gents pack test`

- **Verified.** `executor.rs::drive` opens a live `http_calls::Session` for any plugin
  with a network grant. `rounds.rs:100-106` then **strips `state` and `http_results`
  from the caller's input** ("only the host sets them") and serves the plugin's
  requests for real.
- So §3.5's "plugin cases with canned answers (`"http_calls": true, "http_results"`,
  `"state"`), no network" cannot be written.
- Every BD-C1/C2/C3 and BD-F1a case would call `api.openalex.org`, `archive.org` and
  other real hosts from CI. "Redirect chain", "server ignoring Range" and "60 s cutoff"
  cannot be staged against real hosts at all. The any-host grant admits only public
  HTTPS on port 443 (`http_calls.rs` module doc), so a loopback fixture server is
  refused too.

**Fix.**
- BD-01's `http.rs` becomes a **pure** `fn round(input: Value) -> Value` API: input
  carries `http_results` and `state`, output carries `http_calls` or a result.
- Every resolver and transfer test is a native `cargo test` that feeds canned rounds.
  `test-pack.sh:145-160` runs these once per Rust plugin.
- Plugin JSON cases are kept only for paths that never ask for HTTP: finalize on an
  agent part file, refusals before the first request, and terminal mode.
- Rewrite §3.5 and the acceptance of BD-C1..C3, F1a and F1b to match.

#### N5. `goal_token_budget` on the four agent Tasks fails `make test-structured_book`

- **Verified.** `test-pack.sh:207-208` is a built-in check for every documents pack:
  "no task sets a goal token budget". A failure reads "goal budgets are opt-in".
- §2.2, §2.5 ("Triggers") and D18 set it on the four agent Tasks, so SB-T1's acceptance
  fails.

**Fix.**
- Ship no `goal_token_budget`.
- D18 becomes "batch timeout is the cap. The README shows the operator how to opt in to
  a budget per Task after install".
- R20 becomes informational.
- `goal_objective_template` alone is not checked and can stay.

#### N6. Hard-linked sources make OCR fail at random on macOS at the pinned commit

- **Verified.** In `G/plugin/bound.rs`, `BoundDir::for_file` (the ocr pack binds
  `OcrChunk.path`, a file, through it) calls `pin(file)`. On macOS, `F_GETPATH` returns
  *any one* of a hard-linked file's names.
- Before `967339dfd` ("Accept a hard-linked file through its pinned folder", fixes
  gents #2379), `pin` refused such a file with "... now resolves to ...; the path
  changed after it was validated".
- §1 and §2.8 hard-link `sha256/<aa>/<hex>.pdf` into `runs/<id>/source.pdf`. That gives
  the file two names, which is exactly the #2379 case.
- `967339dfd` is **not** an ancestor of the design's target `15c5452b8` and not in
  `v0.20.0`. It reached main in `d4df8a02b`.
- Under the default `ocr_failure_policy: fail`, a random pin refusal fails the book.
  The operator runs darwin.

**Fix.**
- Move the target and floor to `origin/main ≥ 967339dfd` (today `d4df8a02b`).
- On the `v0.20.0` path, always **copy** the source (R8's fallback) instead of
  hard-linking.
- Add a hard-linked-source case to X-00a. Update the "Runtime target" table and
  operator decision 1.

#### N7. Commit continuation and unique `outcome_ref` can silently drop paragraphs

- §2.8 says `work:commit` writes "Chapter + Paragraph×k (≤400 paragraphs ...; `WorkItem{commit, part}`
  continues) + ItemOutcome". It does not say *which* part writes the chapter's
  `ItemOutcome`.
- If part 1 writes it, then:
  1. Part 2's `ItemOutcome` with the same `outcome_ref` fails part 2's whole transaction
     (F-19). Its Paragraphs and the part-3 continuation are lost after 3 attempts.
  2. If outcome refs are made per part instead, the batch overfills (`batch_size` was
     sized per chapter) and quiesces for good (F-18).
- In case 1, `commit_closed` sees "every chapter ok" and builds `canonical/book.json`
  and the digest from the workspace files. The `Paragraph` documents then silently
  disagree with the canonical file, and nothing can detect it, because a callback
  cannot query DefraDB.

**Fix.**
- One rule in §2.8: only the **final** part writes the chapter's `ItemOutcome`, and
  intermediate parts write only Chapter or Paragraph rows plus the next `WorkItem`.
- Part `p` re-derives its paragraph slice from `chapters/<id>.json`.
- SB-S9 adds a case for the 3-part chapter with exactly one `ItemOutcome`.
- The same rule applies to the R16 fallback `WorkItem{pages, cursor}`.

### Non-blocking (fix before dispatch; each is cheap)

| # | Problem | Fix |
| --- | --- | --- |
| W-1 | No rule for when a worktree is cut, and no integrator | Rule: "cut `sb/<id>` from `feat/structured-book-packs` only after every `depends_on` branch is merged into it". Name the orchestrator as integrator: it merges each finished branch and runs `make -C K test-structured_book` on the feature branch after each merge |
| W-2 | `H` is not in git; X-00a/b/c share `H/SPIKE.md` | Write `H/spike/{sb,bd,vision}.md`; X-01 concatenates them into `SPIKE.md` |
| W-3 | No task builds the pinned gents, but every acceptance needs `$G/target/release/gents` | X-00a builds it once in `G` worktree `../gents-pin`, records the path and `gents --version` in its spike file, and the rules point at that path |
| W-4 | Contract holes in `core/contract.rs` | (a) `TaskFile` has no rendered `user_prompt`, so the generic `stage:<any>_next` release in `gates.rs` (S3) cannot build later batches' `StageTask`s without calling C6-C8; add `user_prompt` and `prompt_bytes`. (b) `pattern.json` and `gaps.json` have no struct. (c) S10 also writes `ChapterFile`, `TocFiles` and `BookMetadata` and is not listed as a writer. (d) `core/schemas/*.json` is claimed by SB-G0 ("dumps Shelf's 9 JSON schemas", decision 6) and by SB-C9 (target paths): give it to G0 (`core/fixtures/schemas/`) and have C9 `include_str!` them, or the reverse, but not both |
| W-5 | Two fan-outs cross task boundaries with no dependency edge | `pattern_closed` (S7a) must release `discover` items, whose module is S7b. `stage:structure` (S8a) must release `classify`, which S8b owns. Add dispatch keys `stage:discover` (finalize_gaps.rs, S7b) and `stage:classify` (structure_llm.rs, S8b), started by a `StageStart` from S7a and S8a |
| W-6 | The "single-item stages get a 1,800 s batch timeout" fallback (§2.8, D14) cannot be built: there is one `sb-items` event source with one `timeout_secs` | If R17 says no, split `sb-items` into `sb-items-single` (filter `stage: {_in: [metadata,toc_finder,toc_extract,pattern]}`, 1,800 s) and `sb-items` (the rest, 14,400 s). Both are the pack's own collection, so the check passes |
| W-7 | SB-G0 caveats | `(*Job).findFinalizeGaps` calls `common.RefreshLinkedEntries`, which always loads through `svcctx.DefraClientFrom(ctx)` (`jobs/common/toc.go:538`, `:432`), so it cannot run without a DefraDB; port the loop and hand-derive its cases, or seed a Docker DefraDB via `testutil`. The digest lives inside `researchmcp/client.go:loadSnapshot`, which GETs the Shelf HTTP API; the golden must serve the snapshot from an `httptest.Server` (`NewClient(baseURL, ...)`). `generateEntriesToFind` works in memory (it uses the `SetLinkedEntries` cache, as `finalize_dedupe_test.go` does). G0's acceptance should list each target as pure, httptest or hand-derived |
| W-8 | `assets` takes no globs (`pack.rs::declared_paths`), so `tests/fixtures/<each fixture file>` must be literal. SB-T1 adds fixture files and the runtime run folder (a `"copy"` source must be a declared pack file), but may not edit the manifest | Let SB-T1 edit only `manifest.json` `assets` (append-only), or have SB-01 enumerate the fixture names now |
| W-9 | Two tasks own the zero-entry policy: S7b (`gap_closed`: require/fallback → failed, whole_book → `ch_001`) and S8a ("zero linked entries handled per policy") | Give it to S7b only; S8a reads `toc/final.json` and never branches on policy |
| W-10 | "Every StageTask sets every field ... to `"-"`" and `{% if doc.feedback %}` in the task template. Applied to `feedback`, the prompt always shows a `-` block | State that the `"-"` rule covers only the five fill-source fields, and that `feedback` is empty or absent |
| W-11 | The `sb-task-join` group has no timeout. Every successful StageTask leaves an open one-member group for good, about 2,000 per book | Add R23: does group recovery cost grow with open groups (one grouped binding per 5 s tick)? If it does, give the group `timeout_secs: 259200` with the default `min_count`, so stale one-member groups time out and `task_retry` returns no output for a lone StageTask |
| W-12 | §4 step 9 `--json '{run_id:"e2e-thurston", ...}'` is not JSON | Quote the keys; `test-pack.sh` passes `--json` straight to `document create` |
| W-13 | `cargo test` runs once per Rust plugin entry in `test-pack.sh` | Goes away with N3's split (one entry per source) |

### Task-list delta for round 2

- **SB-01:**
  - adds 9 placeholder behaviors and contexts, stubs for every `core/*` module,
    replace-by-id `merge.sh`, and `scripts/sync-core.sh` with `--check` (N1, N3);
  - its acceptance is `make -C K test-structured_book` (N1).
- **BD-01:** the same for its two behaviors and two sources; `http.rs` is the pure round
  API (N4).
- **SB-P1, SB-P2:** target `plugins/book_tools/source/tools/*.rs`; deps unchanged (N3).
- **SB-S1, SB-S2:** no-op cases for foreign or opt-out inputs (N2).
- **SB-S7a, S7b, S8a, S8b:** take the W-5 and W-9 keys and policy.
- **SB-S9:** the final-part `ItemOutcome` case (N7).
- **SB-W1:** foreign sources unfiltered (N2); the `sb-items` split only if R17 says no
  (W-6).
- **SB-T1:** may append to `assets` (W-8); no `goal_token_budget` (N5).
- **BD-C1..C3, BD-F1a:** `cargo test` with canned rounds, plus JSON cases only for
  paths with no network (N4).
- **X-00a:**
  - builds the pinned gents (W-3);
  - re-runs R1 as a fresh-home lone install (N2);
  - adds the hard-link case (N6) and R23 (W-11);
  - drops R14 and R18.
- **Runtime target:** `origin/main ≥ 967339dfd`; on the tag, copy the source and do not
  hard-link it (N6).

---

## Round 2 re-review

Reviewed `DESIGN.md` revision 2 against gents `origin/main` at **`d4df8a02b`**
(fetched again 2026-10-08; nothing new since round 1), tag `v0.20.0`, packs
`origin/master` at `cd00bb8` (`scripts/test-pack.sh`, `Makefile`), and Shelf.
The lens is the same: are tasks atomic, isolated, ordered and checkable, are
the hand-off formats defined, and can the golden plan be built? "Verified"
means I read the code at `d4df8a02b`. "Reproduced" means I also ran it with the
local `gents 0.20.0 (e6a25f9ec)` release build in a scratch home; nothing in
gents, packs or shelf was modified.

**Verdict: close, but not ready to dispatch.** Every round-0 item (B1 to B5b)
and every round-1 item (N1 to N7) is fixed in the design. Three new blocking
problems remain. Two of them come from round-1 fixes: the sync-copy layout
(N3) and the "acceptance = `make test` in your worktree" rule (N1). Under
either one, the integrator's own loop ("a red merge is reverted") reverts good
work, or a required test can never pass. All three have small fixes.

### Status of earlier blocking items

| Id | Status | Notes |
| --- | --- | --- |
| B1 (round 0) / N1 | **Fixed** | SB-01 ships 9 placeholder behaviors and contexts with the §2.5 ids, a stub for every `core/*` and stage module, and `merge.sh` merging by id. Acceptance is `make test-<pack>`, which builds first (`test-pack.sh:118-120`). **Verified:** `AgentContext.tools_id` is optional (`document_config/context.rs`), so contexts need no placeholder tools. One trivial residual: `check_readme` fails a README with no `"## "` (`check.rs:262-268`), so the SB-01 and BD-01 README skeletons must contain at least one `## ` heading |
| B2 (round 0) | **Fixed in the rules, broken in practice** | Worktree per task, cut after the deps merge, and the orchestrator as integrator are all specified. But the integrator's "run `make test` after each merge; revert a red merge" loop is defeated by R2-B1 (every C merge is red) and R2-B2 (the second run of the same tree is red) |
| B3 (round 0) | **Fixed; buildable** | Every target symbol exists in Shelf: `generateEntriesToFind`, `(*Job).findFinalizeGaps`, `appendLinkTocRetryHint`, `validateTocLinkEvidence`, `structuredRepairPrompt`/`maxStructuredRepairAttempts` (`providers/structured_output.go`), `ApplyEdits`, `(*Client).loadSnapshot`, `researchmcp.NewClient`, `SetLinkedEntries`, `BuildClassifyPrompt`, `BuildPolishPrompt`, `PrepareBookText`, `normalizeTocExtractEntries`, and the `(*TocEntryFinderTools).{AnalyzePageEvidence,ValidateCandidatePage}` methods (`ocr_evidence.go:42,206`). `ValidateCandidatePage` works on the in-memory `t.book`, so it is **pure**, but G0's pure/httptest/hand-derived list does not name it; add it. Ordering residuals are in W2-2 |
| B4 (round 0) | **Fixed, with a process gap** | §1.1 owns every file and payload. But `contract.rs` is read-only after SB-01, and nobody owns amendments. The `TaskFile` per-stage context ("target entry, page window, back-matter range") is not enumerated in DESIGN, so SB-01 has to invent 9 variants that ~20 later tasks compile against (W2-4) |
| B5a (image-folder front gate) | **Fixed** | `metadata_prepare` is gone. The front gate opens metadata and toc_finder. Images use `expected = min(30,N)` and PDFs count intersecting chunks (§2.8). S2 has the image case |
| B5b (liveness) | **Fixed** | Openers exist per batch and per stage, and single-item stages run as batches of 1. The finish opener at ingest (72 h watchdog) covers the groups that never get a member: no chunks after an OCR plan failure, a front gate that never opens, a `_next` handler that fails 3 times. The commit continuation (N7) and the R16 page continuation write the `ItemOutcome`/Signals only from the final part |
| N2 | Fixed | The three foreign sources are unfiltered, with no-output optional ports. The `sb-fire` filter is on `FireOutcome`, a runtime collection, so `check_filters` can run it (`check.rs:228-260`) |
| N3 | **Fixed, but it introduces R2-B1** | Four sources. **Verified:** `test-pack.sh:149-159` runs `cargo test --manifest-path <source>/Cargo.toml` natively and in place, not on the build mirror. So a drift `#[test]` that reads `../book/source/core` does work. That is exactly why it fails (R2-B1) |
| N4 | Fixed | Pure `round()`, canned rounds in `cargo test` |
| N5 | Fixed | No goal fields (`test-pack.sh:207-208`) |
| N6 | Fixed | `link_mode: copy` is the default, and hard link is opt-in on main |
| N7 | Fixed | Only the final part writes the chapter's `ItemOutcome` |

The emit_outcome chain also holds at `d4df8a02b`. `desired_state.rs:565-626`
refuses a delivered collection without `handoff_id: String`. Fire admission
reads `handoff_id` and an integer `attempt` from the fully hydrated source doc:
`event_source.rs:1077-1095` projects every schema field, and
`trigger_engine/mod.rs:599-643` builds the fire from it. Because every field is
projected, a null `feedback` is a defined `none`, and `{% if doc.feedback %}`
is safe under the engine's `UndefinedBehavior::Strict` (`template/mod.rs:54`).

### New blocking problems

#### R2-B1. The `core/` copy and its drift test turn every C-task merge red, and P1 cannot see its dependencies

- §2.7 and decision 9: `core/` is authored in `plugins/book/source/core/`. It
  is copied into `plugins/book_tools/source/core/` by `sync-core.sh`.
  `book_tools` carries a `#[test]` that compares the two byte for byte.
- The §6 rules say:
  - nobody edits the copied `core/` folder;
  - copies change only by running the sync script, owned by "SB-01, BD-01 and
    the merge tasks";
  - "commit only your target paths".
- The target paths of SB-C1..C9 and SB-G0 list only
  `plugins/book/source/core/...`.
- Consequences:
  1. Every C task changes an original, so `cargo test` of `book_tools` (run by
     `make test-structured_book`, once per entry) fails the drift test. Each C
     task fails its own acceptance in its worktree.
  2. If a C task runs the sync script but does not commit the copy (the rule),
     the merged feature branch is red, and the integrator reverts the merge.
  3. SB-G0 adds `core/fixtures/**`, so it trips the drift test the same way.
  4. **SB-P1 and SB-P2 compile against `plugins/book_tools/source/core/`.**
     That copy stays the SB-01 stub until "the merge tasks" (W2) re-sync. P1
     therefore cannot call C4a/C4b's grep, heading, page and `check_candidate`
     code, and P2 cannot call C5's canonical and digest code. Their
     `depends_on` edges are satisfied on paper and useless in the tree.

**Fix.**
- **Rule.** "A task that changes anything under `plugins/book/source/core/`
  runs `plugins/book_tools/sync-core.sh` before committing. It commits the
  mirrored paths `plugins/book_tools/source/core/<the same files>`."
- **Target paths.** Add those mirrored paths to C1..C9 and G0. Each task's
  mirror changes only its own file names, so merges stay file-disjoint.
- **Exception.** Add the mirrored paths to the "do not edit" exception list,
  next to SB-T1's `assets` exception.
- **Integrator.** The integrator also runs `sync-core.sh --check` after each
  merge.
- **Constraint.** State one constraint for C-task tests: tests inside `core/`
  may read files only under `core/` (for example `core/fixtures/`), because
  they also compile and run in the `book_tools` copy. A path like
  `../../tests/fixtures` does not exist there.

#### R2-B2. Bound plugin cases write into the committed fixtures in place, and their outputs carry the worktree's absolute path

- **Verified** in `GC/pack/test.rs:150-180`:
  - a case's `bind` becomes `BoundDir::new(case_dir.join(relative), Some(case_dir))`;
  - `BoundDir::new` is **always `ReadWrite`** (`plugin/bound.rs:68-93`);
  - the directory is the source tree itself, with no copy and no reset.
- **Verified** in `plugin.rs::call_bound` (`:485-540`): the canonical absolute
  path is written into `bind_dir.input_field`, and the original path into
  `original_field`.
- **Verified:** the case passes only on `PluginVerdict::Success` with an exact
  `outcome.output == expect` (`test.rs:169-176`). `PluginCase` is
  `deny_unknown_fields` with only `input`, `expect` and `bind`, so there is no
  setup step, no reset and no "expect failure" form.
- Consequences for `book_pipeline` (read_write, F-4) and `download_fetch`:
  - **The first run changes the fixture.** Ingest copies the source and writes
    `run.json`. Stages write `tasks/`, `results/` and `status.json` (O_EXCL).
    Finalize **deletes or moves the part file**.
  - **The second run of the same tree takes a different branch:**
    - the marker exists, so a different `writer_ref` emits nothing;
    - `closed: true` refuses the result;
    - the part file is gone.
    The integrator runs `make test` on the feature branch after *every* merge,
    so the second merge after any stateful case is red and gets reverted. The
    committed fixtures also show up as modified or deleted in `git status`.
  - **Outputs that carry `workspace`** (StageStart, Signal, StageTask,
    WorkItem, OcrJob.path, FetchedSource.path) embed
    `/…/wt/<id>/packs/gents/structured_book/plugins/book/tests/...`. An
    `expect` written in one worktree fails after the merge and on CI. Outputs
    with `created_at` (BookRun, BookStatus) cannot be matched exactly either.
  - **No target paths for fixtures.** S2..S9, P1, P2 and BD-F1b list only
    `tests/<module>-*.json`. Where their fixture folders live is unowned.
- So most of the S-task acceptance ("cases: X → Y") cannot be written as
  `gents pack test` cases.

**Fix.**
- **Stateful paths become native tests.** Every stateful stage path is a
  native `cargo test` that copies its fixture into a `tempfile::TempDir` and
  calls `run(key, &input, Some(&ws))`. Those tests run in place, once per
  entry, through `test-pack.sh:149-159`.
- **What stays as JSON plugin cases:**
  - unbound calls (dispatch, the foreign/opt-out no-output paths, `gate_open`,
    `batch_close`, `stage_close`, `task_failed`, `task_retry`, `from_fetch`,
    `chunk_slot`, `chunk_text`);
  - bound cases that write nothing: every `book_tools` op, and `chunk_join` and
    `page_assemble` on a folder with no `run.json`;
  - for those, an `expect` only when the output holds no path or clock.
- **Refusals.** "Refused" cases must be **success outputs**, for example
  `BookStatus{failed}`, `DownloadPlan{refused}` or `{"error": "..."}`. A plugin
  error can never be an expected result. This applies to "a non-op input is
  refused" (SB-P1) and "op refused by `plugins/book`" (SB-01), which go to
  `cargo test` or become a success-shaped refusal.
- **Clock.** Inject the clock into `created_at` (as BD-F1a already does) so
  native tests can pin it.
- **Fixture paths.** Add `plugins/<crate>/source/<module>/testdata/**` (or
  `tests/fixtures/<module>/**`) to every S, P and BD-F task that needs
  fixtures.
- **End to end.** Real bound end-to-end coverage stays in `runtime` cases
  (`runtime_pages.json`). Those copy the fixture into a fresh repository on
  every run (`test-pack.sh:386-417`), which is the right shape. SB-T1 may add
  more runtime cases, for example a seeded `StageStart` for a later stage.

#### R2-B3. browser_download cannot be installed until `web-research-mcp` is registered, and neither its pack test nor the e2e registers it

- **Verified:** `document_config/references.rs:394-400` requires a
  `ToolServiceRegistry` document for every `remote.services.mcp_service_id`.
  It runs over the whole candidate at publication.
- **Reproduced:** in a fresh home, `pack install` of a documents pack whose
  Tools doc references `web-research-mcp` fails with `Tools dl-f-tools field
  remote.services.mcp_service_id references missing ToolServiceRegistry
  "web-research-mcp"`. `pack check` does not catch it.
- `external_dependencies` does not register anything: install only reports it
  (`self_config/mod.rs:2431,2505`).
- **Verified** in `test-pack.sh`:
  - only `install_graph` calls `register_services` (`:315`);
  - `install_documents` (`:243-275`) and `runtime_case` do not;
  - web_deep_research is a **graph** pack, the only pack with
    `external_dependencies` today, so no documents pack has hit this.
- Consequences:
  - BD-T1's real `install.json` fails, and so does any later
    `make test-browser_download`. BD-W1 passes only while `install.json` is
    still the jq placeholder.
  - §4 has no step that registers the MCP service, so e2e step 6
    (`pack install browser_download`) fails. X-E2E cannot start.

**Fix.**
- **Harness.** Add a task (sonnet, S, deps X-01) that changes
  `K/scripts/test-pack.sh` so `install_documents` (and `runtime_case`) call
  `register_services "$home" || return 0` before installing, as
  `install_graph` already does. This is a shared-harness change, so it goes
  through packs review like any other.
  - Fallback if that change is refused: BD-T1 ships only jq cases and records
    the install gap in its README.
- **E2E.** Insert step 5b: register `web-research-mcp` (a `tool_service_registries`
  entry with the operator's host, MCP port and path, through `config export`
  and `config apply`, exactly as `register_services` does).
- **R22.** Add the same step to R22 on the tag.

### Non-blocking (fix before dispatch; each is cheap)

| # | Problem | Fix |
| --- | --- | --- |
| W2-1 | **S3 is missing dependencies.** §2.5 says a Task-failure retry puts its text inside `user_prompt` for link (`appendLinkTocRetryHint`, which is C7) and for toc_finder (`PreviousAttempt.Reasoning`, which is the C6 renderer). Both go through `stage:task_retry`, which lives in `gates.rs` (S3). But S3 depends only on C1, so it will either stop and report or re-implement the hint text | Add SB-C6 and SB-C7 to S3's `depends_on`. Or put a `retry_prompt(stage, &TaskFile, reason)` hook per stage module in SB-01's stubs, implemented by S5 and S6 |
| W2-2 | **SB-G0 is missing a dependency.** It depends only on X-01 but writes `P/plugins/book/source/core/fixtures/**`, so its worktree has no pack: `make test-structured_book` has no target, and there is no `sync-core.sh` | G0 depends on SB-01. The critical path (X-01 → SB-01 → G0 → C3 → C4a → P1) gets longer by G0 alone |
| W2-3 | **SB-01 is one L/opus serial gate for about 35 tasks.** It carries the manifest, the placeholders, 2 crates with about 30 stubs, `contract.rs` with every struct, payload, ref builder and budget, the dispatch test, sync and drift, 3 check scripts, `merge.sh`, `_sites.md` and the wasm crate decisions | Split it: **SB-01a**, the pack skeleton plus crates plus stubs, green (sonnet, M); **SB-01b**, `contract.rs` plus the dispatch test (opus, M, deps 01a); **SB-01c**, `.build/` scripts plus `_sites.md` (sonnet, S, deps 01a). B1..B9 then need only 01c, and C/S tasks need 01b |
| W2-4 | **`contract.rs` has no amendment path.** It is read-only after SB-01, the §1.1 `TaskFile` stage contexts are not enumerated, and "if you need a field, stop and report" has no receiver | Enumerate the 9 `TaskFile` variants' fields in §1.1 (from Shelf's work-unit structs). Then add a rule: a contract change is a separate `SB-K<n>` task owned by the integrator. It edits only `contract.rs` plus its mirror, and the requesting task rebases after it merges |
| W2-5 | **SB-W2a's acceptance cannot be met by W2a.** Fragments meet `gents pack check` for the first time at W2a, but W2a may edit only `pack_config.json` | W2a's acceptance becomes "merged by id, no placeholder left, `make test` output saved to `H/build-notes/structured_book/merge.log`", and W2b turns it green. Better: `check-fragment.sh` merges the one fragment into a temporary copy of the pack and runs `gents pack check` on it, so each B task and W1 meets the real check early. B1..B9 then depend on SB-02 as well |
| W2-6 | **Task prompts are capped at 1 MiB when rendered.** **Verified:** the Task prompt goes through `render_template`, which fails above `MAX_RENDERED_BYTES = 1 MiB` (`template/mod.rs:40,89`; `render_task` `:792-799`). So one oversized polish or classify `user_prompt` (a very long chapter) fails at fire admission on every attempt and fails the book with an unhelpful reason | C1's fan-out helper refuses an item whose `prompt_bytes` is above 1,000,000 (leaving room for the footer). It writes `ItemOutcome{failed: "prompt <n> bytes exceeds the 1 MiB task prompt cap"}` and adds a case |
| W2-7 | **There is no `--version` flag.** **Reproduced:** `gents --version` exits 2 ("unexpected argument"); the command is `gents version`. X-00a's acceptance and e2e step 1 use `--version`, so `run.sh` under `set -e` stops at step 1 | Use `gents version` |
| W2-8 | **Who creates the base branch is unowned.** `feat/structured-book-packs` has no creator, and `K` is checked out detached at `470c291`, not at `origin/master` `cd00bb8` | X-01 (or the orchestrator) creates the branch from `origin/master` `cd00bb8` and records the SHA. The integrator merges in its own worktree of that branch, never in `K` |
| W2-9 | **Some paths are relative to an unknown cwd.** The relative paths in `git -C K worktree add ../wt/<id>` and `make -C ../wt/<id>` resolve against different directories | Define `W=$SRC/github.com/gents-ai/wt` and use `$W/<id>` everywhere |
| W2-10 | **X-00a writes into `G`.** `git -C G worktree add ../gents-pin` writes `G/.git/worktrees`, and "read and build only" does not cover that. The tag build that X-00a/X-00b need for R22 has no recorded path either | `git clone --shared G <scratch>/gents-pin`, then check out `d4df8a02b`; do the same for `GENTS_TAG_BIN` at `v0.20.0`. Record both paths |
| W2-11 | **Golden input mapping is unowned.** G0's inputs are "the Go structs' JSON". Go structs without `json` tags marshal with Go field names, and Shelf's page inputs are not `PageFile`s, so each of C3, C5..C9, S7a, S7b and S10 would write its own Go→contract adapter | G0 documents one mapping per area in `H/goldens/README.md`, ideally emitting inputs already shaped as the `contract.rs` types. If it does not, the C task owns the adapter in its test module, and the work-list says so |
| W2-12 | **"`cargo test` once per Rust plugin source"** (§3.5) is not what the harness does: it runs once per **entry** (`test-pack.sh:150-159`), so `book_tools` runs twice | Wording only; it costs time |

### Task-list delta for round 3

- **New task:** `X-02  test-pack.sh: register external services for documents
  installs and runtime cases` (sonnet, S, deps X-01), with
  `K/scripts/test-pack.sh` as its only target. BD-T1 and X-E2E depend on it
  (R2-B3).
- **SB-C1..C9 and SB-G0:** target paths add the mirrored
  `plugins/book_tools/source/core/<same files>`, produced by `sync-core.sh`
  (R2-B1).
- **SB-G0:** deps add SB-01 (or SB-01a) (W2-2). Add `ValidateCandidatePage` to
  the "pure" list.
- **SB-S1..S10, SB-P1, SB-P2, BD-F1b:**
  - stateful cases become native `cargo test`s over temporary copies;
  - JSON cases cover only unbound or write-free paths;
  - refusals are success outputs;
  - fixture paths are added to the targets (R2-B2).
- **SB-S3:** deps add SB-C6 and SB-C7, or use the per-stage `retry_prompt`
  hook (W2-1).
- **SB-01:** split into 01a, 01b and 01c (W2-3). The README skeleton has a
  `## ` heading. The dispatch "op refused" check is a `cargo test`.
- **SB-W2a:** acceptance as in W2-5. `check-fragment.sh` runs `pack check` on
  a temporary single-fragment merge.
- **SB-C1:** add the 1 MiB prompt cap (W2-6).
- **X-00a, X-E2E:** use `gents version`; use a clone, not a `G` worktree; add
  e2e step 5b, the MCP registration (W2-7, W2-10, R2-B3).
- **Rules:**
  - `W` is an absolute worktree root;
  - X-01 creates the base branch;
  - each mirror path is an exception to "do not edit";
  - `contract.rs` changes go through `SB-K<n>` tasks (W2-4, W2-8, W2-9).
