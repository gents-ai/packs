# Review: does gents actually support what DESIGN.md relies on?

Lens: every runtime mechanism DESIGN.md depends on (plugin capabilities,
network, triggers, datastore surfaces, slot binding, pack dependencies),
checked against gents source. Read only; nothing in shelf, gents or packs was
changed.

Source baselines read:
- gents `e8774ade3` (the design's baseline, the local checkout, `main` of 2026-10-06)
- gents `origin/main` `15c5452b8` (2026-10-08, 209 commits later). Between the two, only
  `crates/gents/src/callback/` changed among the files that matter here (commits
  `1e38bdadc` and `c9d26b8d1`). `plugin/`, `pack.rs`, `document_config/`,
  `defra_write/` and `trigger_engine/event_delivery.rs` are identical, so every finding
  below applies to both unless it says otherwise.
- gents tag `v0.20.0` (`a5d02f106`) and the installed binary (`f44a0cef0`, older than the tag)
- packs `470c291`

Paths are relative to `crates/gents/src/` unless they start with `crates/` or `packs/`.

## Verdict

The design does not work as written. Eight problems stop it from installing or
running (B1 to B8 below). Each one has a contained fix and none needs a gents
change. Two of them (B1 and B2) break every model stage and every callback
binding in §2.6, so the fix has to land in SB-02 (schemas) and SB-W1 (wiring)
before any port task starts. X-00 should re-check each B item on the pinned
commit.

"gents v0.20" does not run either pack. Tag `v0.20.0` has plugin callbacks,
`bind_dir.access` and allowed folders, but it has no host HTTP:
`plugin/http_calls.rs` and `plugin/rounds.rs` do not exist at that tag
(`git cat-file -e v0.20.0:crates/gents/src/plugin/http_calls.rs` fails). So
browser_download needs an unreleased build, and the packs repo's own ocr pack
already needs `main` (guide §0). **Pin a gents commit explicitly. Make it
`origin/main` at or after `c9d26b8d1`, not `e8774ade3`** (see B8).

---

## Blocking

### B1. Every field named `*_key` is treated as a secret. Bindings that name one are refused, and the field is dropped from callback input.

- `toolset/shared/command.rs:813-819`: `is_secret_env_name` is true for any name
  that contains `KEY`, `SECRET`, `TOKEN` or `PASSWORD`, in any case.
- `callback/documents.rs:202-228`, `validate_callback_binding`: an `input_fields`
  entry that matches is an error ("source field `task_key` is secret-bearing").
  `callback/scan.rs` (`materialize_for_binding`) logs "callback binding invalid
  at scan" and returns `Ok(false)`. The binding never fires and nothing visible
  fails.
- `callback/documents.rs:294-305`, `strip_secret_fields`: the same test is
  applied recursively to every callback input before the plugin sees it
  (`callback/plugin.rs:184`, `callback/scan.rs` grouped path). That includes the
  members of a grouped array.

The design uses `task_key`, `item_key`, `batch_key`, `stage_key`, `gate_key`,
`chunk_key`, `cause_key`, `entry_key`, `gap_key` and `unique_key`. Every
binding in §2.6 passes "all" fields, so every one of the 20 bindings is
invalid. The §2.7 dispatch (`result:<stage>` keys on `task_key`) would never
see its key, and `gate_open` could not tell which gate it is closing.

**Fix:** rename every one of them to `*_ref` (`task_ref`, `item_ref`,
`batch_ref`, `stage_ref`, `gate_ref`, `chunk_ref`, `cause_ref`, `entry_ref`,
`gap_ref`, `dedupe_ref`) in §2.3, §2.4, §2.6, §2.7, the surfaces (§2.5) and the
C6 prompt change. C6 then becomes `entry_doc_id -> entry_ref`. This is a
wording change in two Shelf prompts, so record it. Also add a `defs.json` jq
assertion: no schema field and no `input_fields` entry matches
`/key|token|secret|password/i`. Check browser_download's schemas the same way.

### B2. A surface `fill` always writes a string, and a trigger source field must be a non-null string or integer.

- `defra_write/mod.rs:173-186`: a filled field's value is always
  `Value::String(fill.resolve(..))`.
- `defra_write/input.rs` `literal()`: an `Int` field given a string fails with
  "expected native JSON value matching `Int`; values are not coerced". So
  `attempt`, `batch_size` and `batches_total`, which §2.5 fills on all 9
  surfaces and §2.4 types as `Int`, make **every model write fail**.
- `trigger_engine/event_source.rs:56-79`, `captured_source_field`: a fill
  source field that is null, missing, boolean or a list is an error. The
  trigger context is then not built, and the Task does not run. One-shot
  StageTasks with `item_key`/`batch_key` left null (metadata, toc_finder,
  pattern) fail the same way.
- browser_download §3.2 fills `allow_unknown_license` (`Boolean`) and
  `formats` (`[String]`) into `SourceSearchResult` from `DownloadPlan`. Both are
  refused at fire time.

**Fix:**
- Fill only `run_id` (correlation), `task_ref` and `workspace`. Make the
  result handler read `attempt`, the batch fields and `item_ref` from the bound
  `tasks/<stage>/<item>.json`, which it can already read.
- If you keep the fills, type the copied fields `String` in the `*Result`
  collections. `ItemOutcome.batch_size` stays `Int`, because a plugin writes
  it.
- `StageTask` must set every filled field to a non-empty string, using `"-"`,
  never null.
- browser_download: replace the Boolean and list fills with one `String`
  `plan_json` that `dl-resolve-search` parses.

### B3. `bind_dir` fields must be declared string properties of `input_schema`.

`pack.rs:455-480`: `input_field` and `original_field` must each be string
properties of `input_schema`, or `pack check` fails. As designed:
- `book_pipeline` and `download_fetch` declare `{"type":"object"}`. Neither
  `workspace`/`workspace_original` nor `library_path`/`library_root` is
  declared.
- `book_search` and `book_research` omit `book_original`.

**Fix:**
- Declare `workspace` and `workspace_original` as `{"type":"string"}` in
  `book_pipeline`, and `library_path` and `library_root` in `download_fetch`.
- Drop `original_field` from the two read-only model tools, which never pass
  the path on. That also keeps a host-filled field out of the model's
  schema.

### B4. Declaring `limits` needs `--grant-authority`, for structured_book and for gents/ocr.

- `plugin/store.rs:184-203` (`grant_on_install`) bails with "asks for increased
  resource limits; install it with --grant-authority" whenever a declared limit
  exceeds the previously granted one (none on a first install).
- `plugin/authority.rs` `limits_consented` and `plugin/executor.rs:391-398` check
  the same thing again at admit.
- `bind_dir` alone needs no consent.

§2.1 says structured_book "installs without `--grant-authority`", and the
§4 e2e steps 5 and 7 omit the flag. Both fail, because `book_pipeline`,
`book_search` and `book_research` declare limits, and so does ocr
(`packs/gents/ocr/manifest.json:155-158`). The ocr README's install line is
wrong for the same reason. Packs CI hides the problem because
`scripts/test-pack.sh` always passes `--grant-authority`.

**Fix:** add `--grant-authority` to all three installs in §4 and the READMEs,
and correct the §2.1 sentence.

### B5. Callbacks run one at a time per agent, inline in the engine loop (answers R9).

- `callback/scan.rs:375-399`: `publish_invocation` awaits
  `run_owned_invocation` before the loop continues.
- `agent/runtime/startup.rs:698-716` spawns a single `run_callback_engine` per
  agent. Nothing in `callback/` spawns per invocation; the only spawn is
  `spawn_blocking` for the legacy WASM planner.

So every ocr extract (up to 900 s each), every `download_fetch` round (§3.4
allows 840 s), every page assembly and every stage callback runs serially
across all books and all three packs. While a call runs, the engine handles no
arrivals, no group recovery and no timeouts.

Consequences for the design:
- The 6 h `ocr` and `front` gate timeouts (§2.6) are measured from the first
  member's arrival (`event_delivery.rs:328-406`, `first_seen_at`).
  - A 40-chunk book whose extracts take about 9 minutes each overruns 6 h.
  - The gate then fires with `min_count: 1` and only some members. The rest
    arrive later and are dropped, because the group's idempotency key is used
    up (`documents.rs` `create_pending_invocation` returns the existing
    invocation).
- `Signal{link}` can fire with only the "toc" member present, so linking runs
  on a book whose OCR has not finished.

**Fix:**
1. Split gates into two kinds:
   - **Correctness gates** (link, finish, front): set
     `min_count = expected`. Do this with a fixed `expected_count` per
     source, or have `gate_open` refuse to start when a member is missing
     (emit `BookStatus{failed, reason:"gate <x> incomplete"}`). Never act on a
     partial set.
   - **Liveness closes** (batch, stage): keep partial timeouts.
2. Cap `download_fetch` at about 60 s per call. An 8 MB file still fits in one
   call, and the fetch stops starving OCR.
3. State the serial throughput in the README.
4. File a gents issue for concurrent callback execution.

### B6. A group that never receives a member never times out.

`event_delivery.rs:328-345`: `evaluate_group` returns `GroupOutcome::Empty`
before it creates the `EventGroupState`. The clock (`first_seen_at`) starts at
the first member. Timeouts are found only by `recover_group_page`, which pages
existing member documents.

Decision 5 says "Group `timeout_secs` makes a lost item close as `failed`".
That holds only while at least one item of the batch, and one batch of the
stage, produced an outcome. Losing every item of a batch is easy: B7's
QueueFull, a dead backend, a Task that ends without writing. When it happens
the batch never closes and the book hangs with no error.

**Fix:** the stage callback that fans out also writes:
- one **opener** member per batch, `ItemOutcome{status:"opened"}`, with
  `batch_size = items + 1`;
- one opener per stage, `BatchOutcome{counts:"opened"}`, with
  `batches_total + 1`.

That starts each clock at fan-out time. `batch_close` and `stage_close` ignore
openers when counting. Apply the same rule to any gate whose producers can
fail.

### B7. A duplicate member blocks a group forever, or fires it early. Models can write more than once.

- `event_delivery.rs:356-372`: more members than `expected` quiesces the group
  permanently.
- `event_delivery.rs:408-432`: members must agree on `expected`.
- A group fires as soon as `docs.len() == expected`, whoever the members are.
- `document_config/write_tool.rs:233-255` and
  `agent/output_obligation.rs:240-262`: `output_obligation` sets only a
  minimum. Nothing stops a second `write_*` call in the same run.

Example: two `TocFinderResult`s give two `result:toc_finder` runs, which give
two `Signal{link, member:"toc"}`. The link gate (expected 2) then fires without
OCR, or quiesces once the OCR signal arrives. The same happens to
`ItemOutcome` batches when a link agent writes twice.

**Fix:**
- Put `@index(unique: true)` on `task_ref` in every `*Result` collection
  (fill it per attempt, so `task_ref` includes the attempt). The second
  surface write then fails, and the model sees the error.
- Put a unique `signal_ref = gate_ref + ":" + member` on `Signal` and a
  unique `outcome_ref` on `ItemOutcome`. A duplicate handler then fails its
  whole transaction (`callback/plugin.rs` `commit_success`) instead of
  over-filling a group.
- `gate_open` also asserts that the member names are distinct.

### B8. The design's baseline commit drops callback documents created before the engine looks, and the e2e "wait for observing" step does not cover callbacks.

- At `e8774ade3`, per-document callback bindings mark every existing document
  "seen" when they first observe a collection (`callback/scan.rs` `seed_seen_docs`
  and in-memory `seen_docs`, capped by `SEEN_DOCS_SEED_LIMIT = 10_000`). A
  document written during startup or downtime never fires. This is gents
  #2343, fixed by `1e38bdadc` and `c9d26b8d1` on `origin/main` with durable
  arrival cursors.
- The "event source now observing" log line in §4 step 9 comes from the
  trigger engine only. `packs/scripts/test-pack.sh:399-402`: "The callback
  engine logs nothing when it picks up its bindings".
- On `origin/main`, a *materialization* error now stalls the binding's cursor:
  `deliver_binding_arrivals` returns on `?` and never checkpoints past that
  document. One example is an `input_fields` entry missing from a foreign
  collection. If `sb-fetched` names a `FetchedSource` field that
  browser_download does not have, every later `FetchedSource` waits behind it.

**Fix:**
- Pin to `origin/main` at or after `c9d26b8d1`.
- In e2e, create the seed with `gents document create DownloadJob --home H
  --graphql URL --json ...` (what `test-pack.sh:411-414` does; it says a served
  home admits writes only from its own principal). Retry with a fresh `run_id`
  until `DownloadPlan` appears, as the packs harness does.
- Add a `defs.json` check that every foreign `input_fields` entry exists in the
  producing pack's SDL.

---

## Non-blocking but wrong as stated

### N1. The real per-call limit is the 4 MiB stdout cap, not 128 documents.

`book_pipeline` declares `max_output_mib: 4`, which is the ceiling (guide §5.2).
A `stage:polish` StageTask carries a `user_prompt` of up to 120,000 characters
(SB-C8). Thirty such tasks from one call exceed 4 MiB. The call then fails
`BadOutput`, the stage callback fails, and with B6 the book hangs.

**Fix:** emit by size (at most about 3 MiB per call) as well as by count. When
a call runs out of room, it writes `WorkItem{stage:"<stage>_emit", cursor}` to
continue. The same applies to `commit` and to `page_assemble` on large chunks.

### N2. Backend queue overflow fails Tasks without retrying.

- `admission/controller.rs:270-310`: when the queue is full the call fails
  with `QueueFull`. Nothing retries it. A grep finds no retry path for
  `QueueFull`.
- `backend_registry.rs:15`: `DEFAULT_MAX_QUEUE_DEPTH = 100`.

The design fans out every batch at once (§2.6, "parallel"). A 300-entry book
makes 300 link Tasks on `book_agent`, which has one backend: 32 run, 100 queue
and about 170 fail at once. Each agent turn also re-enters admission.

**Fix:**
- Release batches one at a time: `batch_close` emits the next batch's
  StageTasks. That keeps at most 128 Tasks per stage in flight.
- In e2e, set `max_queue_depth` (for example 4096) on both backends with
  `gents config backend set`.
- Record in the README that the design has no per-book cap (Shelf's 8).

### N3. Plugin callbacks default to one attempt, and an interrupted call is never retried.

- `plugin.rs:993`: `attempts_allowed(None) = 1`. §2.6 sets no `max_attempts`.
- `workspace/journal.rs:62-78` and `callback/plugin.rs` `execute`: a call cut off
  by a restart is marked `Interrupted` and never run again.

**Fix:** set `max_attempts: 3` on every callback. Workspace writes must then be
idempotent: write to a temp file and rename (SB-C1). Interrupted invocations
should go through the R13 repair route, and the README should list them as
"stuck after restart".

### N4. Image folders make one OCR chunk per file, so the `Signal{ocr}` gate overflows at 257 files, not 1,000.

- `packs/.../ocr/source/plan.rs` `units()` returns `None` for images, so the
  file is read by cursor.
- `graph.rs` `plan` makes one chunk per directory entry, up to `MAX_CHUNKS =
  1000`.
- `expected = chunks_total` above 256 fails `canonical_positive_count`
  (`event_delivery.rs:408-432`), and the group quiesces.

R12's threshold of 1,000 is the wrong number.

**Fix:** either refuse image folders of more than 256 files in `ingest`, or
use the item → batch → stage close for OCR chunks as well (an outcome per
chunk, batched by 128).

### N5. The ChunkSlot/BookChunk join copies each chunk's OCR text three to four more times.

`OcrDocument.markdown` is copied into `ChunkSlot`, then into the stored grouped
invocation input (`callback/documents.rs` `create_pending_invocation` persists
`input`), then into `BookChunk`, then into `BookPage`.

The join exists only because `OcrDocument` has no `path`. The ocr plugin builds
`document` itself in `graph.rs` `extract` and has `chunk["path"]` there.

**Fix (later phase, in packs):** make a one-line ocr change that copies
`path` onto `OcrDocument`. Then bind `sb-page-assemble` directly on
`OcrDocument` and delete `ChunkSlot`, `BookChunk`, two callbacks and one
group. Until then the join works.

### N6. Group timeouts are noticed late.

`callback/scan.rs` `recover_group_page` visits **one** grouped binding per 5 s
tick, and reads one 256-document page each visit (`GROUP_RECOVERY_PAGE_SIZE`).
B5 also blocks the loop. The design has four grouped bindings. With tens of
thousands of `ItemOutcome` and `Signal` documents across books, a timed-out
group can be noticed tens of minutes after its deadline.

**Fix:** size the timeouts with this lag in mind. Fewer grouped collections
help; for example, `BatchOutcome` could become another stage of
`ItemOutcome` filtered by `doc_kind`.

### N7. A `JSON` scalar works through surface create, but refuses empty arrays (R7).

`defra_write/input.rs`: `parameters` and `literal` accept `JSON`, and
`reject_empty_json_arrays` refuses `[]` anywhere inside the value.
`write_text_edits` with no edits, and classify output with an empty list, would
both fail.

**Fix:** keep `*_json` fields as `String`, as the design already does. Do not
switch to `JSON`.

### N8. Three plugin entries can share one crate, so `sync-core` is not needed.

- `crates/gents-cli/src/commands/pack/check.rs:163-183` only asks that files
  sit under some plugin's `source`.
- `build.rs:247` mirrors each plugin into `target/plugins/<name>` separately.

So `book_pipeline`, `book_search` and `book_research` can all name `source:
"plugins/book"`, each with its own artifact, TOOL.md, `input_schema` and
`bind_dir`, and dispatch on input shape or `op`. That removes the `make
sync-core` target, the CI diff check and the risk of the copies drifting.
X-00 should confirm that a duplicate `source` builds three times.

### N9. Pack test details.

- `runtime_pages.json` needs `"repository": {"access": "read_write"}`
  (`test-pack.sh:382-383`). Otherwise the bound callback is denied.
- Fixture workspace files under `P/tests/` must be listed in `assets`, because
  check fails on undeclared files outside plugin sources (`check.rs:163-183`).
  Fixtures under `plugins/<p>/tests/` are exempt.

---

## The design's open questions R1–R10, answered from code

| # | Answer | Evidence |
| --- | --- | --- |
| R1 | `pack check` passes, because an unfiltered foreign source is never queried (`commands/pack/check.rs:227-260`). Runtime behaviour while the collection does not exist yet is not verified. Install ocr and browser_download first | |
| R2 | Yes. Outputs are refused only for runtime and protected collections, and pack ownership is not checked | `callback/plugin.rs` `writable_output_collection`, `validate_handler` |
| R3 | Yes. Only the port's own correlation field is checked against the host value | `callback/plugin.rs` `output_documents` |
| R4 | Yes. Nothing stops a callback from writing its source collection, and each new document is a new arrival | `callback/plugin.rs`, `callback/scan.rs` |
| R5 | Yes. The tool goes `PluginTool::call` → `call_data_bound` → `bind_input`, which is headless (`interactive` is false). An allowed `read_write` folder covers `read` (`Scope::granted` takes the max) | `plugin/tool.rs:70-80`, `plugin/executor.rs`, `plugin/allowed.rs` |
| R6 | Yes. Callbacks call `PluginExecutor::run` → `drive`, which serves `http_calls` rounds for the granted manifold | `plugin/executor.rs` `drive`, `callback/plugin.rs:204-207` |
| R7 | `JSON` is accepted, but `[]` is refused (N7). The surface itself sets no string cap | `defra_write/input.rs` |
| R8 | Not answerable from gents. It depends on Afterburner's WASI `path_link`. Keep the copy fallback | |
| R9 | **Serial**, one engine per agent (B5) | `callback/scan.rs:375-399`, `agent/runtime/startup.rs:698-716` |
| R10 | Not a gents question | |

## Checked and confirmed as the design states

- A grouped callback gets a JSON array of the members' `input_fields`
  (`callback/scan.rs` `materialize_group`).
- Grouped output correlation is `caused_by_correlation`
  (`callback/plugin.rs:196-203`).
- A grouped call is never bound: `bind_input` uses `input.get(field)` and
  returns `None` on an array (`plugin/executor.rs` `bind_input`).
- `read_write` with no `write_fields` always binds read-write
  (`pack.rs:273-284`).
- Only `graph` packs can be dependencies (`pack.rs:678-700`,
  `pack/installation/documents.rs:50-52`).
- The pack's own plugins pin with `digest: ""` (`pack/loader.rs:202-217`).
- Event groups have a 256-document cap (`runtime_snapshot.rs:125`) and accept
  `expected_count` as `{source_field}` (`document_config/event_trigger.rs`).
- Host HTTP: 1 MiB per response (a 1 MiB range passes, since the check is
  `>`), 16 MiB per call, 16 requests per round, no redirects followed, and a
  `User-Agent` header is allowed (`plugin/http_calls.rs:58-84,696`).
- `OutboundHttp: null` reaches any public HTTPS host.
- `pack build` mirrors plugin sources (`commands/pack/build.rs:247`).
- `pack check` skips dotfiles, so `.build/` works (`check.rs:196-200`).
- An allowed folder may not be, or contain, the gents home or `$HOME`
  (`plugin/allowed.rs` `ensure_scope_path`), so `$LIB` must be a specific
  folder outside both.

---

## Round 1 re-review

Reviewed `DESIGN.md` revision 1 against gents source. Read only; nothing in
shelf, gents or packs was changed (the only git action was `git fetch` in the
gents checkout).

Baselines:
- gents `origin/main` is now **`d4df8a02b`** (2026-10-08 10:47). It is two commits
  past the design's `15c5452b8`: `967339dfd` and merge #2382, which touch only
  `plugin/bound.rs` and its tests (see RB-5).
- Tag `v0.20.0` `a5d02f106`, for the fallback claims.
- Both trees were extracted with `git archive` into a scratch folder.
- Paths are relative to `crates/gents/src/` at `d4df8a02b` unless stated.
- Both trees pin the same afterburner rev, `ea11816`, so `Manifold` parses the
  same way on both.

### Verdict

Round 0's blocking items B1 to B8 are all fixed or properly replaced (table
below). Revision 1 adds a Task-failure retry path (decision 6, §2.6
`sb-fire`/`sb-task-join`) and a FetchedSource bridge. **Both break on runtime
rules the design does not account for.** There are five new blocking items,
RB-1 to RB-5:
- RB-2 stops `structured_book` installing at all.
- RB-1 stops every model Task from running.
- RB-3 breaks the browser_download to structured_book handoff that the e2e
  test depends on.
- RB-4 makes every Task-failure retry fail.
- RB-5 makes OCR of a hard-linked source fail at random on macOS at the
  design's pinned commit.

Each fix is a few lines in §2.3/§2.4/§2.6 and none needs a gents change.

### Round-0 blocking items: status

| Id | Status | Evidence on `d4df8a02b` |
| --- | --- | --- |
| B1 | **Fixed** | Every `*_key` is now `*_ref`, and no schema field in §2.4 matches `/key\|token\|secret\|password/i` (grepped). `validate_callback_binding` (`callback/documents.rs:205-231`) and `reject_secret_bearing_callback_fields` (`:233-252`) also check **filter** field names; decision 11 covers filters too. `fire_key` is kept out of `sb-task-failed`. `strip_secret_fields` (`:297`) would strip it anyway |
| B2 | **Fixed** | Surfaces fill only String fields. `defra_write/mod.rs:171-186` always writes `Value::String`. `trigger_engine/event_source.rs:56-79` refuses a null or missing source field, and the design's rule that `StageTask` uses `"-"` covers that. browser_download fills only `library_path` and `plan_json`. Residual, non-blocking: every `DownloadPlan` must carry a non-empty `library_path`, or the finder and agent fires are refused, so make `DownloadJob.library_path` required in `dl-resolve` |
| B3 | **Fixed** | `pack.rs:465-488` needs string properties; `book_pipeline` and `download_fetch` now declare both fields. Plugin input is never validated against `input_schema` (`plugin.rs:89`), so array inputs to grouped callbacks are fine |
| B4 | **Fixed** | `--grant-authority` on all three installs (§0.1, §2.1, §4 steps 5-7) |
| B5 | **Fixed (modified)** | The design's objection holds. In `trigger_engine/event_delivery.rs:477-496`, a group with `min_count == expected` that never fills is never eligible, so it hangs silently. The replacement, partial at timeout plus a bound completeness check, a `status.json` marker and the 72 h finish watchdog, fails visibly |
| B6 | **Fixed** | Batch and stage openers, plus the finish opener at ingest. A gate with no member (front, ocr, link) is caught by the finish watchdog |
| B7 | **Fixed** | Pack SDL accepts `@index(unique: true)` (`gents-cli/src/commands/pack/scaffold.rs:162`; used by `packs/gents/grok_tui_port/schemas/*.graphql`). A losing callback fails its whole transaction (`callback/plugin.rs::commit_success`) |
| B8 | **Fixed, but raise the floor** | `c9d26b8d1` is an ancestor of `origin/main` and not of `v0.20.0` (checked with `git merge-base`). The e2e seeds with `gents document create` (`gents-cli/src/commands/document.rs:99`). The floor must now be `d4df8a02b` (RB-5) |

### New blocking items

#### RB-1. `session_id_template` must name a session that **already exists**, so every model Task fire is refused.

§2.5 sets `session_id_template: "sb/{{ doc.task_ref }}"` on all 9 triggers, so
that `FireOutcome.session_id` carries `task_ref`. The fire path refuses this:
1. `trigger_engine/mod.rs:578-595` renders the template, and `:650-655` marks
   the fire `target_existing` whenever the trigger has a template.
2. `lifecycle/materialize.rs:1318-1334` loads the session row. With
   `continue_existing`, `durable::resolve_session_id(.., owned = session.is_some())`
   (`trigger_engine/durable.rs:20-30`) returns `None` for a missing session.
   The admission then fails with "Task target session is missing or belongs to
   another owner".
3. The self-config help says the same (`self_config/command/help.rs:236,353`):
   "session_id_template names an existing session … omit it for a new one per
   fire".

Nothing creates `sb/<run>/<stage>/<item>/<attempt>` sessions. Every StageTask
fire is therefore rejected, and no model stage ever runs. The batch timeout
then marks every item `failed: never reported`, and the book fails 4 h later
with a misleading reason. v0.20.0 has the same check
(`lifecycle/materialize.rs:1332` at the tag).

**Fix:**
- Delete `session_id_template` from all triggers. A fire without one gets a
  fresh session derived from the fire identity (trigger plus StageTask
  `_docID`, `mod.rs:592-594`), which is exactly what Shelf's "fresh agent per
  retry" needs.
- Recover `task_ref` through `handoff_id` instead (RB-2).
- Correct F-7 and the FID-F1 row in the revision log.

#### RB-2. `emit_outcome: true` needs a non-empty `String handoff_id` on the trigger's source collection; `StageTask` has none, so install fails.

- At install: `config_client/desired_state.rs:574-626`
  (`validate_outcome_source_fields`, called from `apply_desired_state_plan`,
  `:377` and `:1380`, which pack install uses at `pack/installation.rs:478`)
  refuses "Trigger … delivers StageTask to Task …, which sets emit_outcome, but
  StageTask has no handoff_id field". So `gents pack install structured_book`
  fails with all 9 Tasks at `emit_outcome: true`.
- At fire time: `trigger_engine/mod.rs:636-643` refuses each fire whose
  document has an empty `handoff_id` ("emit_outcome requires a source
  handoff_id").
- The same rules are in v0.20.0 (`desired_state.rs:518`, `trigger_engine/mod.rs:636`).

**Fix (this also gives a correct replacement for RB-1's join):**
- Add `handoff_id: String` to `StageTask`. Every StageTask writer
  (`stage:<x>`, `<x>_next`, `result:<x>` retries, `task_retry`) sets
  `handoff_id = task_ref`.
- `FireOutcome.source_handoff_id` is copied from it
  (`trigger_engine/mod.rs:625`, `durable.rs:197-211`; field in
  `gents-schemas/.../fire_outcome.graphql`).
- `sb-fire`:
  - Set event source `correlation_field: "source_handoff_id"`, and handler
    `correlation_field: "source_handoff_id"`.
  - Set the `TaskFailed` output port to `correlation_field: "task_ref"`. The
    host then writes the failed Task's `task_ref` into the marker.
  - Set `input_fields: [source_handoff_id, trigger_id, terminal_state, reason, attempt]`.
  - The plugin derives `run_id` from `task_ref`.
  - Today the design gives `sb-fire` no correlation ("—"). `validate_handler`
    (`callback/plugin.rs:69-73`) refuses outputs without a source correlation,
    so this is needed as well.
- `defs.json`: assert that `StageTask` declares `handoff_id: String`.

#### RB-3. The host owns each output port's correlation field, so `sb-from-fetch` cannot write `BookJob.run_id = "sb-" + run_id`.

`callback/plugin.rs:132-147` (`output_documents`) handles a plugin-supplied
value in the port's `correlation_field` like this:
- If it differs from the source correlation, the call fails with "the plugin
  changed "run_id", which the runtime writes".
- Otherwise the source correlation is inserted.

The §2.6 `sb-fetched` binding has correlation `run_id` and a `BookJob` output.
So either:
- the call fails three times (`max_attempts: 3`) and no BookJob is ever made, or
- the BookJob gets the bare FetchedSource `run_id`, which `ingest` then
  refuses for lacking the `sb-` prefix (§2.3, D19).

Either way, browser_download to structured_book never starts a book, and §4's
must-criteria fail.

**Fix:** declare the `BookJob` port with `correlation_field: "fetch_run_ref"`.
Add `fetch_run_ref: String` to `BookJob` and let the plugin set
`run_id = "sb-" + run_id` itself. Only the port's own correlation field is
host-owned. The same rule constrains every callback whose output must carry a
different `run_id` from its source. Today that is only this one, so add a
`defs.json` assertion that names it.

#### RB-4. `task_retry` writes into `cause_ref`, which `StageTask` and `ItemOutcome` do not declare.

- §2.6 gives `sb-task-join` → `sb-task-retry` the outputs "StageTask one … or
  ItemOutcome one; corr cause_ref". The host inserts the group key into the
  port's correlation field (`callback/plugin.rs:144-147`).
- The §2.4 `StageTask` and `ItemOutcome` have no `cause_ref`. The create
  mutation then names an unknown field and the whole transaction fails.
- `validate_handler` does not check port fields against the SDL
  (`callback/plugin.rs:45-76`), so `pack check` does not catch it.
- Result: every Task-failure retry fails three times. The item is only ever
  closed by the batch timeout, so D14's fallback becomes the only behaviour
  without anyone choosing it.

`batch_close` (BatchOutcome, StageStart), `stage_close` and `gate_open`
(StageStart) and `chunk_join` (BookChunk) are fine, because those collections
declare `cause_ref`.

**Fix:** add `cause_ref: String` to `StageTask` and `ItemOutcome`. Add a
`defs.json` assertion: for every callback output port, the port's
`correlation_field` is a field of the port's collection in the producing SDL.

#### RB-5. Pinning at `15c5452b8` makes binding a hard-linked source file fail at random on macOS.

- §1 and S1 hard-link the input into `runs/<run_id>/source.<ext>`. When it
  came from browser_download, the file also lives at
  `sha256/<aa>/<hex>.<ext>`, so it has two names.
- `gents/ocr` binds `OcrJob.path` (a file) through `BoundDir::for_file`, which
  calls `pin()` (`plugin/bound.rs:133`).
- At `15c5452b8`, `pin()` requires that the open handle's `F_GETPATH` equals the
  validated path. macOS reports **any one** of a hard-linked file's names, so
  the check fails at random with "… now resolves to …; the path changed after
  it was validated".
- `967339dfd` (#2382, now on `origin/main`) adds `named_in_pinned_folder` to
  accept that case (`plugin/bound.rs:288-340`). The operator runs macOS, and
  this decides whether a book's OCR runs at all.

**Fix:**
- Set the runtime floor to **`d4df8a02b`** in "Runtime target", in Decision 1
  and in §4 step 1.
- On `v0.20.0` (no fix), ingest must use the copy fallback, never a hard link.
  Change R8's fallback from "if `path_link` is missing" to "always on the tag
  and on any build before `d4df8a02b`".

### New non-blocking findings

- **NB-1. Goal mode without goal tools (R20).** A
  `goal_objective_template` starts a durable Goal that runs until
  `update_goal` completes it. The task field help
  (`self_config/command.rs:2569`) says it "requires
  built_ins.enable_goal_tools on the behavior". The §2.5 tools docs enable only
  `enable_session_history_tool`. The likely outcome is that an agent that wrote
  a valid result keeps running to `goal_token_budget`, and its FireOutcome
  comes back non-success. `task_retry` then re-issues the item (new
  `spawn_ref`, so the write succeeds) until the budget is spent. Duplicates
  are then refused by `outcome_ref`, but the model calls are wasted three times
  over.
  - Either set `enable_goal_tools: true` and add one sentence telling the
    agent to call `update_goal` after `write_*`, recorded as prompt change C9,
  - or ship without goal templates until X-00a shows how a goal behaves on a
    trigger-fired Task.
- **NB-2. Agent fetch on the tag: the host root has to be interpolated.** A
  pack cannot know `$LIB`. Use `"root": "${GENTS_DOWNLOAD_LIBRARY}"`, which is
  supported on both trees (`pack/interpolate.rs`; packs `repo_maintenance` and
  `security_scan` use `${GENTS_*_ROOT:-.}`), and document the variable in the
  README.
- **NB-3. The agent fetch argv prefix limits only the head of the command.**
  `first_matching_prefix` (`toolset/shared/command.rs:654-662`) matches a
  prefix, so arguments after `-o` are free:
  - `-T <file>` or `-d @<file>` uploads any readable file to any HTTPS host;
  - `-K <file>` and a second URL are also possible.

  Decision 2 should say this. It is an exfiltration path, not only "outside
  the WASM sandbox". If the operator accepts it, the agent's `execution_mode`
  and `root` are the only containment.
- **NB-4. Late results after a batch timeout.** A result that arrives after
  its batch group fired at timeout still runs `result:<stage>`. Its
  `ItemOutcome` is dropped by the group (the group's idempotency key is used
  up), but its side outputs (`TocLink`, `GapFix`, `TocEntry` revisions,
  `results/` files) are written after `_closed` has moved on. Have each result
  handler refuse when `tasks/<stage>/<item>.json` is already marked closed by
  `_closed` (set that marker in `_closed`).
- **NB-5. `sb-task-join` leaves one never-filled group for every successful
  StageTask.** That is harmless with no timeout, but the `EventGroupState` rows
  grow without bound. If X-00a shows recovery paging them, add
  `timeout_secs` (e.g. 86400) with `min_count: 2`. A group that never fills is
  then dropped rather than fired, by `group_candidate_eligible`.
- **NB-6. R17 is still open.** No code was found that stops a callback
  binding from sourcing `FireOutcome`, but nothing confirms that the callback
  engine's arrival cursor sees that runtime collection. Keep it the first
  check in X-00a. Once RB-1 and RB-2 are applied, the join no longer depends
  on `session_id`.
- **NB-7. R21 is likely "parallel engines".** Each documents pack has its
  own `agent_principal`, and `run_callback_engine` is per agent. So OCR
  extracts (the ocr agent) probably do not block structured_book callbacks.
  §2.8's "Throughput" text should say "per pack" until X-00a confirms it.

### Checked and confirmed in revision 1

- `expected_count: 2` without a timeout is valid; `min_count` needs a timeout;
  the 256 cap applies (`document_config/event_trigger.rs:286-372`).
- An empty nullable `[String]` from a surface becomes null, and only `JSON`
  scalars refuse `[]` (`defra_write/input.rs:104-148`).
- Task templates are MiniJinja, so `{% if doc.feedback %}` works
  (`template/mod.rs`).
- A missing or null bound field runs the call unbound (`plugin/executor.rs:220-222`).
  So one `book_pipeline` entry can serve both bound and unbound callbacks.
- The `OutboundHttp` host list in `download_resolve` parses
  (`plugin/http_calls.rs:124-174`). `OutboundHttp: null` is any public host.
- The ocr plugin writes `OcrChunk.path = path_original` (the job path), so
  `workspace = dirname(OcrChunk.path)` holds for both `source.<ext>` and
  `source/` (packs `ocr/source/graph.rs:56-86`).
- `gents init --max-queue-depth`, `gents plugin dirs` and
  `gents document create` exist on main. `plugin dirs` also exists on the tag.

---

## Round 2 re-review

Reviewed `DESIGN.md` revision 2 against gents source. Read only; nothing in
shelf, gents or packs was changed (the only git actions were `git fetch` and
`git archive` into a scratch folder).

Baselines:
- gents `origin/main` is still **`d4df8a02b`** (`git ls-remote` on 2026-10-08
  shows the same head), so the design's target and floor are current.
- Tag `v0.20.0` `a5d02f106`, for the fallback claims.
- packs `origin/master` `cd00bb8`, for `gents/ocr`.
- Paths are relative to `crates/gents/src/` at `d4df8a02b` unless stated.

### Verdict

Every earlier blocking item (B1 to B8 and RB-1 to RB-5) is fixed, and the
code confirms each fix (tables below). Revision 2 still has **three new
blocking problems**. None needs a gents change:
- R2-1: canonical promotion writes outside the folder the callback is bound
  to. It always fails, so `validate_quote`, reuse and D23 never work, and
  three of the §4 must-criteria fail.
- R2-2: the §2.7 dispatch contract returns port-keyed objects. gents writes a
  single-port callback's whole output as the document, so 8 structured_book
  callbacks (every gate, join and retry) and 3 browser_download callbacks
  fail every invocation.
- R2-3: browser_download's `serial` triggers permanently drop any job that
  arrives while an earlier one is running.

### Round-0 blocking items: status on `d4df8a02b`

| Id | Status | Evidence |
| --- | --- | --- |
| B1 | **Fixed** | No schema field, `input_fields` entry or filter field in §2.4, §2.6 or §3.2 contains `key`/`token`/`secret`/`password`; the only matches are prose, `fire_key` (excluded, §2.6) and `goal_token_budget` (not used). `validate_callback_binding` checks input fields (`callback/documents.rs:203-230`). `reject_secret_bearing_callback_fields` checks filters, and the scan path calls it again (`callback/scan.rs:492-496`). `strip_secret_fields` (`:297`) would drop any field that slipped through. |
| B2 | **Fixed** | Surfaces fill only String fields (§2.5, §3.2). `StageTask` sets all five fill sources and `handoff_id` non-empty (§2.4). browser_download fills only `library_path` and `plan_json`, and `DownloadJob.library_path` is now required (§3.2). |
| B3 | **Fixed** | `book_pipeline` declares `workspace` and `workspace_original`; `download_fetch` declares `library_path` and `library_root`; the read-only tools have no `original_field` (§2.1, §3.1). This matches `pack.rs:465-488`. |
| B4 | **Fixed** | `--grant-authority` is on all three installs (§0.1, §2.1, §4 steps 5-7). The ceilings hold: 2048/900/4 and 512/90/1 are within `MAX_DECLARED_MEMORY_MIB = 4096`, `MAX_DECLARED_WALL_CLOCK_SECS = 900` and `MAX_DECLARED_OUTPUT_MIB = 4` (`plugin.rs:122-135`, `pack.rs:519-532`). |
| B5 | **Fixed (modified)** | Execution is still serial: `publish_invocation` awaits `run_owned_invocation` (`callback/scan.rs:589-610`). The replacement (partial at timeout, then a bound completeness check, plus the finish watchdog) fails visibly. The settings are valid: `min_count` with a timeout, `min_count <= expected` (`document_config/event_trigger.rs:326-345`). |
| B6 | **Fixed** | Batch, stage and finish openers. |
| B7 | **Fixed** | Unique refs on every grouped member collection. A losing callback fails its whole transaction (`callback/plugin.rs::commit_success`). |
| B8 | **Fixed** | The floor `d4df8a02b` contains `c9d26b8d1`. The e2e seeds with `gents document create` and retries. |

### Round-1 blocking items: status on `d4df8a02b`

| Id | Status | Evidence |
| --- | --- | --- |
| RB-1 | **Fixed** | No trigger sets `session_id_template`. Without one, `session_id = graph_session_id.unwrap_or(identity.session_id())`, and `target_existing` is false unless a graph session or a template exists (`trigger_engine/mod.rs:578-595, 650-655`). Each StageTask doc therefore gets a fresh session. |
| RB-2 | **Fixed** | `StageTask.handoff_id: String` satisfies `validate_outcome_source_fields`, which checks `named_type() == "String"` (`config_client/desired_state.rs:574-626`). The fire copies it with `source_string("handoff_id")` and refuses an empty value (`trigger_engine/mod.rs:625-643`). `durable.rs:197-215` writes `FireOutcome.source_handoff_id` and `attempt` (`fire.attempt` is the source doc's `attempt` read as `as_i64`, `mod.rs:629-634`). The FireOutcome SDL has both fields (`gents-schemas/schemas/agent/fire_outcome.graphql`). `sb-fire` correlates on `source_handoff_id` and has handler correlation, so `validate_handler` passes (`callback/plugin.rs:45-76`). Nothing stops a callback from sourcing FireOutcome. The only FireOutcome source rule is for Triggers whose Task emits outcomes (`config_client/event_source_cursor.rs:198-212`, `document_config/references.rs:528-537`). The runtime half is still R17 |
| RB-3 | **Fixed** | `output_documents` writes the correlation into the port's `correlation_field` only, and refuses a different plugin value only there (`callback/plugin.rs:84-150`). With the port on `fetch_run_ref`, the plugin owns `run_id`. Note: for per-document bindings the value written is the **event source's** correlation (`caused_by_correlation`, set from `Delivery::correlation_field` = `source.correlation_field`, `trigger_engine/event_delivery.rs:57-61`, `callback/scan.rs:519-522`). The handler's `correlation_field` is only a fallback (`callback/plugin.rs:203-209`). The design sets both to the same value everywhere, which is correct. `defs.json` should assert they are equal |
| RB-4 | **Fixed** | `StageTask` and `ItemOutcome` now declare `cause_ref`, and so does every other grouped-port collection (`BatchOutcome`, `StageStart`, `BookChunk`; §2.4). |
| RB-5 | **Fixed** | The floor is `d4df8a02b`, ingest copies by default, and `hard_link` is opt-in. Note that `BoundDir::for_file` itself hard-links the bound file into a private `.gents-bind` folder (`plugin/bound.rs:12-18`), so even a copied source has two names while ocr runs. That makes the floor necessary for **every** file-bound OCR call on macOS, not only for `link_mode: hard_link`. The design's floor already covers this; on the tag the copy fallback does not avoid it. Add this to R22 (see R2-N4) |

### New blocking items

#### R2-1. A callback bound to `runs/<run_id>` cannot write `books/<sha>/canonical/` or `books/<sha>/latest_run.json`.

- `BoundDir` admits **one** directory per call, and WASI preopens only that
  directory (`plugin/bound.rs:1-18`, "One directory, or one file alone,
  admitted for one plugin call").
- `bind_input` binds exactly the path in the input field
  (`plugin/executor.rs:209-268`). An allowed parent folder grants access, but
  it does not widen the preopen.
- §1 says "`workspace` on every document is the absolute run folder" (except
  `BookJob`). `stage:commit_closed` runs in `sb-stage`, bound on
  `StageStart.workspace`, which is the run folder. It must "promote" to
  `books/<sha>/canonical/book.json` and `canonical/<digest-hex>.json`, both
  outside the bound folder.
- A failed run's `terminal()` must also write `books/<sha>/latest_run.json`
  (D23) from a run-bound handler.

Consequences:
- Every promote fails three times and the commit stage never finishes.
- `book_research` (bound on `books/<sha>`) never finds a canonical file.
- `ingest`'s reuse check never sees one.
- §4 must-criteria fail: the digest equality, `validate_quote`, and
  `BookRun{reused}`.

**Fix (pick one):**
- Make `commit_closed` emit `StageStart{stage:"promote", workspace:"<library>/books/<sha>"}`.
  Its bound handler reads `runs/<run_id>/canonical/book.json` and renames it
  into `canonical/` (same preopen, so an atomic rename is possible). It then
  writes `BookStructure` and the finish Signal. D23's `latest_run.json` goes
  through the same book-bound step, or into `stage:finish`.
- Or bind `stage:finish` on the book folder (the `gate_open` output sets
  `workspace`) and do all book-level writes there.
- Either way, add a rule to §1: every write outside `runs/<run_id>/` has a
  named book-bound handler. Add an SB-S9 case that runs under a real
  `bind_dir` (a `pack test` plugin case with `bind`), not only `cargo test`.

#### R2-2. A single-port callback's output **is** the document, so the §2.7 "object keyed by port name" contract writes a bogus field and fails.

- `output_documents` (`callback/plugin.rs:84-95`) does this:
  `[only] => vec![(only, Some(output))]`. With one port, the plugin's whole
  value is the document (or array). Only with several ports is it "an object
  keyed by output name". The guide says the same (`gents-pack-guide.md:565-567`).
- §2.7 makes every module return "an object keyed by the port-name consts".
- These callbacks have exactly one port:
  - structured_book: `sb-from-fetch` (BookJob), `sb-chunk-slot`,
    `sb-chunk-text` (ChunkSlot), `sb-chunk-join` (BookChunk), `sb-gate-open`,
    `sb-stage-close`, `sb-task-retry` (StageStart), `sb-task-failed`
    (StageTask).
  - browser_download: `dl-resolve`/`dl-resolve-search` (DownloadPlan),
    `dl-terminal` (FetchedSource), and `dl-finalize` (FetchedSource) unless it
    gets a second port.
- For each of them, `{"book_chunk": {...}}` becomes a BookChunk create with an
  unknown field `book_chunk`. The transaction fails on every attempt.
- So no gate opens, no batch or stage closes, no Task failure is retried,
  and no FetchedSource reaches structured_book.
- A related trap: "no output" on a single optional port must be JSON `null`
  (`(_, None | Some(Value::Null)) => Vec::new()`, `:110`). `{}` is written as
  an empty document that carries only the correlation. For `sb-task-failed`,
  that empty doc joins the original StageTask, fills the 2-member group, and
  starts a retry for a Task that succeeded.

**Fix:**
- `core/contract.rs` lists the port set per callback id.
- `main.rs` unwraps the value when the dispatch key's callback has one port,
  and maps "no output" to `null` (single port) or omits the key (several
  ports).
- SB-01 adds a dispatch test for both shapes; BD-01 does the same.
- `defs.json` asserts that the contract's port sets match the
  `pack_config.json` outputs.
- Alternative: give every callback a second optional port (for example
  `book_status`), so the keyed form is always right. That is simpler to keep
  correct.

#### R2-3. `concurrency: "serial"` on `dl-source-finder` and `dl-fetch-agent` drops overlapping jobs for good.

- `trigger_engine/mod.rs:836-850`: under `Serial`, a fire while another
  request of the trigger is active returns `Skipped { SERIAL_BUSY }`.
- `trigger_engine/event_source/durable.rs:143-170` **acknowledges** that
  skip and checkpoints past the arrival with `legacy_serial_busy = true`.
  `config_client/event_source_cursor.rs:351-381` then *excludes* that
  arrival.
- The help text says the same: "serial skips while busy;
  queued_serial runs in order" (`self_config/command/help.rs:353`). The tag
  has the same code (`event_source/durable.rs:145-166` at `v0.20.0`).

Consequences:
- A second `DownloadPlan{needs_search}` (or `{ready, agent}`) that lands
  while the first finder runs never fires.
- No `SourceSearchResult`, `AgentFetchResult` or `FetchedSource` is ever
  written for it, and nothing reports it.
- §4 step 9's "retry with a fresh `run_id`" creates exactly this overlap. The
  Thurston job resolves to `ready` through IA and so escapes it, but any
  search-path or agent-mode job does not.

**Fix:** use `"queued_serial"` on both triggers. It exists on main and on
the tag (`document_config/trigger.rs:89` at the tag), and it keeps the
politeness reason for serial execution. `defs.json` should assert that no
trigger in either pack uses `serial`.

### New non-blocking findings

- **R2-N1. `input_fields` are exact, and the design never lists them.**
  - `fetch_source_doc` (`callback/scan.rs:846-906`) and `materialize_group`
    (`:644-668`) project exactly `binding.input_fields`
    (`documents.rs:71-74`); an empty list passes `{}`.
  - The dispatch in §2.7 depends on specific fields being present:
    `doc_kind`, `task_ref` and `stage`, `source_handoff_id` and
    `terminal_state`, `chunk` and `path`, `source_id`.
  - Every "Bound" binding must list `workspace`. Otherwise `bind_input` sees
    no field and runs the call unbound (`plugin/executor.rs:220-222`), and
    the handler fails.
  - A name that the source collection lacks stalls that binding's arrival
    cursor for good (`scan.rs:389-390`, `?` before the checkpoint).
  - **Fix:** add an `input_fields` column to the §2.6 table. Have
    `defs.json` assert that every bound binding lists `workspace` and that
    every grouped binding lists `doc_kind` plus the fields its handler
    reads.
- **R2-N2. A failed commit is retried, and so are the "harmless" unique
  conflicts.**
  - On a commit failure, `execute` persists the pre-effect journal
    (`callback/plugin.rs:230-248`). `retry_allowed` therefore permits more
    attempts (`workspace/journal.rs:62-78`; `callback/run.rs:719-775`, with a
    1 s backoff that doubles up to 60 s).
  - Each "harmless failed transaction" (a duplicate terminal row, a
    duplicate Signal) runs the plugin and its file side effects three times
    on the serial engine.
  - This is correct, given idempotent writes and the `writer_ref` rule, but
    state it in §2.8 and size it.
  - Retries also look only at failed invocations inside
    `SUCCEEDED_REPAIR_WINDOW` (`documents.rs:511-541`).
- **R2-N3. `FetchedSource.source_id` is unique but has no value on rows
  without bytes.** Prior-art §6 defines `"<run_id>:sha256:<hex>"`, which
  `failed`/`unavailable`/`refused` rows cannot have. structured_book
  deliberately avoids depending on how DefraDB treats null in a unique index
  (§2.3); browser_download does not. Set `source_id = "<run_id>:<status>"` on
  those rows.
- **R2-N4. The tag fallback for structured_book is weaker than "probably".**
  - At `v0.20.0` the callback engine rescans with
    `{collection}(limit: 10000) { _docID }`, unordered
    (`callback/scan.rs:595-610` at the tag). The 10,000-row limit is
    `SEEN_DOCS_SEED_LIMIT`. Rows past it are reachable only through the live
    subscription.
  - Once `BookPage`, `StageTask`, `ItemOutcome` or `Signal` pass 10,000 rows
    (a few dozen books), a missed update is lost without any error.
  - Separately, the RB-5 entry above shows that the tag's file binding
    hard-links even a copied source.
  - R22 should test both: OCR of a copied PDF 20 times on macOS, and
    delivery with more than 10,000 rows in a source collection. The
    "Runtime target" table should say "small libraries only, macOS
    unverified" for the tag.
- **R2-N5. `gents/ocr` callbacks have no `max_attempts`.** At packs
  `cd00bb8`, `ocr-plan` and `ocr-extract` declare none, so the default is 1
  attempt (`plugin.rs:994`). One transient remote-OCR error leaves a chunk
  with only its `path` slot until the 24 h `sb-chunk-join` timeout. The book
  then fails under `fail`. That is visible, but slow. Record it in the
  README, or add `max_attempts` to ocr in the later ocr-change phase.
- **R2-N6. `sb-task-failed` runs once for every successful model Task as
  well.** Every `emit_outcome` fire writes a FireOutcome, and the success
  rows return `null`. That is one extra serial invocation per StageTask,
  about 2,000 per book. Size it with R21/R23.

### Checked and confirmed in revision 2

- `book_search` and `book_research` bind a model-supplied absolute path
  headless through `call_data_bound` (`plugin/executor.rs:274-298`), and the
  allowed `read_write` folder covers `read` (`:236-238`).
- Host bash `allowed_argv_prefixes` apply in every mode, including
  `Unrestricted`, and argv is passed unparsed, with no shell string
  (`toolset/shared/command.rs:617-666`). §3.6's exfiltration note (F-35) is
  accurate and complete for the prefix.
- The `trigger_id` values are not rewritten at pack install (no namespacing in
  `pack/loader.rs`), so `sb-fire`'s `trigger_id: {_like: "sb-task-%"}`
  matches. `pack check` runs the filter against the runtime schemas,
  FireOutcome included (`gents-cli/src/commands/pack/check.rs:226-260`).
- An arrival error for an absent foreign collection is contained to its
  binding (`callback/scan.rs:344-351`), which supports R1.
- Group settings in §2.6 all pass `EventSource::validate_group`: the fixed
  count 2 with no timeout; `{source_field}` counts; and `min_count: 1` with a
  timeout (`document_config/event_trigger.rs:280-366`).
- `OcrJob.run_id` is `@index(unique: true) @immutable` (packs `cd00bb8`
  `ocr/schemas/ocr_job.graphql`), so the design's one OcrJob per run is
  enforced by the database.

---

## Round 3 re-review

Reviewed `DESIGN.md` revision 3 against gents source. Read only; nothing in
shelf, gents or packs was changed. The only git actions were `git fetch` and
`git archive` into a scratch folder.

Baselines:
- gents `origin/main` is still **`d4df8a02b`**: `git fetch` on 2026-10-08
  finds no new commits. `c9d26b8d1` and `967339dfd` are both ancestors
  (`git merge-base --is-ancestor`).
- Tag `v0.20.0` `a5d02f106`, for the fallback claims.
- packs `origin/master` is now `3605ae8` (#18). That commit touches only the
  `pipeline`, `lsp_rust` and `background_continuation` packs, so `gents/ocr`
  is the same as at `cd00bb8`.
- Paths are relative to `crates/gents/src/` at `d4df8a02b` unless stated.

### Verdict

All earlier blocking items are fixed: B1 to B8, RB-1 to RB-5 and R2-1 to
R2-3. The code confirms each fix (tables below). Revision 3 still has **two
new blocking problems**. Both are one-line wiring mistakes, and neither needs
a gents change:
- **R3-1** stops every structured_book book at ingest.
- **R3-2** stops browser_download from passing `pack check` or installing, on
  both main and the tag.

### Round-0 blocking items: status on `d4df8a02b`

| Id | Status | Evidence |
| --- | --- | --- |
| B1 | **Fixed** | A case-insensitive grep of DESIGN.md for `key\|token\|secret\|password` finds only prose, `fire_key` (excluded from `sb-fire`), `goal_token_budget` (not used) and `dispatch_key` (a Rust function). No schema field, `input_fields` entry or filter uses one. Enforced at `callback/documents.rs:205` (`validate_callback_binding`) and `:233` (`reject_secret_bearing_callback_fields`, run again at scan, `callback/scan.rs:492-496`); `:297` strips anything that slips through. |
| B2 | **Fixed** | Surfaces fill only `String` CTX fields (`defra_write/mod.rs:181` always writes `Value::String`). `StageTask` sets the five fill sources and `handoff_id` non-empty (§2.4). browser_download fills only `library_path` and `plan_json`. |
| B3 | **Fixed** | `book_pipeline` declares `workspace` and `workspace_original`; `download_fetch` declares `library_path` and `library_root`; the two read-only tools have no `original_field`. `pack.rs:465-488` passes. |
| B4 | **Fixed** | `--grant-authority` is on all three installs (§0 decision 1, §2.1, §4 steps 5-7). The limits are within the ceilings, and `plugin/store.rs:197` is the refusal that the flag avoids. |
| B5 | **Fixed (modified)** | Callbacks are still serial (`publish_invocation` awaits `run_owned_invocation`, `callback/scan.rs:589-600`). Decision 4.3 replaces the round-0 fix: a gate fires partial at its 72 h timeout, then the bound `stage:<gate>` checks the member set and fails the book. The group settings are valid (`document_config/event_trigger.rs:280-366`). |
| B6 | **Fixed** | Batch and stage openers, plus the finish opener written at ingest. A plan that never appears is caught by the finish watchdog. |
| B7 | **Fixed** | Unique refs on every grouped member collection (§2.3). A losing callback fails its whole transaction (`callback/plugin.rs::commit_success`). |
| B8 | **Fixed** | The floor `d4df8a02b` contains `c9d26b8d1`. The e2e seeds with `gents document create` and retries. |

### Round-1 and round-2 blocking items: status

| Id | Status | Evidence |
| --- | --- | --- |
| RB-1 | **Fixed** | No trigger sets `session_id_template` (§2.5). `target_existing` is false without a template or a graph session (`trigger_engine/mod.rs:650-655`). |
| RB-2 | **Fixed** | `StageTask.handoff_id: String`. Fire admission refuses an empty one (`trigger_engine/mod.rs:636-643`). The FireOutcome SDL is the same on main and the tag (`gents-schemas/.../fire_outcome.graphql`): it has `source_handoff_id` and `attempt`. |
| RB-3 | **Fixed** | The `BookJob` port correlates on `fetch_run_ref`. `output_documents` refuses a plugin value only in the port's own correlation field (`callback/plugin.rs:132-147`). |
| RB-4 | **Fixed** | Every collection that a grouped port writes declares `cause_ref`. |
| RB-5 | **Fixed** | The floor is `d4df8a02b`, and `link_mode` defaults to `copy`. |
| R2-1 | **Fixed** | `workspace` is now the book folder `books/<sha>` (§1, F-37), so the promote is a rename inside the one preopen (`plugin.rs:536-543` preopens exactly `bound.path()`). `book_search` binds `<workspace>/runs/<run_id>` from the footer. |
| R2-2 | **Fixed** | The single-port wire shape is in §2.7, and `CALLBACK_PORTS` is asserted against `pack_config.json`. This matches `output_documents` `[only] => (only, Some(output))` and `(_, None\|Some(Null)) => no rows` (`callback/plugin.rs:80-112`). The shape is the same at the tag (`callback/plugin.rs:86` there). |
| R2-3 | **Fixed** | Both triggers use `queued_serial`, which exists on main and the tag (`document_config/trigger.rs:89`) and needs only a document event source (`trigger_engine/mod.rs:966-980`). |

### New blocking items

#### R3-1. `workspace_original` is listed in four `input_fields`, but no source collection declares it. Every arrival on those bindings fails, and the cursor never moves past it.

The §2.6 `input_fields` table lists `workspace_original` for `sb-ingest`
(BookJob), `sb-page-assemble` (BookChunk), `sb-stage` (StageStart) and
`sb-work` (WorkItem). None of those §2.4 types has that field.

What the code does with it:
- `fetch_source_doc` (`callback/scan.rs:846-860`) runs
  `ensure!(available.contains(field) || field == "_docID", "callback input field {field} is not a safe scalar source field")`
  against the collection's introspected fields
  (`trigger_engine/event_source.rs:354-410`).
- The error propagates through `materialize_for_binding` into
  `deliver_binding_arrivals`. That function returns on `?` before
  `checkpoint` (`callback/scan.rs:386-391`), so the binding retries the same
  first document on every scan and never delivers anything after it.
- The tag has the same check (`callback/scan.rs:645` at `v0.20.0`).

Consequences:
- No `BookJob` is ever ingested, so no book starts.
- If the ingest binding were fixed alone, every `StageStart`, `WorkItem` and
  `BookChunk` binding would stall the same way.
- The design's own `defs.json` rule ("every name exists in the source
  collection's SDL", §2.6) would fail `make test-structured_book` first, so
  this is caught at test time. As written, though, the design cannot pass
  both its tests and its table.

The field is not needed in the projection. The host writes `original_field`
into the plugin input itself after binding, overwriting any value
(`plugin.rs:520-535`: `object.insert(field, bound.original())`). `gents/ocr`
does exactly this: its `ocr-plan` and `ocr-extract` bindings list `path` but
not `path_original` (packs `ocr/pack_config.json` `callback_bindings`).

**Fix:**
- Delete `workspace_original` from the four rows of the `input_fields`
  table.
- Add the rule to §2.6: "`original_field` is never an `input_fields` entry;
  the host injects it."
- Add a `defs.json` assertion that no binding lists a plugin's
  `bind_dir.original_field`.
- Apply the same rule to browser_download. Its bindings are not listed at
  all (see R3-N3), and none of `DownloadPlan`, `FetchProgress` or
  `AgentFetchResult` has `library_root`.

#### R3-2. `root` is placed inside `host.bash`, and `BashTools` refuses unknown fields. browser_download's config then fails to parse, on main and on the tag.

§3.6 gives the `dl-fetch-agent` tools as `host.bash {mode, execution_mode,
network_mode, root: "${GENTS_DOWNLOAD_LIBRARY:-.}", allowed_argv_prefixes}`.
- `document_config/tools.rs:128-213`: `BashTools` is
  `#[serde(deny_unknown_fields)]` and has no `root`.
- `root` is a field of `HostTools` (`tools.rs:65-77`, also
  `deny_unknown_fields`).
- The tag is the same (`tools.rs:125-130` and `HostTools.root` at
  `v0.20.0`).
- `gents/change_unit` shows the right shape:
  `"host": {"bash": {...}, "files": {...}, "root": "${GENTS_CHANGE_UNIT_ROOT:-}"}`.

The `dl-fetch-agent` behavior and its tools document ship in
browser_download in both fetch modes, so the whole pack fails `pack check`
and `pack install`. It is not only the fallback path that breaks.

**Fix:** move `root` up one level, to
`host: {root: "${GENTS_DOWNLOAD_LIBRARY:-.}", bash: {mode: "Unrestricted",
execution_mode: "workspace_write", network_mode: "enabled",
allowed_argv_prefixes: [...]}}`. The enum spellings in the design are
accepted (`gents-loop/src/tool_policy.rs:18-31, 148-162`). Also make BD-B2
run `gents pack check` on the merged fragment, which the structured_book
fragments already do (`check-fragment.sh`).

### New non-blocking findings

- **R3-N1. §3.6 understates the exfiltration path.** `host.root` is only the
  default working folder: "cwd itself is not a sandbox" (`tools.rs:70-74`).
  The `workspace_write` seatbelt profile allows `file-read*` everywhere and
  limits only writes, to `WRITABLE_ROOT` (`toolset/shared/command.rs:1062-1085`).
  - So `curl ... -o x -T <file>` can upload **any file the operator's user
    can read**, including the gents home and `~/.ssh`, not only files
    "readable under `root`". Pointing `GENTS_DOWNLOAD_LIBRARY` at a bare
    folder does not contain that.
  - Correct the §3.6 bullet and the README. The only real limit is the
    `allowed_argv_prefixes` head. A `forbidden_argv_prefixes` entry cannot
    help, because it also matches only a head (F-35).
  - `workspace_write` also needs macOS `sandbox-exec`, and fails closed
    elsewhere (`command.rs:869-895`). State in the runtime table that the
    agent fallback is macOS-only.
- **R3-N2. The e2e step 2 command does not exist.** `gents config backend
  set` takes only `--file`, `--home` and `--graphql` (`gents-cli/src/cli/args.rs:3219-3226`).
  - `gents init --max-queue-depth` exists (`args.rs:1680`).
  - Set the first backend's depth with `init`, and the second backend's
    with `max_queue_depth` inside the `--file` JSON for step 3.
- **R3-N3. browser_download has no `input_fields` table.** §2.6 got one
  (R2-N1), but §3.4 did not. A wrong name stalls that binding for good, as
  in R3-1. Add one row per binding:
  - `dl-resolve` (DownloadJob)
  - `dl-resolve-search` (SourceSearchResult)
  - `dl-fetch` (DownloadPlan: `library_path`, never `library_root`)
  - `dl-fetch-continue` (FetchProgress)
  - `dl-terminal`
  - `dl-finalize` (AgentFetchResult)

  Extend the `defs.json` name check to browser_download's own SDL.
- **R3-N4. Port JSON shape.** `PortSpec` is `deny_unknown_fields`, and
  `schema` is a required string (`graph_pipeline/types.rs:14-30`;
  `gents/ocr` uses `"schema": "OcrChunk/v1"`). "Optional" in §2.6 means
  leaving out `required`, which defaults to `false`; there is no `optional`
  key. A port marked `required: true` that returns no rows fails the call
  (`callback/plugin.rs:113-118`). Every port in §2.6 except those that always
  write should therefore leave `required` out. Say this once in SB-W1's
  acceptance.
- **R3-N5. `sb-plan-check` (F-43) is valid as configured.**
  - A grouped source with no `expected_count` and a timeout passes
    `validate_group` (`document_config/event_trigger.rs:347-358`).
  - It fires only at the timeout with every member it holds
    (`trigger_engine/event_delivery.rs:477-495`, `None => timed_out && minimum <= actual`).
  - The membership query reads up to 257 rows (`:337-345`). A foreign image
    job of more than 256 files quiesces only its own group.
  - The group key includes the binding id (`event_delivery.rs:172-186`), so
    `sb-plan-check` and `sb-chunk-slot` on the same `OcrChunk` rows do not
    interfere.
  - The design's premise holds: `gents/ocr` `plan` returns all chunks from
    one call (`ocr/plugins/ocr/source/graph.rs:55-100`), so they land in one
    transaction.
- **R3-N6. R17 is still unproven at runtime.** No code refuses a callback
  binding on `FireOutcome`. The only FireOutcome rules apply to Triggers
  (`document_config/references.rs:528-537`,
  `config_client/event_source_cursor.rs:198-212`), and the arrival query
  takes any collection name. `source_handoff_id` has no index, which only
  slows the per-document correlation lookup. Keep R17 first in X-00a.

### Checked and confirmed in revision 3

- `bind_input` binds exactly the named path and preopens only it
  (`plugin/executor.rs:203-266`, `plugin.rs:536-543`). Binding the book
  folder `read_write` from an allowed `$LIB` works headless.
- `gents/ocr`'s schemas have every field that the four foreign bindings list:
  `OcrChunk.{run_id, chunk, path, source, pages, format}`,
  `OcrDocument.{run_id, chunk, source, markdown, page_count, complete, error, warnings}`,
  and `OcrJob.files: [String]`. The `ocr-plan` binding passes `files` on,
  and `main.rs:401-410` reads them in the order given. A folder chunk's
  `path` is the job's original path (`graph.rs:56-61, 78`), so the
  "three levels up" rule holds for both `source.<ext>` and `source/`.
- The `remote_ocr` slot of `gents/ocr` is `optional: true`, so step 5's
  single slot binding is enough.
- List fields (`source_paths`, `warnings`, `authors`, `figure_refs`) are
  valid `input_fields`. The introspection keeps every non-system field
  (`event_source.rs:390-410`).
- The `FetchedSource` fields that `sb-from-fetch` reads all exist in
  prior-art §6 plus §3.2's additions.
- `remote.services[].{mcp_service_id, required, tool_names}` match
  `RemoteServiceTools` (`tools.rs:279-318`).
- The tag accepts a plugin that declares `OutboundHttp` (consent text in
  `plugin/authority.rs:40-43`). Installing browser_download on the tag
  therefore depends only on R3-2 and X-00b.
- `gents init --max-concurrent`, `gents config export|apply --root`,
  `gents document create` and `gents version` exist on main.
