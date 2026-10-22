# structured_book and browser_download: design

Status: architecture, phase 2 (design only), **revision 3**. This revision
resolves every blocking item in the "Round 2 re-review" sections of
`REVIEW-fidelity.md`, `REVIEW-gents-feasibility.md` and `REVIEW-work-list.md`
(and the round-0 and round-1 items before them); the mapping is in the
"Revision log" at the end. Every fix was checked against gents `origin/main`
`d4df8a02b` (fetched again for revision 3: unchanged), tag `v0.20.0`
`a5d02f106`, packs `origin/master` `cd00bb8` and Shelf.
Nothing in `shelf`, `gents` or `packs` was modified. The packs are built next,
in a branch of the packs repo (suggested name `feat/structured-book-packs`).

Inputs: `shelf-inventory.md`, `gents-pack-guide.md` and `download-prior-art.md`
in this folder, plus the three reviews.

## Runtime target

The packs target **gents `origin/main` at `d4df8a02b`** (2026-10-08, fetched
again for revision 2; `v0.20.0-305-gd4df8a02b`), or any later commit. That
is also the **floor**: it contains `c9d26b8d1` (durable callback arrival
cursors, gents #2343) and `967339dfd` (#2382: `BoundDir::pin` accepts a
hard-linked file through its pinned folder, gents #2379, F-33). Below
`967339dfd`, OCR of a hard-linked source fails at random on macOS.

| Build | What it is | ocr pack | structured_book | browser_download, plugin fetch | browser_download, agent fetch (fallback, §3.6) |
| --- | --- | --- | --- | --- | --- |
| Installed `~/bin/gents`, "0.20.0 (f44a0cef0)" | a commit from before the `v0.20.0` tag; it has no `bind_dir.original_field` and no `plugin dirs` | no (`pack check` fails on `optional`, guide §0) | no | no | no |
| Tag `v0.20.0` (`a5d02f106`) | the release | local OCR only (spike) | **small libraries only, macOS unverified** (R22): the tag's callback rescan reads at most 10,000 `_docID`s per collection, unordered (`callback/scan.rs:595-610` at the tag), so once `BookPage`, `StageTask`, `ItemOutcome` or `Signal` pass 10,000 rows a missed live update is lost without an error; and the tag's file binding hard-links even a **copied** source into `.gents-bind` (`plugin/bound.rs:12-18`), so every file-bound OCR call can hit the pre-#2379 macOS refusal. `link_mode: copy` only | **no**: `plugin/http_calls.rs` and `plugin/rounds.rs` do not exist at the tag, and plugins get no network | yes (spike X-00b) |
| `origin/main` ≥ `d4df8a02b` | unreleased | yes | yes (hard link allowed) | yes | yes |

The `emit_outcome`/`handoff_id` rule (F-25) and the "session template names
an existing session" rule (F-26) are the same on the tag and on main, so the
fixes for them apply to both.

The operator picks one (see "Decisions for the operator"). The design below
is written for `main`. Every place where the tag behaves differently says so.
A plugin cannot learn the gents build it runs on, so the source placement is
a job field: `BookJob.link_mode` is `copy` (the default: a streamed copy,
safe on every build and independent of R8) or `hard_link` (opt-in, only on
main ≥ `967339dfd`; refused by nothing in code, so the README states the
rule). `RunFile.link_mode` records what ingest did.

## Facts this design relies on (checked in source)

`G` is `gents/crates/gents/src` at `origin/main` `d4df8a02b`; `GC` is
`gents/crates/gents-cli/src/commands`. Facts marked "(review)" were found by a
reviewer and re-checked for this revision; "(r2)" marks facts added in
revision 2, each read in source for this revision.

| # | Fact | Where |
| --- | --- | --- |
| F-1 | A callback plugin gets the projected source doc on stdin. A grouped callback gets a JSON array of the members' `input_fields` | `G/callback/scan.rs::materialize_group` |
| F-2 | Grouped output correlation is the invocation's `caused_by_correlation` (the group key) | `G/callback/plugin.rs::execute` |
| F-3 | `bind_input` does `input.get(field)`, so a grouped (array) call is **never bound** | `G/plugin/executor.rs::bind_input` |
| F-4 | `access: read_write` with no `write_fields` always binds read-write | `G/pack.rs::PluginDirBinding::call_access` |
| F-5 | Plugin outputs are refused only for runtime and protected collections; pack ownership is not checked | `G/callback/plugin.rs::writable_output_collection` |
| F-6 | `Task.output_schema_ref` is stored but not enforced | `G/document_config/task.rs` |
| F-7 | **(r2, corrects revision 1)** `FireOutcome` carries `handoff_id` (its own id), `fire_key`, `trigger_id`, `source_collection`, `source_doc_id`, `session_id`, `terminal_state`, `reason`, **`source_handoff_id`** (copied from the delivered doc's `handoff_id`), `attempt` (copied from the delivered doc's integer `attempt`), `created_at`. It is written only for a Task with `emit_outcome: true`. With no `session_id_template`, `session_id` is derived from the fire identity (trigger + source `_docID`), so every source doc gets a fresh session | `gents-schemas/.../fire_outcome.graphql`, `G/trigger_engine/durable.rs:180-215`, `G/trigger_engine/mod.rs:578-650` |
| F-8 | OCR `plan` splits a paged file into ranges of `per = max(20, ceil(count/64))` units (at most 64 chunks). **Image folders get one chunk per file**, up to 1,000 | packs `ocr/source/plan.rs::ranges`, `graph.rs::plan` (`_ => vec![""]`) |
| F-9 | `OcrChunk` carries `path`; `OcrDocument` does not. An `OcrDocument` can be `complete: false` (extract stopped at `EXTRACT_MAX_BYTES = 1_500_000`) | packs `ocr/schemas`, `graph.rs` |
| F-10 | `gents pack build` mirrors each plugin's `source` into `target/plugins/<name>/`; `pack check` accepts any file under some plugin's `source` | `gents-cli/src/commands/pack/{build.rs:247,check.rs:163-183}` |
| F-11 | No Shelf `system.tmpl` contains a template action | shelf |
| F-12 | **(review)** Any field name containing `KEY`, `SECRET`, `TOKEN` or `PASSWORD` (any case) is secret-bearing. A binding that lists one in `input_fields` is refused; and every callback input, including grouped members, has such fields stripped recursively | `G/toolset/shared/command.rs:813`, `G/callback/documents.rs::{validate_callback_binding,strip_secret_fields}` |
| F-13 | **(review)** A surface `fill` always writes `Value::String`; an `Int`, `Float`, `Boolean` or list field given a string fails ("values are not coerced"). A trigger fill source field must be a non-null string or integer | `G/defra_write/mod.rs:173-186`, `G/defra_write/input.rs::literal`, `G/trigger_engine/event_source.rs::captured_source_field` |
| F-14 | **(review)** A `JSON` scalar is accepted but any empty array inside it is refused | `G/defra_write/input.rs::reject_empty_json_arrays` |
| F-15 | **(review)** `bind_dir.input_field` and `original_field` must be string properties of the plugin's `input_schema` | `G/pack.rs:455-480` |
| F-16 | **(review)** Declaring `limits` above the previous grant needs `--grant-authority` at install (a first install has no grant) | `G/plugin/store.rs:197`, `G/plugin/authority.rs::limits_consented` |
| F-17 | **(review)** Callback invocations run **serially**: `publish_invocation` awaits `run_owned_invocation` inline, one engine per agent | `G/callback/scan.rs:589-600`, `G/agent/runtime/startup.rs:704` |
| F-18 | **(review)** A group's clock (`first_seen_at`) starts at its first member; a group with no members never times out. More members than `expected` quiesce the group for good; members must agree on `expected`; `expected` above 256 quiesces it. A group fires at `count == expected`, or at timeout when `count >= min_count`; with no timeout it waits for the exact count | `G/trigger_engine/event_delivery.rs:340-430, 477-496` |
| F-19 | **(review)** A callback's output documents, its invocation update and its result are written in **one transaction**, all or nothing | `G/callback/plugin.rs::commit_success` |
| F-20 | **(review)** A plugin callback runs once unless `max_attempts` is set; an interrupted call is never re-run | `G/plugin.rs:994`, `G/document_config/callback.rs:70` |
| F-21 | **(review)** `output_obligation` sets a minimum number of writes only; a second write in the same run is allowed | `G/document_config/write_tool.rs`, `G/agent/output_obligation.rs` |
| F-22 | **(review)** Host HTTP (main only): 1 MiB per response, 16 MiB per call, 16 requests per round, no redirects followed | `G/plugin/http_calls.rs` |
| F-23 | `Task.goal_token_budget` exists | `G/document_config/task.rs:35` |
| F-24 | The web-research MCP has no download tool. `web_scrape_url` returns Markdown, and `content_hash` hashes the envelope, not the file | `source-inc/web-research-mcp` tool list; prior-art §1.1 |
| F-25 | **(r2)** A Trigger that delivers a collection to an `emit_outcome` Task needs `handoff_id: String` on that collection, or config publication (and so `pack install`) refuses it; only `CallbackResult`/`WorkspaceReceipt` are exempt. Fire admission also refuses each doc whose `handoff_id` is empty ("emit_outcome requires a source handoff_id"). Same at the tag (`desired_state.rs:518`, `mod.rs:636`) | `G/config_client/desired_state.rs:565-626`, `G/trigger_engine/mod.rs:636-643` |
| F-26 | **(r2)** A trigger with `session_id_template` makes every fire `target_existing`; admission then refuses a session that does not exist ("Task target session is missing or belongs to another owner"). Omit the template for a fresh session per fire. Same at the tag (`materialize.rs:1332`) | `G/trigger_engine/mod.rs:650-655`, `G/lifecycle/materialize.rs:1318-1334`, `G/trigger_engine/durable.rs:20-30`, `G/self_config/command/help.rs:236,353` |
| F-27 | **(r2)** The host owns each output port's `correlation_field`: it writes the source correlation there, and a plugin value that differs fails the call ("the plugin changed …, which the runtime writes"). Every other field is the plugin's. A handler with outputs must read a source correlation (`validate_handler`). Port fields are **not** checked against the SDL, so a port whose correlation field the collection lacks fails only at write time | `G/callback/plugin.rs:45-76, 132-147` |
| F-28 | **(r2)** `pack check` requires every slot behavior to exist in `pack_config.agent_behaviors` (and nothing else there); `pack test` runs `check_dir` before it builds, and `check_dir` refuses a documents pack whose `.afb` is not built yet. `make test-<pack>` (`scripts/test-pack.sh`) builds first | `G/pack/inference.rs:282-320`, `GC/pack/test.rs:44-52`, `GC/pack/check.rs:75-90`, packs `scripts/test-pack.sh:102-112` |
| F-29 | **(r2)** `pack check` runs every event-source **filter** as a query in a throwaway node holding only the runtime schemas and this pack's `schemas/`; a filter on another pack's collection fails the check. Unfiltered sources are not queried. At install, a source collection that is absent is skipped, not refused | `GC/pack/check.rs:228-260`, `G/config_client/desired_state.rs:801-821` |
| F-30 | **(r2)** `pack test` loops over `manifest.plugins` and runs every `<source>/tests/*.json` case under **each** entry, with that entry's artifact, limits, network grant and `bind_dir` | `GC/pack/test.rs:93-140` |
| F-31 | **(r2)** A plugin with a network grant gets a live HTTP session; the host strips `state` and `http_results` from the caller's input and serves requests for real, so canned HTTP answers cannot be given in a plugin case | `G/plugin/rounds.rs:95-110` |
| F-32 | **(r2)** `test-pack.sh` built-in check: no task sets `goal_token_budget` ("goal budgets are opt-in"). `goal_objective_template` requires `built_ins.enable_goal_tools` and runs until `update_goal` completes it | packs `scripts/test-pack.sh:192-193`; `G/self_config/command.rs:2569` |
| F-33 | **(r2)** On macOS, `BoundDir::pin` sees any one name of a hard-linked file; before `967339dfd` it refuses that ("the path changed after it was validated"); `967339dfd` adds `named_in_pinned_folder`. Absent at the tag | `G/plugin/bound.rs:288-340`; tag `plugin/bound.rs:293` |
| F-34 | **(r2)** Group `timeout_secs` is a static positive integer (only `expected_count` can name a source field). At timeout a group with fewer than `min_count` members is marked dormant and does not fire | `G/document_config/event_trigger.rs:225-366`, `G/trigger_engine/event_delivery.rs:388-397, 477-496` |
| F-35 | **(r2)** Host bash `allowed_argv_prefixes` match only the head of argv; any arguments may follow | `G/toolset/shared/command.rs:775-782` |
| F-36 | **(r2)** Pack JSON string values are env-interpolated at load: `${VAR}` (required, error if unset), `${VAR:-default}`. Present on the tag too | `G/pack/interpolate.rs` |
| F-37 | **(r3)** A plugin call binds **one** directory (or one file through a private folder holding a hard link to it), and WASI preopens only that; a covering allowed folder grants access but does not widen the preopen | `G/plugin/bound.rs:1-18`, `G/plugin/executor.rs:209-268` (`bind_input`) |
| F-38 | **(r3)** With **one** output port, the plugin's whole value is that port's document (or array, for `many`); only with several ports is it an object keyed by port name, and an unknown key fails the call. `null` (or an absent key) writes nothing; `{}` writes an empty document carrying only the correlation | `G/callback/plugin.rs:80-126` (`output_documents`) |
| F-39 | **(r3)** Trigger `concurrency: "serial"` **skips** a fire while the trigger is busy, and the event-source cursor acknowledges and checkpoints past it, so that arrival never fires. `queued_serial` runs them in order. Same at the tag | `G/trigger_engine/mod.rs:836-850`, `G/trigger_engine/event_source/durable.rs:143-170`, `G/config_client/event_source_cursor.rs:351-381`, help `self_config/command/help.rs:353`; tag `document_config/trigger.rs:89` |
| F-40 | **(r3)** Publication requires a `ToolServiceRegistry` document for every `remote.services.mcp_service_id` (so `pack install` fails without it; `pack check` does not see it). `external_dependencies` registers nothing. In `scripts/test-pack.sh` only `install_graph` calls `register_services`; `install_documents` and `runtime_case` do not | `G/document_config/references.rs:394-400`; packs `scripts/test-pack.sh:243-275, 293-315, 386-417` |
| F-41 | **(r3)** A `pack test` case with `bind` binds the committed case folder itself, always read-write, with no copy or reset; a case passes only on `Success` with an exact `expect`; `PluginCase` has only `input`, `expect`, `bind`. `test-pack.sh` runs `cargo test` natively, in place, once per **plugin entry** | `GC/pack/test.rs:150-180`, `G/plugin/bound.rs:68-93`; packs `scripts/test-pack.sh:145-160` |
| F-42 | **(r3)** `gents/ocr` plan: PDF pages from `pdf::page_count` (`Backend::open`: the lazy indexer `pdflazy.rs`, falling back to `hayro_syntax::Pdf` up to 256 MiB), ranges by `plan.rs::ranges`; a folder is walked recursively in byte-wise name order, hidden entries skipped, every other file one chunk, **unless `OcrJob.files` lists the files, which are then read in that order**; a source whose count fails still gets one chunk without `pages`; chunk indices start at 0; `OcrChunk.path` is the job's (original) path. All OcrChunks of a job are written by one callback, so in one transaction (F-19) | packs `ocr/plugins/ocr/source/{pdf.rs:482-516, plan.rs:58-82, main.rs:400-455, graph.rs:55-100}` |
| F-43 | **(r3)** A grouped event source may have **no** `expected_count` if it has `timeout_secs`; it then fires at the timeout with every member it holds (`min_count` ≤ members ≤ 256) | `G/document_config/event_trigger.rs:280-366` |
| F-44 | **(r3)** A rendered Task prompt above `MAX_RENDERED_BYTES = 1 MiB` fails at fire admission | `G/template/mod.rs:40,89` |
| F-45 | **(r3)** `gents/ocr` can end an extract with `complete: false` for **two** reasons: the 1.5 MB byte cap and its own call time budget (`ctx.rs:16`, "the OCR time budget of this call ran out"); the pack does not continue from the cursor. Its two callbacks declare no `max_attempts` (so 1 attempt) | packs `ocr/plugins/ocr/source/{ctx.rs:16, graph.rs:120-140}`, `ocr/pack_config.json` |

---

## 0. Decisions in one page

1. **Two `documents` packs plus the existing `gents/ocr`:**
   - `gents/browser_download` turns an identifier, URL or description into a
     verified file and writes a `FetchedSource`.
   - `gents/structured_book` turns a `BookJob` or `FetchedSource` into
     canonical chapters and paragraphs, a structure digest and an exact-quote
     tool.

   Neither pack can list `gents/ocr` as a `dependency`, because only `graph`
   packs can be dependencies. Install order is documented instead: ocr, then
   browser_download, then structured_book. All three installs need
   `--grant-authority` (F-16: all of them declare `limits`).
   browser_download also needs the `web-research-mcp` service **registered**
   before its install (a `tool_service_registries` entry; F-40), which the
   README, the e2e (§4 step 5b) and the packs harness change X-02 do.
2. **Triggers and datastore writes, not a graph.** Same reasons as before:
   open-ended fan-out known only at run time, and one `documents` pack
   consuming another pack's collections. We get run status, cancel-free
   liveness and bounded retries back with `BookStatus`, a finish watchdog
   (decision 4) and retry budgets enforced in plugin code.
3. **The book workspace is a folder on disk, and DefraDB is the system of
   record.** Unchanged. Workspace files are deterministic intermediates and
   are typed in one Rust module (`core/contract.rs`, §1.1) that every task
   compiles against.
4. **Gates: "collect unbound, decide bound", never act on a partial set.**
   1. Producers create `Signal{gate, gate_ref, member, signal_ref}`.
      `signal_ref` is unique (`<run_id>/<gate>/<member>`), so a duplicate
      producer fails its own transaction (F-19) instead of over-filling the
      group (F-18).
   2. One grouped event source **per gate kind** (front, ocr, link, finish),
      each with its own timeout, feeds the unbound `gate_open` callback. It
      only reports: `StageStart{stage:<gate>, payload:{members, complete}}`.
   3. The **bound** `stage:<gate>` handler checks the member set against the
      set that gate needs for the book's variant. If one is missing it writes
      `BookStatus{failed, reason:"gate <g> incomplete: missing <m>"}` (link,
      finish), or applies the documented policy:
      - **front** (Shelf requires the exact prefix, `state.go:57-61`,
        `ConsecutivePagesComplete(20|30)`): under `ocr_failure_policy: fail`
        a missing member releases nothing (the `ocr` gate then fails the
        book); under `quarantine` it continues only when every missing front
        page is marked quarantined in `pages/`, and otherwise releases
        nothing;
      - **ocr**: `ocr_failure_policy`.
   4. **One terminal status per run, with the database as the authority**
      (Shelf persists the terminal status synchronously, `state.go:261-264`):
      - Every `BookStatus` row carries `status_ref @index(unique: true)`.
        Terminal rows (`complete`, `degraded`, `failed`, `reused`) use
        `"<run_id>/terminal"`, so DefraDB admits one per run; progress rows
        use `"<run_id>/progress/<writer_ref>/<n>"` (below).
      - A terminal row is always written **alone** (no other output in that
        invocation), so a unique-ref conflict fails a transaction that loses
        nothing (F-19). The one exception is ingest's `reused` row, written
        with its `BookRun`: it is the run's first write, so nothing can
        conflict with it.
      - Progress rows use `"<run_id>/progress/<writer_ref>/<n>"`, with `n`
        the row's index within that invocation, because several ports return
        many `BookStatus` rows from one call (`page_assemble`, `stage`,
        `work`, `result`) and two equal refs would fail the whole transaction.
      - Every gate or stage failure in this document ("writes
        `BookStatus{failed}`") goes through `terminal()` and these rules.
      - `status.json` is only a hint for the plugin (a callback cannot query
        DefraDB). It is created `O_EXCL` with `{status, reason, stage,
        writer_ref}` just before the terminal row is returned. `writer_ref`
        is deterministic: `<dispatch key>/<source ref>`.
      - A handler that wants to end the book and finds the marker: same
        `writer_ref` → it is its own retry after a failed commit, so it
        **re-emits** the marker's row; different `writer_ref` → it emits
        nothing and releases nothing.
      - Every `stage:*`, `*_next` and `result:*` handler first reads the
        marker and releases no further work once the book is terminal (Shelf's
        `FailBook` stops the job; this saves cost). Two exemptions:
        `stage:finish` always emits (decision 4.5), and `stage:task_retry`
        is an ordinary `stage:*` handler and follows the rule.
      - **Refusals before a run folder exists** (a `run_id` that fails the
        rule of §2.3, more than one source path, an unreadable or uncountable
        source, more than 256 images): ingest returns only the lone terminal
        `BookStatus{failed}` (`status_ref "<run_id>/terminal"`), writes no
        marker and **no finish opener**, so no group binds a missing folder.
   5. **Watchdog.** `ingest` writes an opener `Signal{gate:"finish",
      member:"opener"}` with `expected = RunFile.finish_expected`
      (`members(format, variant) + 1`; EPUB ignores `variant`, as Shelf's
      import has none); every finish Signal is built by the one
      `contract.rs::finish_signal(member)` builder, so all producers agree on
      `expected` (F-18). The finish clock starts at ingest. `stage:finish`
      **always** emits a terminal row: the marker's content if a marker
      exists (this repairs a marker whose commit failed for good), otherwise
      complete, degraded or "pipeline did not finish: missing <members>"
      (creating the marker first). If a terminal row already exists, the
      unique `status_ref` makes this a harmless failed transaction.
5. **Fan-out: "item, batch, stage", with openers and released batches.**
   1. Every model stage, single-item ones included (metadata, toc_finder,
      toc_extract, pattern), and every deterministic multi-item stage goes
      through this path. A single-item stage is a batch of 1.
   2. A bound stage writes the task files for all items, then **releases one
      batch at a time**: at most 32 `StageTask`s and at most 3 MiB of output
      per batch (model stages), or 128 `WorkItem`s (deterministic). With the
      batch it writes one **opener** `ItemOutcome{status:"opened"}`, so
      `batch_size = n + 1` and the batch clock starts at release (F-18). The
      first release also writes a stage opener `BatchOutcome{counts:"opened"}`
      with `batches_total = b + 1`.
   3. Each item ends in exactly one terminal `ItemOutcome`, whose
      `outcome_ref` (`<run_id>/<stage>/<item_ref>`) is unique. Retries create
      new `StageTask`s and never an `ItemOutcome`.
   4. `batch_close` (unbound) writes `BatchOutcome` and
      `StageStart{stage:"<stage>_next"}`; the bound handler releases the next
      batch. `stage_close` (unbound) writes `StageStart{stage:"<stage>_closed"}`.
      The bound `_closed` handler diffs the members against
      `tasks/<stage>/` and records every missing item as
      `failed: never reported`.
   5. Limits: 255 batches × 32 = 8,160 model items per stage; group timeouts
      4 h per batch and 72 h per stage.
6. **Model calls are Tasks, structured output is a write tool, and the
   result is validated against Shelf's schema.**
   - One behavior, one Task and one surface `create` tool per Shelf prompt,
     with `output_obligation.minimum_writes = 1`. Tool names as Shelf.
   - The surface fills only **string** context fields: `run_id`
     (correlation), `stage`, `task_ref`, `item_ref`, `workspace` (F-13). The
     result handler parses the attempt and repair counters from `task_ref`
     (`<run_id>/<stage>/<item_ref>/<attempt>.<repair>`) and reads the batch
     fields from `tasks/<stage>/<item_ref>.json`. `task_ref` is unique on
     every `*Result` collection, so a second write in one run fails and the
     model sees the error (F-21).
   - The bound `result` handler rebuilds the logical object (flat fields plus
     parsed `*_json` strings; every list field that came back `null` becomes
     `[]`, because gents writes an empty nullable list as null,
     `defra_write/input.rs:104-106`; any **other** field that came back
     `null` is left out of the object when its Shelf schema type does not
     allow null, because gents returns an omitted optional field as `null`,
     e.g. toc_finder's `toc_page_range`/`structure_summary` when
     `toc_found=false`, `toc_finder/tools/write_toc_result.go:24-49`; CTX
     fields and port-only fields such as C7's
     `pages_checked`/`structure_notes_json` are stripped) and validates
     it against Shelf's JSON schema (`core/fixtures/schemas/*.json`, dumped
     from Shelf by SB-G0). A schema failure is a **repair**: up to 2 per
     attempt, with Shelf's `structuredRepairPrompt` text, as Shelf's
     `maxStructuredRepairAttempts = 2` (`providers/structured_output.go:14`);
     the third schema failure in one attempt uses up that attempt. So a
     metadata or classify call keeps Shelf's up to 9 generations.
   - **A Task that dies is retried** (§2.8, "Task failures"): every model
     Task sets `emit_outcome: true`, which needs `StageTask.handoff_id`
     (F-25); every StageTask writer sets `handoff_id = task_ref`. A callback
     on `FireOutcome`, correlated on `source_handoff_id`, writes a failure
     marker, which joins with the original `StageTask` and re-issues it with
     `attempt + 1`. **No trigger sets `session_id_template`** (F-26): each
     StageTask doc gets a fresh session, which is Shelf's fresh agent per
     retry. Fallback if R17 says no: the batch timeout is the only signal and
     counts as a used-up budget (D14).
7. **User prompts are rendered by the plugin, system prompts are copied.**
   Unchanged. Each `tasks/*/prompt.md` is `{{ doc.user_prompt }}`, an
   optional feedback block, and a footer with the tool arguments and the
   hard-coded write tool name (change C8).
8. **Vision is out of v1.** Unchanged; `load_page` replaces
   `load_page_image` plus `load_ocr_text` with an explicit observations
   contract in its arguments (change C7).
9. **Plugins: four Rust crates (sources), five plugin entries over two packs.**

   | Plugin | Pack | Crate (`source`) | Role |
   | --- | --- | --- | --- |
   | `book_pipeline` | structured_book | `plugins/book` | Every deterministic stage, as callbacks only |
   | `book_search` | structured_book | `plugins/book_tools` | Read-only model tool for the 4 agents |
   | `book_research` | structured_book | `plugins/book_tools` | Read-only model tool for other packs (`validate_quote`, research reads) |
   | `download_resolve` | browser_download | `plugins/download_resolve` | Resolver APIs, host HTTP with an allow-list (main only) |
   | `download_fetch` | browser_download | `plugins/download_fetch` | Ranged fetch to a file and finalize (verify, hash, place); host HTTP to public HTTPS hosts (main), finalize-only on the tag |

   `pack test` runs every case of a `source` under every entry naming that
   source (F-30), so entries that differ in `bind_dir` field, limits or
   network grant must not share one. `book_search` and `book_research` do
   share `plugins/book_tools`: they declare the same `input_field: "book"`,
   `access: read` and limits, so a case behaves the same under both (it
   runs twice; that costs only time). Shared code (`core/`) lives once in
   `plugins/book/source/core/` and `plugins/download_resolve/source/common/`
   and is copied into the sibling crate by `plugins/book_tools/sync-core.sh`
   and `plugins/download_fetch/sync-common.sh` (owned by SB-01a and BD-01; a
   file under a plugin `source` needs no `assets` entry, F-10). Each script
   has a `--check` mode, and the sibling crate carries a `#[test]` that
   compares its copy with the original byte for byte, so drift fails
   `make test-*` (which runs `cargo test` once per Rust plugin **entry**, F-41,
   so `book_tools` runs twice). A symlinked directory cannot be used:
   `build.rs::mirror` reads a directory symlink as a file. The binary still
   refuses pipeline ops when called with an `op`.
   - **The mirror is committed with its original.** Any task that changes a
     file under `plugins/book/source/core/` (or `download_resolve/source/common/`)
     runs the sync script before committing and commits the mirrored path
     with the same file name; its target paths list both. Tests inside
     `core/` and `common/` read files only below `core/` (`common/`), for
     example `core/fixtures/`, because they also compile and run in the
     mirror, where `../../tests/...` does not exist.
   - **A third copy, `plugins/book/source/ocrcount/`,** holds `gents/ocr`'s PDF
     page counter verbatim (`pdflazy.rs`, `pdfobj.rs`, `pdfdecode.rs`,
     `src.rs`, the `Backend::open`/`page_count` body of `pdf.rs`, and
     `plan.rs::ranges` with `MAX_CHUNKS`/`UNITS_PER_CHUNK`), plus a small
     shim for the two items those files import from elsewhere in the ocr
     crate (`input::Options`, `resume::Fnv`). It is copied by
     `plugins/book/sync-ocrcount.sh` from packs `ocr/plugins/ocr/source/`, and
     a `#[test]` compares each copied file (or function body) with the ocr
     source in the same repository, so a change to the ocr counter fails
     `make test-structured_book` until it is re-synced (F-42, §2.8 "OCR plan
     agreement").
10. **Stable ids and the digest.**
    - `book_id = "bk_" + sha256[0..24]`; `chapter_id = "ch_%03d"`;
      `paragraph_id = "<chapter_id>.p%04d"`.
    - The structure digest is Shelf's `shelf-research-v1` algorithm byte for
      byte. The ids that go into it differ from Shelf's (Shelf used DefraDB
      `_docID`s), so no existing Shelf digest or citation reproduces; that is
      recorded (change D11). `ch_%03d` is positional; a forced re-run with a
      different ToC can renumber, and the digest changes with it.
    - `validate_quote`, `search_passages` and `read_passage` always read the
      **latest** `canonical/book.json`, as Shelf does, so a stale pinned
      digest gets `version_match: false` and no search. Older
      `canonical/<digest-hex>.json` files are kept only for the new op
      `read_passage_at_version`, which never reports `version_match`.
11. **Naming rule.** No schema field, `input_fields` entry or filter field may
    contain `key`, `token`, `secret` or `password` (F-12). Join and dedupe
    fields end in `_ref`. `defs.json` asserts this for both packs.

---

## 1. Workspace layout (on disk, inside an allowed folder)

```
<library>/                                   # gents plugin dirs add <library> --access read_write
  sha256/<aa>/<hex>.<ext>                    # browser_download content-addressed files
  .parts/<run_id>.part                       # browser_download in-flight downloads
  books/<sha256>/                            # = `workspace` of every pipeline document except BookJob (F-37)
    canonical/book.json                      # latest canonical (atomic rename, written by stage:commit_closed)
    canonical/<digest-hex>.json              # immutable per digest (read_passage_at_version only)
    latest_run.json                          # D23, written by terminal() on a failed run
    runs/<run_id>/                           # "the run folder"; book_search binds this one, read-only
      source.<ext> | source/<NNNN.ext...>    # streamed copy (default) or hard link (link_mode, main >= 967339dfd only); OcrJob.path points here.
                                             #   An image folder is copied flat, only the counted files, renamed NNNN.<ext> in ingest's order
      run.json                               # RunFile
      status.json                            # TerminalMarker hint {status, reason, stage, writer_ref}, O_EXCL (decision 4.4)
      pages/<NNNN>.json                      # PageFile
      toc/finder.json  toc/entries.json  toc/final.json
      tasks/<stage>/<item_ref>.json          # TaskFile, one per item, model or deterministic (marker for missing-item diffs;
                                             #   `closed: true` once <stage>_closed ran, so late results are refused)
      results/<stage>/<item_ref>.json        # ResultFile, validated outcome
      pattern.json  gaps.json
      chapters/<chapter_id>.json             # ChapterFile: skeleton -> mechanical -> classified -> polished
      canonical/book.json                    # this run's canonical, promoted on commit_closed
```

**`workspace` on every pipeline document is the absolute book folder
`<library>/books/<sha256>`**, except `BookJob.workspace`, which is the library
root. A call binds exactly one directory (F-37), and the book folder is the
smallest one that holds both the run folder and the book-level files
(`canonical/`, `latest_run.json`), so every handler can do every write it
owns. Handlers find their run folder as `<workspace>/runs/<run_id>`; that is
safe because `run_id` is a single path segment (§2.3).
**Write rule:** a handler writes only below `runs/<its own run_id>/`, except
the two named book-level writes: `stage:commit_closed` promotes into
`canonical/` (rename inside the one preopen) and `terminal()` writes
`latest_run.json`. `ingest`, bound on the library root, creates the book and
run folders and reads `canonical/` for reuse. `book_pipeline` binds
`workspace` with `original_field: "workspace_original"` to learn the real path
for `OcrJob.path`.

Workspace recovery from `OcrChunk`: the source is at `<run>/source.<ext>` or
`<run>/source/` and `OcrChunk.path` is the job's path (F-42), so
`workspace` is the folder **three levels up** from `OcrChunk.path`
(`source*` → `runs/<run_id>` → `runs` → book folder). The three foreign
sources are **unfiltered** (F-29), so `chunk_slot` and `chunk_text` return no
output when `run_id` does not start with `sb-`, and `from_fetch` returns no
output unless `structure == true` and `status ∈ {ok, duplicate}`; their
ports are optional. `chunk_join` and `page_assemble` are also no-ops when the
folder has no `run.json`.

**Late results.** A result that arrives after its batch group fired at
timeout still runs `result:<stage>`. The bound `<stage>_next` handler (fed
by `batch_close`, whose payload lists the batch's members) sets
`closed: true` in the task file of every item of that batch that has no
member, and `<stage>_closed` does the same for the last batch. Each result
handler refuses (no output) when its task file has `closed: true`, so no
`TocLink`, `GapFix`, `TocEntry` revision or `results/` file lands after its
batch closed.

All workspace writes are temp-file-plus-rename, so a callback retried by
`max_attempts: 3` (F-20) is idempotent. A commit failure leaves the
invocation retryable (`callback/plugin.rs:230-248`, `workspace/journal.rs:62-78`,
1 s backoff doubling to 60 s), so every "harmless failed transaction" of this
design (a duplicate terminal row, Signal or outcome) runs the plugin and its
file effects up to 3 times on the serial engine; the idempotent writes and the
`writer_ref` rule make that correct, and it is counted in the throughput
estimate (§2.8).

### 1.1 `core/contract.rs`: the inter-task API (owned by SB-01b; changed only by `SB-K<n>` tasks)

| Item | Writer | Readers |
| --- | --- | --- |
| `RunFile` (`run.json`: format, variant, toc_policy, ocr_failure_policy, link_mode, page_count, chunks_total, front_expected, **finish_expected**, per, **ranges[]** (the `pages` string of every chunk, from `ocrcount::ranges`), **files[]** (image folders: the copied `NNNN.<ext>` names, which are also `OcrJob.files`, in page order; `page_assemble` maps `OcrChunk.source` to a page through this list, never through the chunk index)) | S1 | S2, S3, all stages |
| `TerminalMarker` (`status.json`: status, reason, stage, writer_ref) struct; `terminal(…)` (in `core/workspace.rs`, SB-C1) is the one function that creates the marker and returns the lone terminal `BookStatus` (decision 4.4) | any stage that fails the book; S3 | S3, every release handler |
| `PageFile` (`pages/<NNNN>.json`: page_num, markdown, headings[], header, footer, figure_refs[], quarantined, quarantine_reason) | S2 | C3, C4a, C4b, S4, S6, S8a |
| `TaskFile` (`tasks/<stage>/<item_ref>.json`): common part (batch_ref, batch_size, stage_ref, batches_total, attempt history as a list of `task_ref`s, `closed`, **rendered `user_prompt` and `prompt_bytes`** for model stages) plus one context variant per stage, enumerated below | C1 helper, called by S4-S10 | C3, C4a, C4b, P1, S3 (the generic `_next` release builds `StageTask`s from the stored `user_prompt` without calling C6-C8) |
| `TocFiles` (`toc/finder.json`, `toc/entries.json`, `toc/final.json`) | S5, S7b, S10 | S6, S7a, S8a |
| `PatternFile` (`pattern.json`) and `GapsFile` (`gaps.json`) | S7a; S7b | S7b; S7b |
| `ChapterFile` (4 states) | S8a, S8b, S10 | S9 |
| `BookMetadataFile` (the validated metadata object kept for canonical) | S4, S10 | S9 |
| `ResultFile` | S4-S8b | S3 |
| `GatePayload`, `StagePayload` structs per gate and stage (incl. `ocr_plan`'s chunk list, §2.8) | all | S3, S2 |
| Output port names (rule: port name = snake_case collection, `stage_task`, `item_outcome`, ...) as `const`s | SB-01b | every stage, SB-W1 |
| The stage table of §2.5 as a `const` array | SB-01b | every stage, check scripts, `defs.json` |
| Ref builders (`task_ref` with `<attempt>.<repair>`, `outcome_ref`, `signal_ref`, `status_ref` incl. the progress `/<n>`, `writer_ref`, ...) and their **parsers**, `finish_signal(member)`, `members(format, variant)`, and retry and repair budgets per stage. `task_ref = <run_id>/<stage>/<item_ref>/<a>.<r>` is parsed as exactly 4 `/`-separated segments; the builders refuse an `item_ref` containing `/`, and ingest refuses a `run_id` outside `[A-Za-z0-9._:-]+` (§2.3) | SB-01b | every stage |
| `CALLBACK_PORTS`: for every callback id, its ordered port names, and `callback_of(dispatch key)` | SB-01b | `main.rs` (wire shape, §2.7), `defs.json` (equals `pack_config.json` outputs), SB-W1 |
| `Ctx { ws: Option<Workspace>, now: String }`, the argument every module's `run` takes; `main.rs` fills `now` from the clock, tests pin it | SB-01b | every stage |

**`TaskFile` context variants** (field names snake_case; the Shelf struct each
mirrors is in brackets; a stage that needs more is a `SB-K<n>` change):

| Stage | Context fields |
| --- | --- |
| `metadata` | `pages: [{page_num, markdown}]` (the first 20 non-empty pages of the front prefix) [`prompts/metadata/workunit.go:Page`; `Input.BookText` is rendered from them] |
| `toc_finder` | `total_pages`, `book_title`, `previous_attempt?: {attempt_number, strategy, pages_checked, reasoning, structure_notes: {page: note}}` [`agents/toc_finder/prompt.go:UserPromptData`, `PreviousAttemptData`] |
| `toc_extract` | `toc_pages: [{page_num, markdown}]`, `structure_summary?` (from the finder) [`prompts/extract_toc/workunit.go:Input`] |
| `link` | `entry: {entry_ref, entry_number, title, level, level_name, printed_page_number}`, `total_pages`, `book_structure: {total_pages, back_matter_start, back_matter_types, target_is_back_matter, retry_hint?}` [`agents/toc_entry_finder/prompt.go:TocEntry`, `BookStructure`] |
| `pattern` | `linked_entries`, `candidates`, `candidates_truncated`, `detected_chapters`, `chapter_start_pages`, `body_start`, `body_end`, `total_pages` [`agents/pattern_analyzer/prompt.go:UserPromptData`] |
| `discover` | `entry: {level_name, identifier, heading_format, expected_near_page, search_range_start, search_range_end}`, `total_pages`, `excluded_ranges: [{start_page, end_page, reason}]` [`agents/chapter_finder/prompt.go:EntryToFind`, `ExcludedRange`] |
| `gap` | `gap: {gap_ref, start_page, end_page, size, prev_entry_title, prev_entry_page, next_entry_title, next_entry_page}`, `body_start`, `body_end`, `total_pages`, `linked_entries: [{entry_ref, title, level, level_name, actual_page}]` [`agents/gap_investigator/prompt.go:GapInfo`, `LinkedEntry`; `Key` → `gap_ref`, `DocID` → `entry_ref`] |
| `classify` | `chapter_ids: [..]` (≤64 per item) |
| `polish` | `chapter_id`, `audio_include: bool` (false → the item is a `skipped` outcome at release, §2.8) |
| deterministic (`commit`, ...) | `chapter_id`, `part` |

---

## 2. Pack `gents/structured_book`

Path: `packs/gents/structured_book/`

### 2.1 `manifest.json` (outline)

```jsonc
{
  "manifest_version": 1, "name": "structured_book", "namespace": "gents", "version": "0.1.0",
  "description": "Turns a PDF, image scans, an EPUB or a browser_download FetchedSource into a structured book: page OCR with image refs, metadata, a linked ToC, chapters and paragraphs with stable ids, a structure digest and an exact-quote validation tool. Reads text with gents/ocr.",
  "authors": ["gents contributors"], "tags": ["books","ocr","toc","structure","research"],
  "kind": "documents", "config": "pack_config.json", "dependencies": [],
  "schemas": [ /* the 30 files enumerated in §2.4 */ ],
  "inference_slots": [
    { "name": "book_reader", "description": "One-shot structured calls: metadata, ToC extraction, pattern analysis, structure classification, polish. Bind a profile with max_output_tokens >= 32768 and temperature 0.1.",
      "behaviors": ["sb-metadata","sb-toc-extract","sb-pattern-analyzer","sb-structure-classify","sb-structure-polish"] },
    { "name": "book_agent", "description": "Tool-using agents over the page corpus: ToC finder, ToC entry linker, chapter finder, gap investigator.",
      "behaviors": ["sb-toc-finder","sb-toc-entry-finder","sb-chapter-finder","sb-gap-investigator"] }
  ],
  "plugins": [
    { "name": "book_pipeline", "language": "rust", "source": "plugins/book", "artifact": "plugins/book_pipeline.afb",
      "instructions": "plugins/book/TOOL.pipeline.md",
      "description": "Deterministic stages of the structured_book pipeline; runs only as callbacks.",
      "input_schema": { "type": "object", "properties": {
        "workspace": { "type": "string" }, "workspace_original": { "type": "string" } } },
      "bind_dir": { "input_field": "workspace", "original_field": "workspace_original", "access": "read_write",
                    "description": "The book library (BookJob) or one book folder books/<sha256> below it" },
      "limits": { "memory_mib": 2048, "wall_clock_secs": 900, "max_output_mib": 4 } },
    { "name": "book_search", "language": "rust", "source": "plugins/book_tools", "artifact": "plugins/book_search.afb",
      "instructions": "plugins/book_tools/TOOL.search.md",
      "input_schema": { "type": "object", "required": ["op","book"], "properties": {
        "op": { "enum": ["get_frontmatter_grep_report","load_page","grep_text","get_heading_pages","get_page_ocr","check_candidate","get_gap_context"] },
        "book": { "type": "string" }, "task": { "type": "string" },
        "page_num": { "type": "integer" }, "previous_page": { "type": "integer" },
        "current_page_observations": { "type": "string" },
        "query": { "type": "string" }, "start_page": { "type": "integer" }, "end_page": { "type": "integer" } } },
      "bind_dir": { "input_field": "book", "access": "read", "description": "One run folder, <book folder>/runs/<run_id>" },
      "limits": { "memory_mib": 1024, "wall_clock_secs": 60, "max_output_mib": 1 } },
    { "name": "book_research", "language": "rust", "source": "plugins/book_tools", "artifact": "plugins/book_research.afb",
      "instructions": "plugins/book_tools/TOOL.research.md",
      "input_schema": { "type": "object", "required": ["op","book"], "properties": {
        "op": { "enum": ["get_book","list_structure","search_passages","read_passage","read_passage_at_version","validate_quote"] },
        "book": { "type": "string", "description": "<library>/books/<sha256> (BookStructure.canonical_path)" },
        "digest": { "type": "string", "description": "read_passage_at_version only" },
        "quote": { "type": "string" }, "query": { "type": "string" },
        "regex": { "type": "boolean" }, "case_sensitive": { "type": "boolean" },
        "chapter_id": { "type": "string" }, "paragraph_id": { "type": "string" }, "chapter_ids": { "type": "array" },
        "matter_type": { "type": "string" }, "offset": { "type": "integer" }, "limit": { "type": "integer" },
        "top_k": { "type": "integer" }, "snippet_chars": { "type": "integer" }, "start_char": { "type": "integer" },
        "context_before": { "type": "integer" }, "max_chars": { "type": "integer" },
        "expected_source_sha256": { "type": "string" }, "expected_structure_digest": { "type": "string" } } },
      "bind_dir": { "input_field": "book", "access": "read", "description": "One book folder below the library" },
      "limits": { "memory_mib": 1024, "wall_clock_secs": 60, "max_output_mib": 1 } }
  ],
  "assets": [ "README.md","pack_config.json","schemas/<30 files>",
              "agent_behaviors/<9 dirs>/system_prompt.md","tasks/<9 dirs>/prompt.md",
              "plugins/book_pipeline.afb","plugins/book_search.afb","plugins/book_research.afb",
              "plugins/book/TOOL.pipeline.md","plugins/book_tools/TOOL.search.md","plugins/book_tools/TOOL.research.md",
              "tests/install.json","tests/defs.json","tests/runtime_pages.json",
              "tests/fixtures/<each fixture file, literal: assets take no globs>" ]
}
```

- The two read-only model tools declare no `original_field`: they never pass
  the real path on, and it keeps a host-filled field out of the model's
  schema (F-15).
- No plugin declares `manifold`; file access is only through `bind_dir`.
  **Install needs `--grant-authority` anyway**, because all three entries
  declare `limits` (F-16). The same is true of `gents/ocr`.
- Pack-level fixtures under `tests/fixtures/` are listed one by one in
  `assets` (check fails on undeclared files outside plugin sources, and
  `pack.rs::declared_paths` takes no globs). SB-01a enumerates the fixture
  names it knows; SB-T1 may append to `assets` and to nothing else in the
  manifest.
- `book_search` and `book_research` share `plugins/book_tools` with identical
  `bind_dir` and `limits` (F-30); `book_pipeline` is alone in `plugins/book`.

### 2.2 `pack_config.json` (outline)

```jsonc
{
  "agent_principal": {},
  "agent_behaviors": [ /* 9 */ ],
  "contexts":        [ /* 9 */ ],
  "compactions":     [ { "compaction_id": "sb-agent-compaction", "threshold": 0.85 } ],
  "tools":           [ /* 9 */ ],
  "datastore_tool_surfaces": [ /* 9 */ ],
  "tasks":           [ /* 9: emit_outcome: true; no goal_objective_template, no goal_token_budget (F-32) */ ],
  "event_sources":   [ /* §2.6 */ ],
  "triggers":        [ /* 9 */ ],
  "callbacks":       [ /* 15, all max_attempts: 3 */ ],
  "callback_bindings": [ /* §2.6 */ ]
}
```

SB-01a commits this file on day one with the singletons (`agent_principal`,
`compactions`) **and one placeholder per slot behavior**: the 9 behaviors of
the §2.5 table, each with its final `behavior_id`, `context_id`, slot
reference and a one-line system prompt, plus their 9 contexts, because
`pack check` refuses a slot behavior that `agent_behaviors` lacks (F-28).
The other arrays start empty. Behavior ports and wiring write fragments
(`.build/fragments/<name>.json`, shape fixed by
`.build/fragments/_example.json`); `.build/merge.sh` merges each array
**by id** (`behavior_id`, `context_id`, `tools_id`, `surface_id`, `task_id`,
`event_source_id`, `trigger_id`, `callback_id`, `binding_id`): a fragment
entry replaces the placeholder with the same id, and a duplicate id across
fragments is an error. `gents pack check` skips dotfiles. Counts are asserted
by `defs.json` from the §2.5 and §2.6 tables, not from prose.

Agent Tasks are ordinary one-request Tasks. A `goal_objective_template`
without `enable_goal_tools` would never complete, and `goal_token_budget` is
refused by the packs harness (F-32). The agent loop cap is therefore the
runtime's per-request limits plus the batch timeout (D18); the README shows
how to opt in to a goal and a budget per Task after install.

### 2.3 Collections and correlation rules

- `run_id` is the book run correlation. **A book `run_id` starts with
  `sb-` and matches `^sb-[A-Za-z0-9._:-]+$`** (a single path segment, so
  `<workspace>/runs/<run_id>` and the 4-segment `task_ref` parse are safe).
  `ingest` refuses any other (a lone terminal BookStatus failed with a
  message, decision 4.4);
  `from_fetch` builds `sb-<FetchedSource.run_id>`. `BookJob.run_id` is reused
  as `OcrJob.run_id`, so every OCR record of ours has a `run_id` starting
  with `sb-`; `chunk_slot` and `chunk_text` drop every other one in plugin
  code (the sources cannot filter, F-29), so foreign OCR runs never enter
  the pipeline.
- The host owns each output port's correlation field and refuses a plugin
  value that differs (F-27). So:
  - Per-document callbacks whose outputs keep the source `run_id` use ports
    with `correlation_field: "run_id"`.
  - Grouped callbacks use `correlation_field: "cause_ref"`; the host writes
    the group key there and the plugin writes `run_id` itself. Every
    collection a grouped port writes declares `cause_ref` (`StageTask`,
    `ItemOutcome`, `BatchOutcome`, `StageStart`, `BookChunk`).
  - The two callbacks whose output `run_id` differs from the source's use a
    different port correlation: `sb-from-fetch` writes `BookJob` with
    `correlation_field: "fetch_run_ref"` (host writes the FetchedSource
    `run_id` there, the plugin writes `run_id = "sb-" + run_id`), and
    `sb-task-failed` writes `StageTask` with `correlation_field: "task_ref"`
    (host writes `FireOutcome.source_handoff_id`, which is the failed
    `task_ref`; the plugin derives `run_id` from it).
  - `defs.json` asserts, for every callback output port, that its
    `correlation_field` is a field of the port's collection in the producing
    SDL (F-27: nothing else checks it), and names the two exceptions above.
- Every plugin-written document carries `doc_kind`. Model-written documents
  carry `stage`, `task_ref` and `item_ref`.
- No `!` fields. JSON payloads stay `String` (F-14: the `JSON` scalar refuses
  `[]`, which empty edit and classification lists need).
- Unique refs (`@index(unique: true)`): `*Result.task_ref`,
  `StageTask.spawn_ref`, `Signal.signal_ref`, `ItemOutcome.outcome_ref`,
  `BatchOutcome.batch_ref`, `BookStatus.status_ref`, plus the existing
  `BookJob.run_id`. Every row sets its unique ref (no null values), so the
  design does not depend on how DefraDB treats nulls in a unique index.

### 2.4 Schemas (`schemas/*.graphql`)

**Entry, run and status**

```graphql
type BookJob {                       # entry: created by an operator or by sb-from-fetch
  run_id: String @index(unique: true) @immutable   # must start with "sb-"
  workspace: String                  # library root (absolute), an allowed read_write folder
  source_path: String                # file or image folder, inside the library
  source_paths: [String]             # more than one entry is refused in v1 (D12)
  source_format: String              # auto | pdf | images | epub
  source_sha256: String              # advisory; ingest always hashes and compares
  fetched_source_id: String
  fetch_run_ref: String              # host-written port correlation of sb-from-fetch (F-27); "-" or absent for operator jobs
  link_mode: String                  # copy (default) | hard_link (main >= 967339dfd only)
  title_hint: String
  author_hint: String
  variant: String                    # standard (default) | text-only | photo-book (alias of text-only) | ocr-only
  toc_policy: String                 # require (default, Shelf) | fallback | whole_book
  ocr_failure_policy: String         # fail (default, Shelf) | quarantine
  ocr: String                        # auto | always | never, passed to OcrJob
  remote_ocr: String                 # auto | off | force, passed to OcrJob
  force: Boolean
}
type BookRun { run_id: String @index  doc_kind: String  book_id: String @index  source_sha256: String @index
  source_format: String  source_filename: String  workspace: String  library_root: String  variant: String
  toc_policy: String  ocr_failure_policy: String  reused: Boolean  page_count: Int  chunks_total: Int
  fetched_source_id: String  created_at: String }
type BookStatus { run_id: String @index  doc_kind: String  cause_ref: String  book_id: String @index
  status_ref: String @index(unique: true)   # "<run_id>/terminal" on terminal rows, "<run_id>/progress/<writer_ref>" otherwise
  status: String @index  stage: String  reason: String  created_at: String
  toc_link_entries_total: Int  toc_link_entries_done: Int  finalize_entries_total: Int  finalize_entries_done: Int
  structure_chapters_total: Int  structure_chapters_done: Int }  # processing|complete|degraded|failed|reused
```

**OCR join and pages**

```graphql
type ChunkSlot { run_id: String @index  doc_kind: String  chunk_ref: String @index  slot: String   # path | text
  chunk: Int  workspace: String  source: String  pages: String  markdown: String  page_count: Int
  complete: Boolean  error: String  warnings: [String] }
type BookChunk { run_id: String @index  doc_kind: String  cause_ref: String  chunk_ref: String @index  chunk: Int
  workspace: String  source: String  page_lo: Int  page_hi: Int  page_count: Int  markdown: String
  complete: Boolean  error: String  warnings: [String] }
type BookPage { run_id: String @index  doc_kind: String  book_id: String @index  page_num: Int @index  chunk: Int
  markdown: String  headings: String  header: String  footer: String  figure_refs: [String]
  word_count: Int  quarantined: Boolean  quarantine_reason: String }
```

**Generic orchestration**

```graphql
type Signal { run_id: String @index  doc_kind: String  gate: String @index  gate_ref: String @index
  signal_ref: String @index(unique: true)  expected: Int  member: String  workspace: String  payload: String }
type StageStart { run_id: String @index  doc_kind: String  cause_ref: String  stage: String @index
  workspace: String  attempt: Int  payload: String }
type StageTask { run_id: String @index  doc_kind: String  cause_ref: String  stage: String @index  task_ref: String @index
  handoff_id: String                       # = task_ref on a StageTask (emit_outcome needs it, F-25); "-" on a TaskFailed marker
  spawn_ref: String @index(unique: true)   # task_ref for a StageTask, task_ref + "/failed" for a TaskFailed marker
  item_ref: String  attempt: Int  batch_ref: String  batch_size: Int  stage_ref: String  batches_total: Int
  workspace: String  user_prompt: String  feedback: String  payload: String
  terminal_state: String  reason: String }  # set only on doc_kind "TaskFailed"
type WorkItem { run_id: String @index  doc_kind: String  stage: String @index  item_ref: String @index
  batch_ref: String  batch_size: Int  stage_ref: String  batches_total: Int  workspace: String  payload: String }
type ItemOutcome { run_id: String @index  doc_kind: String  cause_ref: String  stage: String @index  item_ref: String
  outcome_ref: String @index(unique: true)
  batch_ref: String @index  batch_size: Int  stage_ref: String  batches_total: Int  workspace: String
  status: String  detail: String }        # opened | ok | not_found | failed | skipped
type BatchOutcome { run_id: String @index  doc_kind: String  cause_ref: String  stage: String @index
  batch_ref: String @index(unique: true)  stage_ref: String @index  batches_total: Int  workspace: String  counts: String }
```

Every `StageTask` sets the five surface fill sources (`run_id`, `stage`,
`task_ref`, `item_ref`, `workspace`) and `handoff_id` to non-empty strings,
because a null fill source stops the Task (F-13) and an empty `handoff_id`
refuses the fire (F-25); `user_prompt` is always set. The `"-"` rule covers
only those fields: `feedback` is empty or absent when there is none, so the
`{% if doc.feedback %}` block of the task template stays out of the prompt.

**Model results**

Every result collection carries `CTX` = `run_id`, `stage`,
`task_ref @index(unique: true)`, `item_ref`, `workspace` (all `String`, all
filled by the surface).

```graphql
type MetadataResult  { CTX  title: String  subtitle: String  authors: [String]  isbn: String  lccn: String
  publisher: String  publication_year: Int  language: String  description: String  subjects: [String]
  contributors_json: String  cover_page: Int  confidence: Float }
type TocFinderResult { CTX  toc_found: Boolean  start_page: Int  end_page: Int  confidence: Float
  search_strategy_used: String  reasoning: String  structure_summary_json: String
  pages_checked: Int  structure_notes_json: String }                               # C7: model-reported observations
type TocExtractResult { CTX  entries_json: String }
type TocLinkResult    { CTX  scan_page: Int  reasoning: String }  # Shelf's write_result {scan_page?, reasoning}; the entry comes from tasks/link/<item_ref>.json
type PatternResult    { CTX  discovered_patterns_json: String  excluded_page_ranges_json: String  reasoning: String }
type ChapterFindResult{ CTX  scan_page: Int  reasoning: String }
type GapFixResult     { CTX  fix_type: String  reasoning: String  scan_page: Int  title: String  level: Int
  level_name: String  entry_ref: String  new_scan_page: Int }                      # entry_doc_id -> entry_ref (C6)
type ClassifyResult   { CTX  classifications_json: String  content_types_json: String  audio_include_json: String  reasoning_json: String }
type PolishResult     { CTX  edits_json: String }
```

**Canonical records** (written only by `book_pipeline`, after validation)

```graphql
type BookMetadata { run_id: String @index  doc_kind: String  book_id: String @index  title: String  subtitle: String
  author: String  authors: [String]  isbn: String  lccn: String  publisher: String  publication_year: Int  language: String
  description: String  subjects: [String]  contributors_json: String  cover_page: Int  confidence: Float  source: String } # llm | epub; author = authors[0]
type Toc { run_id: String @index  doc_kind: String  book_id: String @index  toc_found: Boolean  start_page: Int
  end_page: Int  structure_summary_json: String  source: String  attempt: Int }   # finder | epub_navigation | none
type TocEntry { run_id: String @index  doc_kind: String  book_id: String  entry_ref: String @index
  entry_number: String  title: String  level: Int  level_name: String  printed_page_number: String
  source: String  sort_order: Int  revision: Int }      # extracted|discovered|validated|epub_navigation; latest revision wins
type TocLink { run_id: String @index  doc_kind: String  entry_ref: String @index  scan_page: Int  status: String
  evidence_json: String  attempt: Int  reason: String  revision: Int }
type PatternAnalysis { run_id: String @index  doc_kind: String  patterns_json: String  excluded_ranges_json: String
  entries_to_find: Int  rejected_json: String  reasoning: String }
type GapFix { run_id: String @index  doc_kind: String  gap_ref: String  fix_type: String  applied: Boolean  detail: String }
type Chapter { run_id: String @index  doc_kind: String  book_id: String @index  chapter_id: String @index  unique_ref: String @index
  sort_order: Int  title: String  level: Int  level_name: String  entry_number: String  entry_ref: String
  start_page: Int  end_page: Int  matter_type: String  classification_reasoning: String  content_type: String
  audio_include: Boolean  audio_include_reasoning: String  parent_id: String  source: String
  polished_text: String  edits_applied_json: String  word_count: Int
  extract_complete: Boolean  polish_complete: Boolean  polish_retries: Int  polish_failed: Boolean
  paragraph_pairing: String }   # one_to_one | positional (§2.8, Paragraphs and polish)
type Paragraph { run_id: String @index  doc_kind: String  book_id: String @index  chapter_id: String @index
  paragraph_id: String @index  sort_order: Int  start_page: Int  raw_text: String  polished_text: String
  word_count: Int  content_hash: String }
type BookStructure { run_id: String @index  doc_kind: String  cause_ref: String  book_id: String @index
  source_sha256: String @index  structure_digest: String @index  digest_version: String
  total_chapters: Int  total_paragraphs: Int  total_words: Int  canonical_path: String  status: String }  # canonical_path = the book folder
```

`Chapter.mechanical_text` stays in `chapters/<id>.json` and the canonical
file; the document carries only `polished_text` (keeps a commit under 4 MiB,
G12).

The 30 SDL files, one collection each: `book_job`, `book_run`, `book_status`,
`chunk_slot`, `book_chunk`, `book_page`, `signal`, `stage_start`,
`stage_task`, `work_item`, `item_outcome`, `batch_outcome`,
`metadata_result`, `toc_finder_result`, `toc_extract_result`,
`toc_link_result`, `pattern_result`, `chapter_find_result`,
`gap_fix_result`, `classify_result`, `polish_result`, `book_metadata`, `toc`,
`toc_entry`, `toc_link`, `pattern_analysis`, `gap_fix`, `chapter`,
`paragraph`, `book_structure` (each `.graphql`).

Consumers of canonical records filter by the `run_id` of the latest
`BookStructure{status:"complete"}` for a `book_id`; forced re-runs add rows
and never delete old ones. `book_research` reads only the canonical file.

Not ported: `AgentState`, `AgentRun`, `LLMCall`, `Metric`, `Job`, `Config`,
`Prompt`, `BookPromptOverride`, `Audio*`, `Voice`, `OcrResult`.

### 2.5 Behaviors, tools and surfaces

Shelf root: `S = shelf/internal`.

**Stage table** (in `core/contract.rs`; every id below is derived from it,
and `defs.json` asserts it):

| dir | behavior_id | stage | slot | task_id | event source = trigger id | tools_id | surface_id | write tool | Result collection | result binding |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `metadata` | `sb-metadata` | `metadata` | book_reader | `sb-metadata-task` | `sb-task-metadata` | `sb-metadata-tools` | `sb-metadata-writes` | `write_book_metadata` | MetadataResult | `sb-r-metadata` |
| `toc_finder` | `sb-toc-finder` | `toc_finder` | book_agent | `sb-toc_finder-task` | `sb-task-toc_finder` | `sb-toc_finder-tools` | `sb-toc_finder-writes` | `write_toc_result` | TocFinderResult | `sb-r-toc_finder` |
| `toc_extract` | `sb-toc-extract` | `toc_extract` | book_reader | `sb-toc_extract-task` | `sb-task-toc_extract` | `sb-toc_extract-tools` | `sb-toc_extract-writes` | `write_toc_extraction` | TocExtractResult | `sb-r-toc_extract` |
| `toc_entry_finder` | `sb-toc-entry-finder` | `link` | book_agent | `sb-link-task` | `sb-task-link` | `sb-link-tools` | `sb-link-writes` | `write_result` | TocLinkResult | `sb-r-link` |
| `pattern_analyzer` | `sb-pattern-analyzer` | `pattern` | book_reader | `sb-pattern-task` | `sb-task-pattern` | `sb-pattern-tools` | `sb-pattern-writes` | `write_pattern_analysis` | PatternResult | `sb-r-pattern` |
| `chapter_finder` | `sb-chapter-finder` | `discover` | book_agent | `sb-discover-task` | `sb-task-discover` | `sb-discover-tools` | `sb-discover-writes` | `write_result` | ChapterFindResult | `sb-r-discover` |
| `gap_investigator` | `sb-gap-investigator` | `gap` | book_agent | `sb-gap-task` | `sb-task-gap` | `sb-gap-tools` | `sb-gap-writes` | `write_fix` | GapFixResult | `sb-r-gap` |
| `structure_classify` | `sb-structure-classify` | `classify` | book_reader | `sb-classify-task` | `sb-task-classify` | `sb-classify-tools` | `sb-classify-writes` | `write_entry_classifications` | ClassifyResult | `sb-r-classify` |
| `structure_polish` | `sb-structure-polish` | `polish` | book_reader | `sb-polish-task` | `sb-task-polish` | `sb-polish-tools` | `sb-polish-writes` | `write_text_edits` | PolishResult | `sb-r-polish` |

**Shelf sources per behavior** (unchanged from round 0):

| behavior_id | Shelf system prompt | Shelf user prompt, ported to plugin | Tools | Shelf output contract |
| --- | --- | --- | --- | --- |
| `sb-metadata` | `S/prompts/metadata/system.tmpl` | `S/prompts/metadata/user.tmpl` + `workunit.go:PrepareBookText` → `core/prompts_front.rs` | none | `metadata/schema.go:ExtractionSchema` |
| `sb-toc-finder` | `S/agents/toc_finder/system.tmpl` | `S/agents/toc_finder/user.tmpl` (PreviousAttempt wired) → `prompts_front.rs` | `book_search` `get_frontmatter_grep_report`, `load_page` | `toc_finder/tools/write_toc_result.go`, `schema.go` |
| `sb-toc-extract` | `S/prompts/extract_toc/system.tmpl` | `S/prompts/extract_toc/user.tmpl` → `prompts_front.rs` | none | `extract_toc/schema.go` |
| `sb-toc-entry-finder` | `S/agents/toc_entry_finder/system.tmpl` | `prompt.go:BuildUserPrompt` → `prompts_finalize.rs` | `book_search` `get_heading_pages`, `grep_text`, `get_page_ocr`, `check_candidate` | `toc_entry_finder/schema.go`, `tools/write_result.go` |
| `sb-pattern-analyzer` | `S/agents/pattern_analyzer/system.tmpl` | `user.tmpl` + `prompt.go:BuildUserPrompt` → `prompts_finalize.rs` | none | `pattern_analyzer/prompt.go:JSONSchema` |
| `sb-chapter-finder` | `S/agents/chapter_finder/system.tmpl` | `prompt.go:BuildUserPrompt` → `prompts_finalize.rs` | `book_search` `get_heading_pages`, `grep_text`, `get_page_ocr` | `chapter_finder/tools/write_result.go` |
| `sb-gap-investigator` | `S/agents/gap_investigator/system.tmpl` | `prompt.go:BuildUserPrompt` → `prompts_finalize.rs` | `book_search` `get_gap_context`, `get_page_ocr` | `gap_investigator/tools/write_fix.go` |
| `sb-structure-classify` | `S/jobs/common/structure_prompts.go:ClassifySystemPrompt` | `structure_classify.go:BuildClassifyPrompt` → `prompts_structure.rs` | none | `structure_prompts.go:ClassifyJSONSchema` |
| `sb-structure-polish` | `S/jobs/common/structure_prompts.go:PolishSystemPrompt` | `structure_prompts.go:BuildPolishPrompt` → `prompts_structure.rs` | none | `structure_prompts.go:PolishJSONSchema` |

**Surface entries** (`sb-<stage>-writes`): one create tool; fields
`run_id {fill: correlation}` and `stage`, `task_ref`, `item_ref`,
`workspace`, each `{fill: {source_field: <same>}}`; then the contract fields
(Shelf-required ones `required: true`, except `TocLinkResult.scan_page`,
which stays optional as in Shelf); `output_obligation: {scope:"trigger",
minimum_writes:1}`.

**Tools documents** (`sb-<stage>-tools`): `built_ins` with only
`enable_session_history_tool`; `datastore.datastore_tool_surface_ids:
["sb-<stage>-writes"]`; agents add `integrations.plugins:
[{"plugin":"gents/book_search"}]`. No host files or bash.

**Triggers** (`sb-task-<stage>`): event source on `StageTask` with
`filter: {stage: {_eq: "<stage>"}, doc_kind: {_eq: "StageTask"}}` (our own
collection, so `pack check` can run it, F-29), correlation `run_id`;
`concurrency: "parallel"`; **no `session_id_template`** (F-26: it must name
an existing session; without it each fire gets a fresh session derived from
trigger + StageTask `_docID`). Tasks set `emit_outcome: true`, which
`StageTask.handoff_id` satisfies (F-25). No Task sets
`goal_objective_template` or `goal_token_budget` (F-32, D18).

**Task template** (`tasks/<dir>/prompt.md`; the tool name is written out per
file, never a template expression):

```
{{ doc.user_prompt }}
{% if doc.feedback %}

{{ doc.feedback }}
{% endif %}
---
Book folder (pass as "book" to book_search): {{ doc.workspace }}/runs/{{ doc.run_id }}
Task file (pass as "task" to book_search): tasks/{{ doc.stage }}/{{ doc.item_ref }}.json
Finish by calling write_result exactly once.
```

One-shots drop the two `book_search` lines. The rendered prompt must stay
under 1 MiB (F-44); SB-C1's fan-out helper refuses an item whose
`prompt_bytes` is above 1,000,000 (room for the footer and feedback) with
`ItemOutcome{failed: "prompt <n> bytes exceeds the 1 MiB task prompt cap"}`.
**Retry text goes in exactly one place per stage:**
- link puts **all** retry text inside `user_prompt`, as Shelf does, and
  leaves `feedback` empty. That covers content rejects, schema repairs and
  Task failures, all through `appendLinkTocRetryHint` (for a Task failure,
  the hint is the `FireOutcome.reason`, as Shelf's
  `createLinkTocRetryUnit(ctx, info, linkTocWorkFailureReason(result))`,
  `job.go:349-352`).
- toc_finder puts content and schema-repair text inside `user_prompt` as
  `PreviousAttempt` (D7) and leaves `feedback` empty. A **Task-failure**
  retry reuses the same `user_prompt` unchanged, as Shelf, which passes
  `nil` (`createBookOpRetryUnit`).
- Every other stage leaves `user_prompt` unchanged and puts the text (schema
  repair, port-new validators) in `feedback`; a Task-failure retry adds
  nothing.

S5, S6 and S8b ship a "reject → retry text, and where it goes" table; SB-C7
adds a golden for the rendered link retry prompt.

**Every change to Shelf prompt wording** (recorded in `.build/notes/<dir>.md`,
merged into the README's "Changes from Shelf"):

| # | Prompt | Change | Why |
| --- | --- | --- | --- |
| C1 | all 9 | A final `## Output` section names the write tool and lists its fields with types and enums from Shelf's schema. "Respond with JSON matching..." becomes "call `<tool>` once with...". Nested values become one JSON-string field | No enforced response_format; results come back through a surface create, validated afterwards |
| C2 | toc_finder, toc_entry_finder, chapter_finder, gap_investigator | `name(args)` becomes `book_search` with `op: "<name>"`, keeping Shelf's op names; one sentence says to pass `book` and `task` from the footer | One tool per plugin |
| C3 | same four | Remove `load_page_image`; toc_finder's `load_page_image` + `load_ocr_text` become `load_page`; "look at the image" wording becomes "read the page text" | No vision on the GLM wire |
| C4 | metadata | Delete the "with web search capability" and "Use web search to verify" sentences | The tool never existed (inventory Finding 7) |
| C5 | toc_entry_finder | The "write_result rejects..." sentences say the result is checked after the run and a reject comes back as feedback; the agent is told to call `check_candidate` (or `get_page_ocr` and look for `write_result_ready: true`) before writing | The in-tool reject loop becomes a pre-check inside the session plus post-hoc validation |
| C6 | gap_investigator | `entry_doc_id` becomes `entry_ref` | Stable refs replace doc ids; `*_key` names are stripped by gents (F-12) |
| C7 | toc_finder | `load_page` takes `page_num`, `previous_page` and `current_page_observations`, and refuses with Shelf's message when `previous_page` is set and the observations are empty. `write_toc_result` gains `pages_checked` and `structure_notes_json` (`{page: note}`), which the model fills from its own calls | A sealed, read-only plugin keeps no state between calls |
| C8 | all 9 | The task footer (book folder, task file, "Finish by calling `<tool>` exactly once") | Tool arguments the plugin needs |

The Go-const prompts (classify, polish) are copied as raw text. Nothing else
in any system prompt changes. `.build/check-prompt.sh` (SB-01c) enforces this.

**Behavior changes that are not prompt wording** (also in the README):

| # | Change |
| --- | --- |
| D1 | `toc_policy: fallback` and `whole_book` are opt-in; the default `require` is Shelf |
| D2 | `ocr_failure_policy: quarantine` is opt-in; the default `fail` is Shelf |
| D3 | No per-book cap of 8 link agents; at most one released batch (32) per stage per book |
| D4 | The link reject loop: Shelf rejects inside the tool for up to 25 iterations, then up to 3 fresh agents. Here `check_candidate` gives the same reason inside the session, and each post-hoc reject uses one of 3 attempts |
| D5 | Agent context: Shelf strips images and compacts old tool results on every request; here results stay until the 0.85 compaction. The e2e run records peak context length |
| D6 | Sampling: the README asks for a `book_reader` profile with temperature 0.1 and `max_output_tokens >= 32768`. Pattern, classify and polish then also run at 0.1 (Shelf left them at the provider default) |
| D7 | toc_finder gets a `1 <= start <= end <= N` range check, and its retry carries `PreviousAttempt` (Shelf passed `nil`). The golden is rendered from `user.tmpl` with data |
| D8 | Metadata and toc_finder start when the front gate holds the exact prefix 1..min(30,N), as Shelf's `ConsecutivePagesComplete` (no partial front under `fail`; under `quarantine` only quarantined holes). Metadata then reads the first 20 non-empty pages **of that prefix**. Shelf reads the first 20 non-empty pages it has; that is the same set unless pages 1..30 hold fewer than 20 non-empty pages (blank versos, plates), where Shelf would also read later pages already OCR'd and this design reads fewer |
| D9 | Progress counters live on `BookStatus` rows, not on the book |
| D10 | EPUB import writes no per-chapter page rows |
| D11 | Digest ids are `bk_`/`ch_` ids, not DefraDB doc ids; Shelf citations do not carry over |
| D12 | Multi-PDF ingest and stitching are refused with a message (`source_paths` of more than one entry) |
| D13 | `reuseUnchangedPolish` is not ported; a forced re-run polishes again |
| D14 | Task failures are retried through `FireOutcome` (or, if R17 fails, a batch timeout counts as an exhausted budget). Shelf retries metadata, toc_finder and toc_extract provider errors only when `jobs.IsRetriableError` holds (`job.go:312-345`); here every non-success `terminal_state` is retried within the budget |
| D15 | Paragraphs are new: scans get per-paragraph passages |
| D16 | `photo-book` is accepted as an alias of `text-only` (same flags in Shelf) |
| D17 | Pattern failures after 3 attempts skip to discover with 0 entries, so validate and gap are skipped, as Shelf |
| D18 | Agent loop caps: Shelf's `MaxIterations` (25, 25, 15, 20)/`RequireToolUse`/`MaxToolCallsPerTurn` have no pack equivalent; an agent Task is one ordinary request, capped by the runtime's per-request limits and the batch timeout (4 h). The README shows how to opt in to `goal_objective_template` + `enable_goal_tools` + `goal_token_budget` per Task after install |
| D19 | Book `run_id`s start with `sb-` |
| D20 | Shelf's agent loop knobs and per-call params that a pack cannot author are listed in the README |
| D21 | A wall-clock limit Shelf does not have: the finish watchdog fails a book 72 h after ingest ("pipeline did not finish"), and a late Signal cannot complete it afterwards. The timeout is static (F-34), so it cannot follow `page_count`; the README gives the per-book estimate (serial callbacks, about 15 min per OCR chunk) and shows how to raise `sb-gate-finish.timeout_secs` after install |
| D22 | `OcrDocument{complete:false}` is a port-only failure mode with two causes (F-45): the 1.5 MB byte cap (`EXTRACT_MAX_BYTES`, F-9) and `gents/ocr`'s own call time budget, after which it returns a cursor the pack never follows. Under `fail` the reason is taken from the document's `error`/`warnings`: `"ocr chunk <c> incomplete (<cause>): pages <x>-<b> missing"`, `<cause>` being "1.5 MB cap", "OCR time budget" or the first warning. X-00a measures how often `complete:false` occurs on the e2e PDF with the operator's OCR profile |
| D23 | Research reads after a failed forced re-run keep serving the last good `canonical/book.json` (with `version_match` against it); Shelf's `requireResearchReady` refuses while `structure_failed`. A failed run writes `books/<sha>/latest_run.json{run_id, status, reason}`, and `get_book` reports it as `status_reason` |
| D24 | Schema repair: 2 repairs per attempt, each a fresh Task with the repair text, where Shelf repairs inside the same provider conversation |
| D25 | ingest refuses a source whose pages it cannot count (the OCR plan would emit one page-less chunk), and a book fails when `gents/ocr`'s plan disagrees with ingest's count (§2.8 "OCR plan agreement"); Shelf rasterised and counted its own pages, so it had no such case |
| D26 | Image folders are copied flat and renamed `NNNN.<ext>` in ingest's natural (numeric-aware) name order; only files ingest recognises as images are copied. Shelf likewise numbered the pages it rasterised itself |
| D27 | A model item whose rendered prompt exceeds 1,000,000 bytes fails (F-44: gents refuses a rendered Task prompt above 1 MiB); Shelf would have sent it |

### 2.6 Wiring: event sources, callbacks, triggers

All callbacks use plugin `book_pipeline` with `digest: ""` and
`max_attempts: 3`. "Bound" means the source doc carries `workspace`.
`input_fields` always list `doc_kind` where the source has it, and never a
secret-bearing name.

| Event source → binding → callback | Source collection (filter) | correlation / group | Bound | Outputs (port: collection, cardinality, corr) |
| --- | --- | --- | --- | --- |
| `sb-job` → `sb-ingest` | BookJob | run_id | yes (library) | BookRun one; OcrJob one (optional); StageStart many; Signal many (finish opener); BookStatus many |
| `sb-fetched` → `sb-from-fetch` | FetchedSource (**unfiltered**, F-29; the plugin returns no output unless `structure == true` and `status ∈ {ok, duplicate}`) | run_id | no | BookJob one (optional), **corr `fetch_run_ref`** (F-27; the plugin sets `run_id = "sb-" + run_id`) |
| `sb-ocr-chunk` → `sb-chunk-slot` | OcrChunk (**unfiltered**; no output unless `run_id` starts with `sb-`) | run_id | no | ChunkSlot one (optional) |
| `sb-ocr-doc` → `sb-chunk-text` | OcrDocument (**unfiltered**; same rule) | run_id | no | ChunkSlot one (optional) |
| `sb-ocr-plan` → `sb-plan-check` | OcrChunk (**unfiltered**; the plugin returns `null` unless every member's `run_id` starts with `sb-`) | run_id; group `{timeout_secs: 1800, min_count: 1}`, **no `expected_count`** (F-43): all OcrChunks of a job arrive in one transaction (F-42), so at the timeout the group holds the whole plan | no (array) | StageStart one (optional), corr cause_ref: `{stage:"ocr_plan", run_id, workspace (three levels up from path), payload:{chunks:[{chunk, pages, source}]}}` |
| `sb-chunk-join` → `sb-chunk-join` | ChunkSlot | chunk_ref; group `{expected_count: 2, timeout_secs: 86400, min_count: 1}` | no (array) | BookChunk one (optional), corr cause_ref |
| `sb-chunk` → `sb-page-assemble` | BookChunk | run_id | yes | BookPage many; Signal many; BookStatus many |
| `sb-gate-front`, `sb-gate-ocr`, `sb-gate-link`, `sb-gate-finish` → `sb-gate-open` | Signal (`gate: {_eq: "<gate>"}`) | gate_ref; group `{expected_count:{source_field:"expected"}, timeout_secs: 259200, min_count: 1}` | no (array) | StageStart one, corr cause_ref |
| `sb-stage` → `sb-stage` | StageStart | run_id | yes | StageTask, WorkItem, Signal, StageStart, ItemOutcome, BatchOutcome, Toc, TocEntry, TocLink, PatternAnalysis, BookMetadata, BookStructure, BookStatus (all many) |
| `sb-work` → `sb-work` | WorkItem | run_id | yes | same set as `sb-stage` plus Chapter many, Paragraph many |
| `sb-r-<stage>` ×9 → `sb-result` | each `*Result` collection | run_id | yes | StageTask, Signal, StageStart, ItemOutcome, BookMetadata, Toc, TocEntry, TocLink, PatternAnalysis, GapFix, BookStatus (many) |
| `sb-items` → `sb-batch-close` | ItemOutcome | batch_ref; group `{expected_count:{source_field:"batch_size"}, timeout_secs: 14400, min_count: 1}` | no (array) | BatchOutcome one; StageStart one (`<stage>_next`, payload lists the members); corr cause_ref |
| `sb-batches` → `sb-stage-close` | BatchOutcome | stage_ref; group `{expected_count:{source_field:"batches_total"}, timeout_secs: 259200, min_count: 1}` | no (array) | StageStart one (`<stage>_closed`), corr cause_ref |
| `sb-fire` → `sb-task-failed` (R17) | FireOutcome (`trigger_id: {_like: "sb-task-%"}`; a runtime collection, so `pack check` can run the filter, F-29). Success rows: the plugin returns no output | **`source_handoff_id`** (event source and binding) | no | StageTask one (optional), **corr `task_ref`**: doc_kind `TaskFailed`, `stage: "task_failed"`, `spawn_ref = task_ref + "/failed"`, `handoff_id: "-"`, `run_id` derived from the task_ref, `terminal_state`, `reason`, `attempt` |
| `sb-task-join` → `sb-task-retry` (R17) | StageTask | task_ref; group `{expected_count: 2}` (no timeout; R23 decides whether to add `timeout_secs: 259200, min_count: 2`, which leaves a lone member dormant, F-34) | no (array) | StageStart one (optional; none when the group holds no `TaskFailed`), corr cause_ref: `{stage: "task_retry", workspace, payload: {stage, item_ref, task_ref, terminal_state, reason, attempt}}`, copied from the two members. The **bound** `stage:task_retry` handler then decides with the files in hand: a `results/<stage>/<item_ref>.json` already exists, or the task file is `closed` → nothing (the result won the race); the failed `task_ref` is **not the newest** entry of the task file's attempt history (a schema repair or content retry already superseded it) → nothing; budget left → StageTask (`attempt + 1`, repair 0, new `task_ref` = `handoff_id`, same batch fields, same `user_prompt` except for link, which appends the `reason` through C7's `appendLinkTocRetryHint` port); budget used → ItemOutcome `failed: task <terminal_state>: <reason>` |

`sb-task-failed` lists `source_handoff_id`, `trigger_id`, `terminal_state`,
`reason`, `attempt` and never `fire_key` (F-12). The `FireOutcome` row
carries no `workspace`, which is why the retry is a join with the original
`StageTask` (which carries everything) and not a bound handler.
`FireOutcome.attempt` equals the `StageTask.attempt` it came from, because
admission copies the doc's integer `attempt` (F-7).

**Fallback for R17/W-6.** If R17 says no, the batch timeout is the only
Task-failure signal. Then `sb-items` is split by filter (our own collection):
`sb-items-single` (`stage: {_in: ["metadata","toc_finder","toc_extract","pattern"]}`,
`timeout_secs: 1800`) and `sb-items` (every other stage, 14,400 s), both
bound to `sb-batch-close`; `sb-fire`, `sb-task-join`, `task_failed` and
`task_retry` are removed.

Counts: 26 pipeline event sources and bindings (27 with the fallback
split), 9 trigger event sources, 9 triggers, 15 callbacks (`ingest`,
`from_fetch`, `chunk_slot`, `chunk_text`, `plan_check`, `chunk_join`,
`page_assemble`, `gate_open`, `stage`, `work`, `result`, `batch_close`,
`stage_close`, `task_failed`, `task_retry`).

**`input_fields` per binding.** A binding projects exactly its
`input_fields` (`callback/scan.rs:644-668, 846-906`; an empty list passes
`{}`); a bound binding that omits `workspace` runs unbound (F-3 path,
`plugin/executor.rs:220-222`), and a name the source collection lacks stalls
that binding's arrival cursor for good (`scan.rs:389-390`). So every binding
lists exactly:

| Binding | `input_fields` |
| --- | --- |
| `sb-ingest` | `run_id`, `workspace`, `workspace_original`, `source_path`, `source_paths`, `source_format`, `source_sha256`, `fetched_source_id`, `link_mode`, `title_hint`, `author_hint`, `variant`, `toc_policy`, `ocr_failure_policy`, `ocr`, `remote_ocr`, `force` |
| `sb-from-fetch` | `run_id`, `source_id`, `status`, `structure`, `library_root`, `path`, `sha256`, `mime`, `title`, `authors`, `year`, `license` |
| `sb-chunk-slot`, `sb-plan-check` | `run_id`, `chunk`, `path`, `source`, `pages`, `format` |
| `sb-chunk-text` | `run_id`, `chunk`, `source`, `markdown`, `page_count`, `complete`, `error`, `warnings` |
| `sb-chunk-join` | `doc_kind`, `run_id`, `chunk_ref`, `slot`, `chunk`, `workspace`, `source`, `pages`, `markdown`, `page_count`, `complete`, `error`, `warnings` |
| `sb-page-assemble` | `doc_kind`, `run_id`, `workspace`, `workspace_original`, `chunk_ref`, `chunk`, `source`, `page_lo`, `page_hi`, `page_count`, `markdown`, `complete`, `error`, `warnings` |
| `sb-gate-*` | `doc_kind`, `run_id`, `gate`, `gate_ref`, `signal_ref`, `expected`, `member`, `workspace`, `payload` |
| `sb-stage` | `doc_kind`, `run_id`, `stage`, `workspace`, `workspace_original`, `attempt`, `payload` |
| `sb-work` | `doc_kind`, `run_id`, `stage`, `item_ref`, `batch_ref`, `batch_size`, `stage_ref`, `batches_total`, `workspace`, `workspace_original`, `payload` |
| `sb-r-<stage>` | the 5 CTX fields plus that collection's contract fields (§2.4) |
| `sb-items*` | `doc_kind`, `run_id`, `stage`, `item_ref`, `outcome_ref`, `batch_ref`, `batch_size`, `stage_ref`, `batches_total`, `workspace`, `status`, `detail` |
| `sb-batches` | `doc_kind`, `run_id`, `stage`, `batch_ref`, `stage_ref`, `batches_total`, `workspace`, `counts` |
| `sb-fire` | `source_handoff_id`, `trigger_id`, `terminal_state`, `reason`, `attempt` (never `fire_key`, F-12) |
| `sb-task-join` | `doc_kind`, `run_id`, `stage`, `task_ref`, `item_ref`, `attempt`, `batch_ref`, `batch_size`, `stage_ref`, `batches_total`, `workspace`, `terminal_state`, `reason` |

`defs.json` asserts that every bound binding lists `workspace`, every
grouped binding lists `doc_kind` (where its collection has it) plus the
fields its handler reads, and every name exists in the source collection's
SDL (ours, or the producing pack's for the four foreign sources).

### 2.7 `book_pipeline` dispatch contract (fixed by the scaffold)

```rust
// source/main.rs (scaffold): read stdin -> Value; ctx = Ctx { ws: Workspace::from_input(&v) (None when unbound), now }
// key = dispatch_key(&v):
//   Object with "op"                                  -> "tool:<op>"  (book_search / book_research ops; never pipeline ops)
//   Array of objects without doc_kind, with chunk+path -> "plan_check" (OcrChunk projections)
//   Array of doc_kind "ChunkSlot"                     -> "chunk_join"
//   Array of doc_kind "Signal"                        -> "gate_open"
//   Array of doc_kind "ItemOutcome"                   -> "batch_close"
//   Array of doc_kind "BatchOutcome"                  -> "stage_close"
//   Array of doc_kind "StageTask"/"TaskFailed"        -> "task_retry"
//   Object doc_kind "BookChunk"                       -> "page_assemble"
//   Object doc_kind "StageStart"                      -> "stage:<stage>"
//   Object doc_kind "WorkItem"                        -> "work:<stage>"
//   Object without doc_kind, with task_ref and stage  -> "result:<stage>"
//   Object with source_handoff_id and terminal_state  -> "task_failed"   (FireOutcome projection)
//   Object with source_path or source_paths           -> "ingest"
//   Object with source_id                             -> "from_fetch"
//   Object with chunk and path, no markdown           -> "chunk_slot"
//   Object with chunk and (markdown|complete|error)   -> "chunk_text"
// Each module: pub fn run(key: &str, input: &Value, ctx: &Ctx) -> Result<Value, String>
// returning an object keyed by the port-name consts in core/contract.rs (absent key = no output).
// main.rs then puts it in the WIRE SHAPE (F-38), from contract::CALLBACK_PORTS[callback_of(key)]:
//   callback with ONE port   -> print keyed[port] itself (the document, or the array for `many`),
//                               or JSON `null` when the key is absent ("no output"; never `{}`)
//   callback with SEVERAL    -> print the keyed object, absent keys left out
// A key whose module returns a port the callback does not have is a panic in tests (dispatch test).
```

Single-port callbacks (wire value = the document itself): `sb-from-fetch`
(BookJob), `sb-chunk-slot`, `sb-chunk-text` (ChunkSlot), `sb-plan-check`,
`sb-gate-open`, `sb-stage-close`, `sb-task-retry` (StageStart),
`sb-chunk-join` (BookChunk), `sb-task-failed` (StageTask). `sb-task-failed`
must print `null` for a success `FireOutcome`: `{}` would write an empty
`StageTask` that joins the original, fills the 2-member group and retries a
Task that succeeded. Several ports: `sb-ingest`, `sb-page-assemble`,
`sb-stage`, `sb-work`, `sb-result`, `sb-batch-close`. SB-01b's dispatch test
covers both shapes, and `defs.json` asserts that `CALLBACK_PORTS` equals the
`outputs` of each callback in `pack_config.json`.

| Module (one task owns each file) | keys |
| --- | --- |
| `stages/ingest.rs` | `ingest`, `from_fetch` |
| `stages/ocr_join.rs` | `chunk_slot`, `chunk_text`, `chunk_join`, `plan_check`, `stage:ocr_plan` |
| `stages/pages.rs` | `page_assemble` |
| `stages/gates.rs` | `gate_open`, `batch_close`, `stage_close`, `task_failed`, `task_retry`, `stage:task_retry`, `stage:front`, `stage:ocr`, `stage:link_gate`, `stage:finish`, `stage:<any>_next` (generic batch release from the stored `TaskFile.user_prompt`, plus the `closed` marks) |
| `stages/metadata.rs` | `stage:metadata`, `result:metadata`, `stage:metadata_closed` |
| `stages/toc.rs` | `stage:toc_finder`, `result:toc_finder`, `stage:toc_finder_closed` (releases the 1-item toc_extract stage through the C1 fan-out helper: task file, item opener, batch and stage openers), `result:toc_extract`, `stage:toc_extract_closed` |
| `stages/link.rs` | `stage:link`, `result:link`, `stage:link_closed` |
| `stages/finalize_pattern.rs` | `stage:pattern`, `result:pattern`, `stage:pattern_closed` (writes `pattern.json` and a `StageStart{discover}` or `StageStart{gap_closed}`) |
| `stages/finalize_gaps.rs` | `stage:discover` (renders and releases the discover items), `result:discover`, `stage:discover_closed`, `result:gap`, `stage:gap_closed` (sole owner of the zero-entry `toc_policy` branch) |
| `stages/structure_build.rs` | `stage:structure` (skeleton, extract, merge; reads `toc/final.json`, never branches on policy; ends with a `StageStart{classify}`) |
| `stages/structure_llm.rs` | `stage:classify` (renders and releases classify items), `result:classify`, `stage:classify_closed`, `result:polish`, `stage:polish_closed` |
| `stages/canonical.rs` | `work:commit`, `stage:commit_closed` |
| `stages/epub_import.rs` | `stage:epub_import` |
| `book_tools`: `tools/search.rs` (SB-P1) | `tool:get_frontmatter_grep_report`, `tool:load_page`, `tool:grep_text`, `tool:get_heading_pages`, `tool:get_page_ocr`, `tool:check_candidate`, `tool:get_gap_context` |
| `book_tools`: `tools/research.rs` (SB-P2) | `tool:get_book`, `tool:list_structure`, `tool:search_passages`, `tool:read_passage`, `tool:read_passage_at_version`, `tool:validate_quote` |

The gate named `link` and the model stage named `link` share a name; the gate
handler key is `stage:link_gate` (the `gate_open` output maps gate `link` to
stage `link_gate`).

`plugins/book_tools/source/main.rs` (SB-01a) dispatches only `tool:<op>` and
refuses anything else; `plugins/book/source/main.rs` refuses any input with
`op`. `core/` is authored in `plugins/book/source/core/` and copied into
`plugins/book_tools/source/core/` by `plugins/book_tools/sync-core.sh`; the
copy is committed together with the original (decision 9).

### 2.8 Stage diagram

```
                      browser_download: FetchedSource(status ok|duplicate, structure=true)
                                   |  sb-from-fetch (unbound) ─► BookJob{run_id:"sb-"+run_id}
 operator ── BookJob ──────────────┴─► sb-ingest [bound library]
       run_id not ^sb-[A-Za-z0-9._:-]+$ | >1 source path | image folder >256 images | page count fails
            ─► lone BookStatus{failed, reason} (no marker, no finish opener: no run folder yet, decision 4.4)
       hash (always) and compare source_sha256; copy (or, link_mode=hard_link, hard-link) source into
       books/<sha>/runs/<run_id>/ (images: only the counted files, flat, NNNN.<ext>); page_count via ocrcount
       (gents/ocr's own counter, decision 9), ranges via ocrcount::ranges; run.json
       ├─ canonical exists && !force ─► BookRun{reused} + BookStatus{reused, status_ref <run>/terminal} + status.json   (stop)
       ├─ always: finish_signal("opener"), expected = RunFile.finish_expected = members(format, variant)+1   (watchdog, 72 h)
       ├─ epub ─► StageStart{epub_import} ─► Toc, TocEntry, chapters/*.json, BookMetadata{epub}
       │          + Signal{finish, metadata} ─► WorkItem{commit}×N ...(join at COMMIT)
       └─ pdf | images ─► BookRun + OcrJob{run_id, path, files (images: RunFile.files, in order)}  (ocr-only/text-only/standard all OCR)
   gents/ocr: OcrJob ─► ocr-plan ─► OcrChunk×C ─► ocr-extract ─► OcrDocument (+ OcrPage, OcrFigure)
   OcrChunk×C ─(group run_id, timeout 30 min, no count)─► sb-plan-check ─► StageStart{ocr_plan} ─► stage:ocr_plan [bound]:
                C == chunks_total and every (chunk, pages, source) == RunFile ─► nothing;
                else ─► terminal(failed, "gents/ocr planned <C> chunks (<first difference>); ingest counted <chunks_total>")
   OcrChunk ─► sb-chunk-slot ─► ChunkSlot{path}  ┐
   OcrDocument ─► sb-chunk-text ─► ChunkSlot{text}┴─(group chunk_ref, 2; 24 h)─► sb-chunk-join ─► BookChunk
                (path slot alone at timeout ─► BookChunk{error:"no OCR text"})
   BookChunk ─► sb-page-assemble [bound]: split_pages (ported from ocr graph.rs), pages/NNNN.json + BookPage×p
                (headings, header/footer recovery, figure refs). Error or complete:false tail:
                ocr_failure_policy=fail ─► terminal(failed, "ocr chunk c (pages a-b) failed: <error>" |
                                           "ocr chunk c truncated at 1.5 MB: pages x-b missing" (D22))
                quarantine ─► mark only the missing/failed pages quarantined (per page when rows exist, else chunk)
       ├─ variant != ocr-only and chunk range ∩ 1..min(30,N) ─► Signal{front, member:chunk<c>,
       │     expected = number of chunks intersecting 1..min(30,N)}   (images: expected = min(30,N))
       └─ every chunk ─► Signal{ocr, member:chunk<c>, expected = chunks_total}
 front gate ─► stage:front: exact prefix 1..min(30,N) required (Shelf). Missing member: fail ─► nothing (the ocr gate
       fails the book); quarantine ─► continue only if every missing front page is quarantined, else nothing.
       standard ─► StageStart{metadata} + StageStart{toc_finder};  text-only/photo-book ─► StageStart{metadata}
 metadata: 1 item (first 20 non-empty pages) ─► sb-metadata ─► MetadataResult ─► result:metadata
       (schema check: ≤2 repairs per attempt; ≤3 attempts) ─► BookMetadata + ItemOutcome ─► metadata_closed:
       ok ─► finish_signal("metadata");  failed ─► terminal(failed) (Shelf)
 toc_finder: 1 item ─► sb-toc-finder ─► TocFinderResult ─► result:toc_finder: schema + range check;
       not found && attempt<3 ─► StageTask{attempt+1, PreviousAttempt(pages_checked, structure_notes)}
       ─► ItemOutcome{ok|not_found|failed} ─► toc_finder_closed:
       found ─► Toc + toc_extract stage, 1 item, via the C1 fan-out helper (task file, item opener, batch and
             stage openers, so a lost Task closes within one batch timeout) ─► … ─► result:toc_extract (normalizeTocExtractEntries,
             schema; retry ≤3) ─► toc_extract_closed: TocEntry×E + Signal{link, member:"toc"}
       not found / extract exhausted:  require ─► BookStatus{failed, Shelf's message}
                                       fallback | whole_book ─► Toc{none} + Signal{link, member:"toc"}
 ocr gate ─► stage:ocr: members missing ─► ocr_failure_policy (fail ─► BookStatus{failed}; quarantine ─► mark pages)
       ocr-only ─► Signal{finish, "ocr"};  text-only ─► Signal{finish, "ocr"};  standard ─► Signal{link, member:"ocr"}
 link gate ─► stage:link_gate: {toc, ocr} required, else BookStatus{failed, "gate link incomplete: missing <m>"}
       ─► StageStart{link}: back-matter derivation, tasks/link/*.json, release batches of ≤32 (E=0 ─► StageStart{pattern})
        sb-toc-entry-finder (check_candidate in session) ─► TocLinkResult ─► result:link: ValidateCandidatePage
          reject && attempt<3 ─► StageTask{attempt+1, retry hint in user_prompt} ; else TocLink{linked|failed} + ItemOutcome
     ─► link_closed: any failed ─► BookStatus{failed} (fail closed, as Shelf) else ─► StageStart{pattern}
 pattern: 1 item ─► PatternResult ─► result:pattern: schema, sanitizers, generateEntriesToFind ─► PatternAnalysis
     ─► pattern_closed: exhausted ─► 0 entries (Shelf);  0 entries ─► StageStart{gap_closed}  else StageStart{discover}
        stage:discover (S7b) ─► discover×n ─► ChapterFindResult ─► result:discover: out-of-range page ─► not_found (no retry, Shelf);
          dedupe ─► TocEntry{discovered} + TocLink + ItemOutcome
     ─► discover_closed ─► findFinalizeGaps ─► gap×g (g=0 ─► StageStart{gap_closed})
        sb-gap-investigator ─► GapFixResult ─► result:gap: applyGapFix ─► TocEntry/TocLink revisions + GapFix + ItemOutcome
     ─► gap_closed: resortEntriesByPage ─► toc/final.json + a TocEntry revision for every entry whose sort_order
        changed ((i+1)*100). Zero linked or discovered entries:
          require | fallback ─► BookStatus{failed, "no linked ToC entries found"}
          whole_book ─► one chapter ch_001, pages 1..N, source "fallback"
        ─► StageStart{structure}
 structure: skeleton, extract, merge ─► chapters/*.json ─► StageStart{classify} ─► stage:classify (S8b) ─► classify×ceil(ch/64)
     ─► ClassifyResult ─► result:classify (schema + exact coverage; retry ≤3 then failed) ─► ItemOutcome
  ─► classify_closed: any failed ─► BookStatus{failed}; polish items for every chapter, in chapter order, batched as
     usual; at each batch's RELEASE the audio chapters get StageTasks and the others get ItemOutcome{skipped}
     (polished_text = mechanical_text, edits_applied_json = "[]", Shelf), so no batch clock starts before its release
     ─► PolishResult ─► result:polish: schema (≤50 edits, 500/500/200), ApplyEdits on the whole chapter text (Shelf); retry ≤3
  ─► polish_closed: any polish_failed ─► BookStatus{failed} (as Shelf) else WorkItem{commit}×chapters
 COMMIT: work:commit [bound], part p re-derives its paragraph slice from chapters/<id>.json:
     part 1 ─► Chapter + Paragraph×k (≤400 paragraphs and ≤3 MiB per part); a non-final part ─► Paragraph×k + WorkItem{commit, part p+1};
     only the FINAL part also writes the chapter's ItemOutcome (one per chapter, so batch_size stays per chapter)
     ─► commit_closed: every chapter's ItemOutcome ok (from members) + workspace check
     (replaces validatePersistedStructureChapters; callback writes are all-or-nothing, F-19), runs/<run>/canonical/book.json,
     digest, promote into books/<sha>/canonical/ (same preopen: the handler is bound on the book folder, F-37)
     ─► BookStructure + finish_signal("structure")
 finish gate ─► stage:finish: ALWAYS emits one lone terminal row (status_ref "<run>/terminal"):
     status.json exists ─► the marker's status and reason (repairs a marker whose commit failed);
     else members(format, variant) all present ─► terminal(complete | degraded (quarantined pages, only under quarantine));
     missing ─► terminal(failed, "pipeline did not finish: missing <members>").
     A terminal row that already exists makes this a harmless failed transaction (unique status_ref).
     members(format, variant): pdf|images: standard {metadata, structure}; text-only {metadata, ocr}; ocr-only {ocr};
     epub (any variant): {metadata, structure}
```

**OCR plan agreement.** The `ocr` gate needs exactly `chunks_total`
members and the front gate exactly the chunks that cross 1..min(30,N), both
computed by ingest before OCR runs (F-18). A plan that disagrees would
quiesce the `ocr` group (too many) or time it out after 72 h (too few). So:
(1) ingest counts with `gents/ocr`'s own code (`ocrcount`, decision 9) and
fails at once if it cannot count; (2) for image folders it copies only the
files it counts, flat, named `NNNN.<ext>`, and passes them as `OcrJob.files`,
which `gents/ocr` reads in that order instead of walking the folder (F-42);
`page_assemble` maps `OcrChunk.source` to a page through `RunFile.files`;
(3) `stage:ocr_plan` compares the actual plan with `RunFile` 30 minutes after
it is written and fails the book with the difference, instead of a 72 h
"pipeline did not finish".

**Paragraphs and polish.** `Chapter.polished_text = ApplyEdits(mechanical_text, edits)`
on the **whole** chapter text, which is Shelf's contract
(`common/structure_text.go:164`: `strings.Replace(text, old, new, 1)` in
order). Paragraphs are then derived by splitting both texts on blank lines
(runs of 3 or more newlines are kept byte for byte in the chapter text;
paragraph text is trimmed of them), with `start_page` from the page-offset
map. When the raw and polished split counts match, `Paragraph.raw_text` and
`polished_text` are paired 1:1; otherwise `polished_text` comes from the
polished split and `raw_text` from the mechanical split by position, with no
1:1 promise (`Chapter.paragraph_pairing: "positional"` records it). SB-S8b tests: byte equality with
Shelf's `ApplyEdits`, an edit whose `old_text` spans a paragraph break and
occurs **before** an in-paragraph occurrence (Shelf edits the spanning one),
and a 3-newline run.

**Retry budgets** (enforced in the result handlers and in `stage:task_retry`;
Task failures and content rejects share one budget of attempts; schema
repairs have their own budget of 2 per attempt, D24):

| Stage | Budget | On exhaustion |
| --- | --- | --- |
| metadata | 3 | failed (Shelf) |
| toc_finder | 3 | `toc_policy` |
| toc_extract | 3 | `toc_policy` (require → failed, as Shelf) |
| link | 3, with retry hint | fail closed |
| pattern | 3 | skip to discover with 0 entries; validate and gap skipped |
| discover | 3 (Task failures); invalid output → not_found, no retry | not found |
| gap | 3 | gap left unfixed |
| classify | 3 | failed |
| polish | 3 | failed (Shelf) |

**Task failures.** A Task that errors, times out, or ends without writing
produces a non-success `FireOutcome` (R17) whose `source_handoff_id` is the
`StageTask.handoff_id`, i.e. its `task_ref` (F-7, F-25). `sb-task-failed`
(correlated on `source_handoff_id`) writes a `TaskFailed` marker into
`StageTask`; the host writes that `task_ref` into it through the port
correlation (F-27). The `sb-task-join` group (`task_ref`, exactly 2) hands
both rows to `task_retry`, which only copies them into a
`StageStart{task_retry}`; the bound `stage:task_retry` decides, with the
task and result files in hand, between nothing (a result already arrived),
a re-issued `StageTask` with `attempt + 1` (a fresh session, because no
trigger sets `session_id_template`, F-26; Shelf's fresh agent) and the
terminal `ItemOutcome`. Unique `spawn_ref` and `outcome_ref` make any
remaining race fail harmlessly (F-19). If R17 is refused, the batch timeout
marks the item failed (D14) and `sb-items` is split (§2.6 fallback).

**After a failure.** Once `status.json` exists, every release handler
(`stage:*`, `*_next`, `result:*`, `stage:task_retry`) releases nothing
(decision 4.4), so the other branches stop spending model calls.

**Throughput** (F-17). Callbacks run one at a time, including every
`gents/ocr` extract (up to 900 s each). A 64-chunk book can take about 16 h
of OCR before linking starts. The gate and watchdog timeouts above (24 h for
a chunk join, 72 h for gates) are sized for that. The README states it, and a
gents issue for concurrent callback execution is filed (operator). Each
documents pack has its own `agent_principal` and `run_callback_engine` runs
per agent, so the engines are probably **per pack** (R21): OCR extracts
likely do not block structured_book callbacks. Two costs come on top of the
stage work: `sb-task-failed` runs once for **every** model Task, success
rows included (about 2,000 serial calls per book, each returning `null`),
and every harmless duplicate-row conflict reruns its plugin up to 3 times
(§1); X-00a sizes both with R21/R23. Timed-out
groups are noticed late, because group recovery visits one grouped binding
per 5 s tick; the timeouts already allow for that.

### 2.9 Plugins: inputs, outputs, permissions

| Plugin | Called as | Input | Output | Permissions |
| --- | --- | --- | --- | --- |
| `book_pipeline` | callbacks only (§2.6) | projected source doc or array | the §2.7 wire shape (single-port value or port-keyed object) | sealed; `bind_dir` `workspace` read_write (allowed folder, headless) |
| `book_search` | model tool for 4 agents | `{op, book, task, page_num?, previous_page?, current_page_observations?, query?, start_page?, end_page?}` | Shelf's tool JSON for that op (`write_result_ready`, `write_result_args`, `categorized_pages`, `clusters`, ...); `check_candidate` returns `{ok, reason}` with `ValidateCandidatePage`'s exact reject text | sealed; `bind_dir` `book` read on the run folder `<book folder>/runs/<run_id>` (the footer passes it). Reads `pages/`, `toc/`, `tasks/<stage>/<item>.json` |
| `book_research` | model tool for any pack | `{op, book, ...}`; Shelf research MCP args, minus `book_id` | Shelf's research MCP outputs | sealed; `bind_dir` `book` read on `books/<sha256>`; always the latest `canonical/book.json`, except `read_passage_at_version` |

`validate_quote` is Shelf's `researchmcp/tools.go:validateQuote` exactly:
`strings.TrimSpace`, empty is an error; the version check (against the
latest canonical file) runs before any search and returns
`exact_match=false, version_match=false`, "source version mismatch; refresh
the assignment before citing"; byte-exact `strings.Index`; rune offsets with
±180 runes of context; at most 20 occurrences. `requireResearchReady`
(`structure_complete && !structure_failed && ≥1 passage`) gates every op.

### 2.10 Test plan (structured_book)

1. **Unit tests** (`cargo test`), each module porting its Shelf tests by
   name (see the work-list acceptance), plus the goldens produced by SB-G0
   (`core/fixtures/{prompts/front,prompts/finalize,prompts/structure,canonical,evidence,finalize,epub,schemas}/*.json`,
   one `{input, output}` shape).
   **Every stateful stage path is a native `cargo test`** (F-41: a bound
   `pack test` case writes into the committed fixture, so its second run
   takes another branch, and its outputs carry the worktree's absolute
   path): the test copies `plugins/book/source/stages/testdata/<module>/**`
   into a fresh temp dir, calls `run(key, &input, &Ctx{ws, now: FIXED})`
   and compares outputs with the temp path replaced by `$WS`. These run in
   place, once per entry (`test-pack.sh:145-160`).
2. **Plugin cases** (`plugins/book/tests/<module>-*.json` for pipeline
   modules, `plugins/book_tools/tests/<op group>-*.json` for the tools), run
   by `gents pack test` under every entry of that source (F-30), which works
   from day one because SB-01a ships a check-green pack. Only for paths that
   write nothing: unbound calls (dispatch, `gate_open`, `batch_close`,
   `stage_close`, `task_failed`, `task_retry`, `from_fetch`, `chunk_slot`,
   `chunk_text`, `plan_check`, including the foreign-run and opt-out
   `null` outputs), every `book_tools` op over a committed read-only fixture
   (`bind`), and `chunk_join`/`page_assemble` on a folder with no `run.json`.
   An `expect` is given only when the output holds no path and no clock.
   A "refused" case is a **success** output (`BookStatus{failed}`,
   `{"error": "..."}`, `null`): a plugin error can never be an expected
   result (`PluginCase` has no failure form, F-41).
3. **Pack tests** (`tests/`):
   - `install.json`: documents, slots and plugins an install creates;
     reinstall and remove idempotency.
   - `defs.json`: jq assertions that the §2.5 stage table holds for every
     behavior, task, trigger, surface, tools doc and binding; every surface
     fills exactly the 5 CTX fields; no `inference_*` keys; no schema field,
     `input_fields` entry or filter field matches `/key|token|secret|password/i`;
     every foreign `input_fields` entry exists in the producing pack's SDL;
     every callback has `max_attempts: 3`; counts match §2.6; every
     collection delivered to an `emit_outcome` Task declares
     `handoff_id: String` (F-25); no trigger sets `session_id_template`
     (F-26); no task sets `goal_token_budget` or `goal_objective_template`
     (F-32); every callback output port's `correlation_field` is a field of
     its collection (F-27), naming the `fetch_run_ref` and `task_ref`
     exceptions, and for per-document bindings that the event source's
     `correlation_field` equals the handler's (the host writes the **source's**
     correlation, `trigger_engine/event_delivery.rs:57-61`); no event source
     on `OcrChunk`, `OcrDocument` or `FetchedSource` has a filter (F-29);
     the `input_fields` rules of §2.6; each callback's `outputs` match
     `CALLBACK_PORTS` (checked from the Rust side by a `cargo test` in
     `plugins/book` that reads `../../pack_config.json`; it is skipped while
     the file still holds SB-01a's empty `callbacks` array); no trigger uses `concurrency: "serial"` (F-39).
   - `runtime_pages.json`: `"repository": {"access": "read_write"}`; the
     fixture book folder (with `runs/sb-fixture/run.json`) is copied in with
     `"copy"` (one entry per file), and the seeded `BookChunk.workspace` is
     `${REPOSITORY}/books/<sha>`. Expect `BookPage` ×3 and
     `Signal{gate:"ocr"}`.
   - `runtime_commit.json`: the same mechanics with a fixture book folder
     holding `runs/sb-fixture/{run.json,chapters/*.json,tasks/commit/*.json}`
     and a seeded `StageStart{stage:"commit_closed"}` whose payload lists the
     members. Expect `BookStructure` and `Signal{gate:"finish",
     member:"structure"}`. This is the one case that exercises a real
     `bind_dir` write outside `runs/<run_id>/` (the promote, F-37); the
     runtime harness copies the fixture into a fresh repository on every run
     (`test-pack.sh:386-417`).
4. **Commands:** `make -C $W/<id> test-structured_book GENTS=$GENTS_BIN` (the
   existing pattern rule, no Makefile edit; it builds before it checks, F-28),
   or by hand `gents pack build P && gents pack test P`. `$GENTS_BIN` is the
   binary X-00a builds and records.
   `install.json` and `runtime_pages.json` run structured_book **alone** in a
   fresh home, as `test-pack.sh` does, with neither ocr nor browser_download
   installed (R1).
5. **End to end** (§4).

---

## 3. Pack `gents/browser_download`

Path: `packs/gents/browser_download/`

Only legitimate resolvers and fetch-to-file plumbing: open-access,
public-domain, author-posted and institutional-repository copies, and plain
URLs the operator supplies. **No shadow-library resolver, hook, host or
documentation.** Bytes never come from the web-research MCP (F-24); it is
used only to find landing pages.

### 3.1 `manifest.json` (outline)

```jsonc
{
  "manifest_version": 1, "name": "browser_download", "namespace": "gents", "version": "0.1.0", "kind": "documents",
  "description": "Resolves a DOI, an archive identifier, a URL or a bibliographic description to an openly licensed or public-domain file, downloads it with resumable range requests, verifies and hashes it into a content-addressed library, and records a FetchedSource with provenance and license.",
  "config": "pack_config.json",
  "schemas": ["schemas/download_job.graphql","schemas/download_plan.graphql","schemas/source_search_result.graphql",
              "schemas/fetch_progress.graphql","schemas/agent_fetch_result.graphql","schemas/fetched_source.graphql"],
  "inference_slots": [
    { "name": "finder", "description": "Finds candidate open copies from a title and author with web search, and (fetch_mode agent only) downloads them with curl.",
      "behaviors": ["dl-source-finder","dl-fetch-agent"] } ],
  "external_dependencies": [ { "service_id": "web-research-mcp", "description": "SearXNG/Firecrawl gateway; only web_search and web_scrape_url are used",
      "repository_url": "https://github.com/source-inc/web-research-mcp", "install_command": "./scripts/stack install-mcp" } ],
  "plugins": [
    { "name": "download_resolve", "language": "rust", "source": "plugins/download_resolve", "artifact": "plugins/download_resolve.afb",
      "instructions": "plugins/download_resolve/TOOL.md", "input_schema": { "type": "object" },
      "manifold": { "fs": "None", "env": "None", "crypto": false, "child_process": false, "http_timeout_ms": 30000,
        "net": { "OutboundHttp": ["api.openalex.org","api.crossref.org","export.arxiv.org","api.archives-ouvertes.fr",
          "zenodo.org","library.oapen.org","directory.doabooks.org","archive.org","catalog.hathitrust.org",
          "www.gutenberg.org","gutenberg.org","scaife.perseus.org","raw.githubusercontent.com","penelope.uchicago.edu"] } },
      "limits": { "memory_mib": 256, "wall_clock_secs": 120, "max_output_mib": 1 } },
    { "name": "download_fetch", "language": "rust", "source": "plugins/download_fetch", "artifact": "plugins/download_fetch.afb",
      "instructions": "plugins/download_fetch/TOOL.md",
      "input_schema": { "type": "object", "properties": {
        "library_path": { "type": "string" }, "library_root": { "type": "string" } } },
      "manifold": { "fs": "None", "env": "None", "crypto": false, "child_process": false, "http_timeout_ms": 30000,
                    "net": { "OutboundHttp": null } },
      "bind_dir": { "input_field": "library_path", "original_field": "library_root", "access": "read_write",
                    "description": "The download library folder" },
      "limits": { "memory_mib": 512, "wall_clock_secs": 90, "max_output_mib": 1 } }
  ],
  "assets": [ "README.md","pack_config.json","schemas/<6 files>",
              "agent_behaviors/{source_finder,fetch_agent}/system_prompt.md","tasks/{source_finder,fetch_agent}/prompt.md",
              "plugins/download_resolve.afb","plugins/download_fetch.afb",
              "plugins/download_resolve/TOOL.md","plugins/download_fetch/TOOL.md",
              "tests/install.json","tests/defs.json" ]
}
```

Two sources, because the entries differ in `bind_dir` and network grant
and `pack test` runs a source's cases under each entry naming it (F-30).
Shared code (`common/`: `http.rs`, policy, User-Agent, `Candidate`) is
authored in `plugins/download_resolve/source/common/` and copied into
`download_fetch` by `plugins/download_fetch/sync-common.sh` (BD-01, decision 9).

Install needs `--grant-authority` (network and limits). Request rules: no
email anywhere (no `mailto=`, no Unpaywall, no `From`);
`User-Agent: gents-browser-download/<ver> (+https://github.com/gents-ai/packs)`.

### 3.2 Schemas

- `DownloadJob`: prior-art §6 fields (`run_id`, `url`, `doi`, `identifier`,
  `title`, `authors`, `year`, `formats`, `allow_unknown_license`,
  `library_path`, **required**: `dl-resolve` refuses a job without it,
  because every `DownloadPlan` must carry a non-empty `library_path` or the
  finder and agent fires are refused, F-13) plus `fetch_mode: String` (`plugin`, the default on main,
  or `agent`, §3.6) and `structure: Boolean` (copied to `FetchedSource`;
  structured_book only picks up rows with `structure: true`).
- `DownloadPlan { run_id @index, status @index (ready|needs_search|unavailable|refused), fetch_mode, library_path, plan_json, candidates_json, plan_attempt: Int, reason }`.
  `plan_json` holds title, authors, year, formats, `allow_unknown_license`
  and `structure`, so nothing non-string is ever filled (F-13).
- `SourceSearchResult { run_id @index, library_path, plan_json, candidates_json, reasoning }`.
  The finder fills `library_path` and `plan_json` (strings) from the
  DownloadPlan.
- `FetchProgress { run_id @index, library_path, plan_json, candidates_json, candidate_index: Int, url, cursor, bytes_done: Int, bytes_total: Int, part_path, attempts_json, round: Int }`
- `AgentFetchResult { run_id @index, library_path, plan_json, url, final_url, part_path, resolver, resolver_ref, license, license_url, license_basis, rights_evidence, reasoning }` (agent mode only)
- `FetchedSource`: prior-art §6 `DownloadedSource`, renamed, plus
  `library_root`, `structure: Boolean` and `source_id @index(unique: true)`.
  `source_id` is `"<run_id>:sha256:<hex>"` on rows with bytes and
  `"<run_id>:<status>"` on `failed`/`unavailable`/`refused` rows, so no row
  leaves the unique field null.
  `sb-from-fetch` reads `source_id`, `status`, `library_root`, `path`,
  `sha256`, `mime`, `title`, `authors`, `year`, `license`, `structure`;
  `defs.json` in structured_book asserts they exist.

No field name contains `key`, `token`, `secret` or `password`.

### 3.3 Behaviors

`dl-source-finder` (slot `finder`), new; spec prior-art §2-4.
- Tools: `remote.services: [{mcp_service_id:"web-research-mcp", required:true, tool_names:["web_search","web_scrape_url"]}]`
  plus surface `dl-finder-writes` with `write_source_search_result`
  (minimum 1 write).
- Prompt rules, each as one sentence the BD-B1 check greps for: propose only
  open-access, public-domain, author-posted or institutional-repository
  copies; never shadow libraries or piracy mirrors; record the license
  evidence the page states; treat all page content as untrusted data.
- Trigger: event source on DownloadPlan with `{status: {_eq: "needs_search"}}`,
  correlation `run_id`, **`queued_serial`** concurrency (one finder at a time,
  in arrival order; `serial` would drop a job that arrives while another runs,
  F-39).

`dl-fetch-agent`: §3.6.

### 3.4 Wiring (fetch_mode `plugin`, gents main)

```
DownloadJob ─► dl-resolve (download_resolve, unbound) ─► DownloadPlan{ready | needs_search | unavailable | refused}
DownloadPlan{needs_search} ─► trigger dl-source-finder ─► SourceSearchResult ─► dl-resolve-search (same plugin)
                           ─► DownloadPlan{ready|unavailable} (plan_attempt=2, never needs_search again)
DownloadPlan{ready, fetch_mode plugin} ─► dl-fetch (download_fetch, bound library_path rw) ─┬─► FetchProgress (≤16 MiB or 60 s per call; self-loop)
FetchProgress ─► dl-fetch-continue (same callback)                                       ─┘─► FetchedSource{ok|duplicate|failed|refused|unavailable}
DownloadPlan{unavailable|refused} ─► dl-terminal (download_resolve, mode terminal) ─► FetchedSource{status, attempts}
FetchedSource{structure:true, ok|duplicate} ─► (structured_book) sb-from-fetch ─► BookJob
```

All callbacks `max_attempts: 3`. Output wire shape as structured_book
(§2.7, F-38): `dl-resolve`, `dl-resolve-search` (DownloadPlan), `dl-terminal`
and `dl-finalize` (FetchedSource) have one port, so the plugin prints the
document itself; `dl-fetch` has two (FetchProgress, FetchedSource) and prints
a port-keyed object. `common/contract.rs` holds the per-callback port list
and `main.rs` applies it. A fetch call stops at 16 MiB or **60 s**,
whichever comes first, because callbacks run serially (F-17) and a long call
starves OCR; an 8 MB file still fits in one call.

- **Resolve** ranks OA locations (publishedVersion > accepted > submitted;
  PDF > EPUB > HTML; repository > unknown host) and refuses candidates with
  no OA signal. Internet Archive: refuse `access-restricted-item` and lending
  collections. HathiTrust: full view only, `pdus` recorded as
  `rights_jurisdiction: US`. A missing license is `license: unknown` with
  `license_basis: resolver_metadata`, fetched only with
  `allow_unknown_license`.
- **Fetch:** `Range: bytes=0-0` for size; up to 8 redirects by hand (F-22);
  1 MiB ranges into `.parts/<run_id>.part`; cursor at 16 MiB or 60 s.
- **Finalize** (shared with agent mode): sniff magic bytes; an HTML landing
  page gets one `citation_pdf_url` retry (plugin mode only); SHA-256 by
  re-reading; upstream md5/sha1/size check; dedup into
  `sha256/<aa>/<hex>.<ext>` (`deduplicated: true` when present).
- **Politeness** inside a call: per-host spacing (arXiv 3 s, others 1 s),
  `Retry-After`, at most 3 attempts per URL.

### 3.5 Test plan (browser_download)

`gents pack test` cannot serve canned HTTP: a plugin with a network grant
gets a live session and the host strips `http_results` and `state` from the
case input (F-31). So:
- `common/http.rs` is a **pure** round API, `fn round(input: Value) -> Value`:
  input carries `http_results` and `state`, output carries `http_calls` or a
  result. Every resolver and transfer test is a native `cargo test` that
  feeds canned rounds (`test-pack.sh` runs `cargo test` once per Rust plugin
  source): each resolver hit, miss, refusal; fetch first round, continue,
  redirect chain, landing page to PDF, a server that ignores `Range` above
  1 MiB (refused), the 60 s cutoff (with an injected clock), dedup, checksum
  mismatch; no request carries `mailto` or an email.
- Plugin JSON cases only for paths that never ask for HTTP: refusals before
  the first request (missing `library_path`, a denylisted or non-HTTPS URL),
  terminal mode, and finalize on an agent-mode part file (ok, wrong host
  refused, HTML refused).
- `tests/defs.json`: no shadow-library host in the allow-list or prompts (jq
  against a denylist kept in the test only); every DownloadPlan status
  handled; secret-name rule; every callback `max_attempts: 3`; the port
  correlation rule (F-27); no trigger uses `serial` (F-39); single-port
  callbacks as listed in §3.4.
- `tests/install.json` installs only once the harness registers
  `web-research-mcp` for documents installs (task X-02, F-40). If that
  harness change is refused, BD-T1 ships `defs.json` only and the README
  records the gap; §4 step 5b registers the service by hand.
- `make -C $W/<id> test-browser_download GENTS=$GENTS_BIN`; live smoke test in §4.

### 3.6 Fallback for gents `v0.20.0`: `fetch_mode: "agent"`

On the tag, plugins get no network (no `http_calls`), so `download_resolve`
and the transfer part of `download_fetch` cannot run. The web-research MCP
cannot deliver bytes either (F-24). The fallback that works on the tag (and
on main, if the operator prefers it) is an agent that downloads with a
restricted host `curl`, followed by the same deterministic finalize:

```
DownloadJob{fetch_mode:"agent"} ─► dl-resolve: the plugin builds a plan without network
     (url, or identifier → the resolver's metadata URL and allowed hosts) ─► DownloadPlan{ready, fetch_mode agent}
DownloadPlan{ready, agent} ─► trigger dl-fetch-agent (slot finder, queued_serial, F-39)
     tools: web-research-mcp web_search + web_scrape_url (to read metadata and landing pages);
            host.bash {mode: Unrestricted, execution_mode: workspace_write, network_mode: enabled,
                       root: "${GENTS_DOWNLOAD_LIBRARY:-.}" (F-36; a pack cannot know $LIB, the README
                       tells the operator to export it before install), allowed_argv_prefixes: [["curl","--fail","--location","--max-redirs","8",
                       "--proto","=https","--proto-redir","=https","-A","gents-browser-download/0.1 (+https://github.com/gents-ai/packs)",
                       "-w","%{url_effective}","-o"]]};
            surface write_agent_fetch_result
     ─► AgentFetchResult{part_path under .parts/, final_url, license evidence}
AgentFetchResult ─► dl-finalize (download_fetch in finalize mode, bound rw, no network) ─► FetchedSource
```

- The agent writes only into `.parts/`; finalize does the sniffing, hashing,
  dedup and placement, so a model never names the final file or its hash.
- Finalize refuses (and deletes the part) when `final_url`'s host is not in
  the resolver allow-list of §3.1, unless the job supplied that URL itself.
- The rights rules are checked by the agent from the metadata page, not by
  code, so every agent-mode row has `license_basis: agent_reported` and
  `needs_review: true`.
- This trades sandboxing for availability: host `curl` with network runs
  outside the WASM sandbox. The argv prefix limits only the head of the
  command (F-35): arguments after `-o` are free, so `-T <file>` or
  `-d @<file>` can upload any file readable under `root` to any HTTPS host,
  and `-K <file>` or a second URL are possible too. That is an
  **exfiltration path**, contained only by `execution_mode` and `root`
  (point `GENTS_DOWNLOAD_LIBRARY` at a folder holding nothing but the
  library). That is why it is an operator decision.
- X-00b checks that the pack (with network-declaring plugins) installs on the
  tag and that this path runs there (R22).

---

## 4. End-to-end scenario (one real short public-domain PDF)

- **Work:** Edgar Thurston, *Coins catalogue no. 2: Roman, Indo-Portuguese,
  and Ceylon* (Madras, 1894). IA identifier `in.gov.ignca.23391`, 80 pages,
  original scan `23391.pdf` (8.14 MB). Published 1894, so public domain; IA
  gives no license field, so the job sets `allow_unknown_license: true`.
- **Fallback:** Schoff, *The Periplus of the Erythraean Sea* (1912), IA
  `periplusoferythr00schouoft`, 344 pages.

`docs/packs/e2e/run.sh`:
1. Uses the gents binary X-00a built at `d4df8a02b` (or the operator's
   later pin), and prints `gents version` (there is no `--version` flag).
2. `gents init` with a fresh home against `http://100.73.235.38:8000/v1`,
   `--max-concurrent 32`; `gents config backend set ... --max-queue-depth 4096`.
3. Adds the second backend and profile for `http://100.87.27.25:8000/v1`
   (same queue depth).
4. `gents plugin dirs add $LIB --access read_write` (`$LIB` outside `$HOME`
   and the gents home); exports `GENTS_DOWNLOAD_LIBRARY=$LIB`.
5. Installs `gents/ocr --grant-authority` (slot `document_reader=ws1`).
5b. Registers `web-research-mcp` (F-40): `gents config export --root $CFG`,
   add a `tool_service_registries` entry `{service_id: "web-research-mcp",
   hostname, lan_ip, mcp_port, mcp_path}` with the operator's host, then
   `gents config apply --root $CFG`, exactly as `test-pack.sh::register_services`
   does (`:293-308`).
6. Installs `browser_download --grant-authority --inference-slot finder=ws1`.
7. Installs `structured_book --grant-authority --inference-slot book_reader=ws1 --inference-slot book_agent=ws2`.
8. Starts `gents server`.
9. Creates the seed with `gents document create DownloadJob --home H --graphql URL --json "{\"run_id\":\"e2e-thurston-$ATTEMPT\",\"identifier\":\"ia:in.gov.ignca.23391\",\"formats\":[\"pdf\"],\"allow_unknown_license\":true,\"structure\":true,\"library_path\":\"$LIB\"}"` (real JSON with quoted keys; `test-pack.sh` passes `--json` straight to `document create`)
   and, as the packs harness does, retries with a fresh `run_id` until a
   `DownloadPlan` appears (the callback engine logs nothing when it picks up
   bindings).
10. Polls `BookStatus` for up to 6 hours.

Pass criteria (`docs/packs/e2e/expect.md`):

**Must** (the run fails otherwise):
- `FetchedSource{status: ok}` whose `sha256` equals `sha256sum` of the
  library file and whose `bytes` equals the IA `files.xml` size.
- Exactly one `OcrJob`; one `FireOutcome` per model `StageTask` fire with
  a non-empty `source_handoff_id` (proves F-25 is met); `BookPage` count = 80 (or the book is `failed` with
  an OCR reason under the default policy).
- `BookStructure.structure_digest` matches `^sha256:[0-9a-f]{64}$` and equals
  the digest recomputed by `book_research get_book` from `canonical/book.json`.
- `validate_quote` with 40 characters cut from a random paragraph returns
  `exact_match: true, version_match: true`; one changed character gives
  `exact_match: false`; a wrong `expected_structure_digest` gives
  `version_match: false` and no occurrences.
- Re-running the `DownloadJob` with a new `run_id` gives
  `FetchedSource{deduplicated: true}`, `BookRun{reused: true}`,
  `BookStatus{reused}` and no new `OcrJob`.
- No request in the `InferenceCall` and plugin HTTP audit matches an email
  regex (`[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}`).

**Should** (a failure is recorded in `RESULTS.md` with its `BookStatus`
reason and a filed follow-up):
- `BookMetadata.title` contains "Coins".
- `Toc` exists; every `TocLink` linked, or the book failed with Shelf's
  reason.
- `BookStatus` is `complete`, and the run has exactly one row with
  `status_ref = "<run_id>/terminal"`.
- At least one `Chapter` and at least 20 `Paragraph`s.
- Peak agent context length recorded (D5).

---

## 5. Spike questions (X-00a/b/c answer them; X-01 amends this design)

Answers already found in code by the feasibility review are marked "code".

| # | Question | Code answer | If the spike says no |
| --- | --- | --- | --- |
| R1 | structured_book installs **alone in a fresh home** (as `test-pack.sh` runs `install.json`/`runtime_pages.json`), with its three **unfiltered** sources on absent `OcrChunk`, `OcrDocument`, `FetchedSource`, and they start firing once ocr/browser_download are installed later | code: an unfiltered source is not queried by `pack check` (F-29) and an absent collection is skipped at install (`desired_state.rs:801-821`); runtime unverified | move all three foreign sources into a `gents/structured_book_bridge` pack (install order then matters only for the bridge) |
| R2 | A callback can write `OcrJob` | yes (F-5) | `BookOcrRequest` plus a one-line Task |
| R3 | Grouped output correlation in `cause_ref`, plugin sets `run_id` | yes | one event source per gate keyed on `run_id` |
| R4 | A callback may write its own source collection (`FetchProgress`) | yes | Plan, `FetchRange`×N, grouped assemble |
| R5 | Model-tool plugin with `bind_dir` from a model argument, headless | yes | `BookPageSlice` surface queries |
| R6 | `http_calls` rounds inside a callback | yes on main; **absent on v0.20.0** | fetch_mode `agent` (§3.6) |
| R7 | Surface create accepts the types used, and long strings | yes; `JSON` refuses `[]` (keep `String`) | shrink classify chunks |
| R8 | WASI `hard_link` in a bound folder (only for `link_mode: hard_link`) | not answerable from gents | copy, streamed (already the default) |
| R9 | Callback concurrency | **serial** (F-17) | — (documented); R21 asks whether packs share one engine |
| R10 | GLM-5.3-Flash image input | not a gents question | v2 `book_vision` |
| R11 | Header/footer recovery without `data-label` | — | listed rewrites of the evidence tests |
| R12 | Image folders | one chunk per file (F-8), so `expected = chunks_total` above 256 quiesces the `ocr` gate (F-18) | ingest refuses above 256 files |
| R13 | Polish and link fail closed | kept | README repair route |
| R14 | An unfiltered foreign source plus a callback that returns no output (optional port) stores nothing and does not fail the invocation | code: an absent optional port writes no row (`callback/plugin.rs:113-126`) | filter in a later `<x>_next`-style bound step instead (cost only) |
| R15 | A **callback** with `bind_dir` read_write on a document-chosen path in an allowed folder, under headless `gents server` | — | redesign (blocks the pack) |
| R16 | A `cardinality: many` port accepts ~800 `BookPage` docs (~4 MiB) | — | page_assemble continues with `WorkItem{pages, cursor}`; only the last part writes the chunk's `front`/`ocr` Signals (as the commit rule, WL-N7) |
| R17 | A callback binding on `FireOutcome` (owner-scoped runtime rows) is seen by the callback engine's arrival cursor and fires; the exact `terminal_state` values (success vs the rest); an obligation miss is non-success | code: `source_handoff_id` and `attempt` are copied from the StageTask (F-7, F-25); no code found that forbids the binding | D14 fallback (timeouts) and the `sb-items` split (§2.6) |
| R18 | **Dropped.** Answered by code: `pack test` runs a source's cases under every entry (F-30), so entries that differ in bind field, limits or network get their own source (decision 9) | — | — |
| R19 | A unique-index violation fails a surface create visibly to the model, and fails a callback transaction | F-19 (callback) | dedupe in handlers by reading `results/` |
| R20 | **Informational.** The pack ships no goal fields (F-32). X-00a measures how long an ordinary agent Task runs on a toc_entry_finder item and whether the runtime caps its tool rounds | — | D18 as written |
| R21 | Do the three packs share one callback engine (one agent) or run in parallel engines? | likely per pack: each documents pack has its own `agent_principal`, and `run_callback_engine` is per agent | throughput only |
| R22 | On tag `v0.20.0`: ocr (local), structured_book (`link_mode: copy`) and browser_download (`fetch_mode: agent`, after registering `web-research-mcp`, F-40) install and run; OCR of a **copied** PDF 20 times on macOS (the tag's file bind hard-links it); delivery with more than 10,000 rows in a source collection (the tag's rescan limit) | code: both risks exist at the tag ("Runtime target") | the tag is unsupported, or "small libraries only" |
| R23 | Does group recovery cost grow with open never-filled groups (one `sb-task-join` group per successful StageTask, about 2,000 per book), and are dormant groups skipped? | recovery visits one grouped binding per 5 s tick; a timed-out group below `min_count` is dormant (F-34) | add `timeout_secs: 259200, min_count: 2` to `sb-task-join` |
| R24 | On macOS at the pin, a `link_mode: hard_link` source (two names: `sha256/…` and `runs/…/source.pdf`) binds and OCRs every time (20 runs) | code: `named_in_pinned_folder` (F-33) | `link_mode: copy` only, and drop the option |

Not ported in v1: operator repairs (`common/repair_*`, `resolve_toc_entry`,
`insert_toc_entry`, `exclude_toc_entry`, `quarantine_ocr`); EPUB export
(`SB-X1`); per-book prompt overrides; migrating Shelf books. Interrupted
callback invocations (F-20) are listed in the README as "stuck after
restart", with the repair route (`gents plugin run gents/book_pipeline` with
a crafted `StageStart`). The README names `stage:finish` explicitly: if its
own commit fails 3 times, or the server restarts during it or during the
finish `gate_open`, no terminal row is ever written; the repair is a crafted
`StageStart{stage:"finish", payload:{members:[...]}}`. It also records that
`gents/ocr`'s callbacks have no `max_attempts` (F-45), so one transient
remote-OCR error leaves a chunk to the 24 h `sb-chunk-join` timeout and, under
`fail`, a failed book (adding `max_attempts` belongs to the later ocr change).

Later phase (packs, needs operator approval because it edits `gents/ocr`):
copy `path` onto `OcrDocument` and delete the `ChunkSlot`/`BookChunk` join.

---

## 6. IMPLEMENTATION WORK-LIST

Roots:
- `P = $SRC/github.com/gents-ai/packs/packs/gents/structured_book` (as seen inside a worktree: `$W/<id>/packs/gents/structured_book`)
- `Q = $SRC/github.com/gents-ai/packs/packs/gents/browser_download` (likewise)
- `K = $SRC/github.com/gents-ai/packs` (the repository; read only for tasks, which work in worktrees)
- `W = $SRC/github.com/gents-ai/wt` (absolute worktree root; every task's worktree is `$W/<id>`)
- `S = $SRC/github.com/jackzampolin/shelf/internal` (read only)
- `H = designs/structured_book` (not a git repository)
- `G = $SRC/github.com/gents-ai/gents` (read only: nothing is written under `G`, not even `G/.git/worktrees`)
- `GENTS_BIN` = the binary X-00a builds: `git clone --shared G <scratch>/gents-pin`, `git -C <scratch>/gents-pin checkout d4df8a02b`,
  then `cargo build --release --bin gents` there. `GENTS_TAG_BIN` = the same at `v0.20.0` in `<scratch>/gents-tag`.
  X-00a records both absolute paths and `gents version` output in `H/spike/sb.md`; every later task uses those paths.

Rules for every task:
- **Branch.** X-01 creates `feat/structured-book-packs` from packs `origin/master` (`cd00bb8` at revision 3) and
  records the SHA in `H/SPIKE.md`. `K` itself stays as it is (it is checked out detached at `470c291`).
- **Worktrees.** One worktree per task: `git -C K worktree add $W/<id> -b <sb|bd|x>/<id> feat/structured-book-packs`.
  Cut it **only after every `depends_on` branch is merged** into `feat/structured-book-packs`.
  Commit only your target paths.
- **Integrator.** The orchestrator is the integrator. It merges each finished branch into
  `feat/structured-book-packs` in its own worktree `$W/integrate` (never in `K`), in `depends_on` order, and after each
  merge runs `plugins/book_tools/sync-core.sh --check`, `plugins/download_fetch/sync-common.sh --check`,
  `plugins/book/sync-ocrcount.sh --check` (once they exist) and
  `make -C $W/integrate test-structured_book GENTS=$GENTS_BIN` (and `test-browser_download`); a red merge is
  reverted and the task re-opened. The fixtures are never modified by a test run (rule "Tests" below), so a second
  run of the same tree gives the same result.
- **Mirrors.** A task that changes anything under `P/plugins/book/source/core/` runs
  `P/plugins/book_tools/sync-core.sh` before committing and commits the mirrored
  `P/plugins/book_tools/source/core/<the same files>`; likewise `Q/plugins/download_resolve/source/common/` →
  `Q/plugins/download_fetch/source/common/` via `sync-common.sh`. Each such task's target paths list both (written
  `core/X (+mirror)` below). Tests inside `core/`/`common/` read files only below `core/`/`common/`.
- **Spike files.** `H` has no git, so each spike writes only its own file under `H/spike/`;
  X-01 concatenates them into `H/SPIKE.md`.
- **Do not edit** `Cargo.toml`, `Cargo.lock`, `manifest.json`, `pack_config.json`, any `main.rs`,
  `core/contract.rs`, `common/contract.rs`, the `sync-*.sh` scripts, `plugins/book/source/ocrcount/**`, or the
  mirrored `core/` and `common/` folders, except: the mirror of your own `core/`/`common/` files (rule
  "Mirrors"); SB-T1 may append to `P/manifest.json` `assets` only; SB-C10 owns `ocrcount/**` and
  `sync-ocrcount.sh`. SB-01a/b/c and BD-01 commit the `Cargo.lock` files; nobody else commits a change to them.
- **Contract changes.** If you need a new field, struct, port or crate, stop and report it to the integrator. It
  opens a task `SB-K<n>` (or `BD-K<n>`; sonnet, S) whose only targets are `core/contract.rs` (+mirror) or
  `common/contract.rs` (+mirror) and, for a crate, `Cargo.toml`/`Cargo.lock`; your task rebases on it after it merges.
- **Tests.** Stateful stage paths are native `cargo test`s over a temp-dir copy of
  `plugins/<crate>/source/stages/testdata/<module>/**` (or `.../<module>/testdata/**` in browser_download), with
  `Ctx.now` pinned (§2.10, F-41). JSON plugin cases (`plugins/<crate>/tests/<your-module>-*.json`) only for calls
  that write nothing; an `expect` only when the output has no path and no clock; a refusal is a success output.
  JSON cases run under every entry of that crate (F-30) and cannot carry canned HTTP (F-31).
- **Acceptance always includes:** `make -C $W/<id> test-<pack> GENTS=$GENTS_BIN` passes (it builds before it
  checks, F-28), your new cases and `cargo test`s are listed as passed, and `git -C $W/<id> status` shows no
  modified tracked file after the run.

| id | pack | kind | title | source paths | target paths | acceptance | size | depends_on | model |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| X-00a | both | test | Spike: build `GENTS_BIN` and `GENTS_TAG_BIN`; structured_book runtime R1-R3, R5, R7-R9, R15-R17, R19-R21, R23, R24 (on main and on the tag); measure `complete:false` on the e2e PDF with the operator's OCR profile (D22) | `H/gents-pack-guide.md`, `G/crates/gents/src/{callback,plugin,trigger_engine,defra_write}`, `K/packs/gents/ocr`, `K/scripts/test-pack.sh` | `H/spike/sb/**` (throwaway packs), `H/spike/sb.md` | both binary paths and `gents version` outputs recorded; each R answered yes/no with the command and output that proves it; R1 run as a lone install in a fresh home (as `test-pack.sh`), with ocr installed afterwards; R17 shows a `FireOutcome` with `source_handoff_id` reaching a callback; R24 runs 20 hard-linked binds on macOS; `complete:false` count and causes recorded; R21/R23 include the `sb-task-failed`-per-Task and commit-retry costs (§2.8) | M | — | opus |
| X-00b | browser_download | test | Spike: R4, R6, R22 (agent fetch on the tag with `GENTS_DOWNLOAD_LIBRARY` and a registered `web-research-mcp`; OCR of a copied PDF 20 times on macOS; delivery past 10,000 rows) | §3.6, "Runtime target", `G/crates/gents/src/plugin/http_calls.rs`, `K/scripts/test-pack.sh::register_services` | `H/spike/bd/**`, `H/spike/bd.md` | same form as X-00a | S | X-00a | sonnet |
| X-00c | structured_book | test | Spike: R10 vision probe (one curl, one PNG, no email in the request) | — | `H/spike/vision.md` | request and response recorded | S | — | haiku |
| X-01 | both | docs | Amend DESIGN from the spike files; create the base branch | `H/spike/{sb,bd,vision}.md` | `H/SPIKE.md` (concatenation plus the base-branch SHA), `H/DESIGN.md`, branch `feat/structured-book-packs` in `K` (from `origin/master`) | every "no" answer applied through its fallback column; revision log updated; branch exists and its SHA is recorded | S | X-00a, X-00b, X-00c | opus |
| X-02 | both | test | `test-pack.sh`: register external services for documents installs and runtime cases (F-40) | `K/scripts/test-pack.sh` (`install_graph`, `register_services`) | `K/scripts/test-pack.sh` | `install_documents` and `runtime_case` call `register_services "$home" \|\| return 0` before `pack install`, as `install_graph` does; `make test-ocr` and `make test-web_deep_research` still pass; a throwaway documents pack whose Tools doc names an `external_dependencies` service installs. Shared-harness change: goes through packs review (operator decision 5); if refused, BD-T1 uses its fallback | S | X-01 | sonnet |
| SB-01a | structured_book | wiring | Scaffold: a pack that passes `make test-structured_book` on day one | this DESIGN §1, §2.1-§2.7; `K/packs/gents/ocr/{manifest.json,pack_config.json,plugins/ocr/Cargo.toml,rust-toolchain.toml}` | `P/manifest.json` (final shape, fixture names enumerated), `P/pack_config.json` (singletons, **9 placeholder behaviors and 9 contexts with the §2.5 ids**, other arrays empty), `P/README.md` skeleton (at least one `## ` heading, `check.rs:262-268`), `P/rust-toolchain.toml`; crate `P/plugins/book/{Cargo.toml,Cargo.lock,TOOL.pipeline.md}`, `P/plugins/book/source/{main.rs,core/mod.rs,stages/mod.rs}` plus a stub file for every module in §2.7 and every `core/*` module (`workspace`, `docs`, `text`, `furniture`, `evidence`, `evidence_text`, `search_link`, `search_front`, `canonical`, `prompts_front`, `prompts_finalize`, `prompts_structure`, `validate`), `main.rs` declaring the crate-root `ocrcount` modules (`#[path]` to stubs under `source/ocrcount/`), a stub `core/contract.rs`; crate `P/plugins/book_tools/{Cargo.toml,Cargo.lock,TOOL.search.md,TOOL.research.md,sync-core.sh}`, `P/plugins/book_tools/source/{main.rs,tools/mod.rs,tools/search.rs,tools/research.rs}` stubs and the mirrored `core/` with its drift `#[test]`; a placeholder for every asset (each SDL `type X { run_id: String @index }`, one-line prompts, trivially-true `tests/*.json`) | `make -C $W/SB-01a test-structured_book` passes; `sync-core.sh --check` exits 0; crates decided and listed (serde, serde_json, sha2, regex, flate2 and hayro-syntax at the ocr crate's versions and features, jsonschema with default features off (if it does not build for wasm32-wasip1, record it and SB-C9 hand-writes validators), zip/deflate and XML readers copied from the ocr crate, a temp-dir helper or `tempfile` as dev-dependency) | M | X-01 | sonnet |
| SB-01b | structured_book | wiring | `core/contract.rs` and the dispatch | §1.1, §2.3-§2.7 | `P/plugins/book/source/core/contract.rs` (+mirror), `P/plugins/book/source/main.rs`, `P/plugins/book/source/stages/mod.rs` | every §1.1 item present, incl. the 10 `TaskFile` variants, `CALLBACK_PORTS`, `Ctx`, ref builders and their parsers (4-segment `task_ref`; `/` refused in `item_ref`); `cargo test` dispatch test covers every key in §2.7 including `source_id` → `from_fetch`, `source_handoff_id` → `task_failed`, an OcrChunk array → `plan_check`, `op` refused by `plugins/book` (as a `cargo test`, the refusal is a plugin error); wire-shape test: a single-port callback prints the bare document or `null`, a multi-port one a keyed object without absent keys | M | SB-01a | opus |
| SB-01c | structured_book | wiring | Build scripts | §2.2, §2.5 | `P/.build/{merge.sh,check-fragment.sh,check-prompt.sh,fragments/_example.json}`, `P/.build/notes/_sites.md` | `merge.sh` (merge by id, duplicate id = error) test: a fragment replaces its placeholder and a duplicate id fails; `check-prompt.sh` maps each C-rule to a hunk regex and has a `--go-const` mode; `check-fragment.sh` checks jq validity, the 5 CTX fills, `output_obligation`, slot and §2.5 ids, `emit_outcome: true`, no `session_id_template`, no goal fields, **and merges the one fragment into a temporary copy of the pack and runs `gents pack check` on it**; per-behavior lists of tool-call sites to rewrite in `_sites.md` | S | SB-01a | sonnet |
| SB-02 | structured_book | schema | All 30 SDL files | §2.4; `S/schema/schemas/{book,page,toc,tocentry,chapter,paragraph}.graphql` | `P/schemas/*.graphql` | acceptance rule; `ls schemas \| wc -l` = 30 and names equal §2.4's list; no `!`; secret-name jq check passes; `StageTask` has `handoff_id: String` and `cause_ref`, `ItemOutcome` has `cause_ref`, `BookStatus` has unique `status_ref`, `BookJob` has `fetch_run_ref` and `link_mode`; `TocLinkResult` has no `entry_ref` | S | SB-01a | haiku |
| SB-G0 | structured_book | test | Golden generator: `_test.go` files injected with `go test -overlay overlay.json` into the target Shelf packages (can call unexported functions; nothing on disk in shelf changes); dumps Shelf's 9 JSON schemas and `structuredRepairPrompt` cases | `S/**` | `H/goldens/**` (generator, `overlay.json`, `run.sh`, `README.md`), `P/plugins/book/source/core/fixtures/**` (+mirror; including `fixtures/schemas/*.json`, the only copy of the schemas) | `H/goldens/run.sh` regenerates byte-identical fixtures; `git -C shelf status` clean afterwards; one documented `{input, output}` shape per area, with inputs **already shaped as the `contract.rs` types** (the Go→contract mapping per area is written in `H/goldens/README.md`; where G0 cannot shape an input, the README says so and the consuming C task owns that adapter in its test module); the README lists each target as **pure** (in-memory, e.g. `generateEntriesToFind` with the `SetLinkedEntries` cache, and `(*TocEntryFinderTools).ValidateCandidatePage`, which works on `t.book`), **httptest** (the digest: `researchmcp.NewClient` against an `httptest.Server` serving the snapshot) or **hand-derived** (the `findFinalizeGaps` loop, which loads through `svcctx.DefraClientFrom`, `jobs/common/toc.go:432,538`) | M | X-01, SB-01b | sonnet |
| SB-C1 | structured_book | plugin | `core/workspace.rs`, `core/docs.rs`: book-folder workspace and run-folder paths, the write rule of §1, atomic writes, streamed copy and hard link, run.json, page store, task files for every item (with rendered `user_prompt`, `closed`, attempt history), fan-out helper (batches ≤32/3 MiB or ≤128, openers, sequential release, the 1 MiB prompt cap, items that resolve **at release** as `skipped`), the `terminal()` marker protocol incl. `latest_run.json` (decision 4.4) | §1, §2.3, decisions 4-5, F-44 | `P/plugins/book/source/core/{workspace.rs,docs.rs}` (+mirror) | cargo tests: atomic write; batch math (0, 1, 32, 33, 128, 129, 8160); opener sizes; ref formats incl. `<attempt>.<repair>` and both `status_ref` forms with `/<n>`; size cap splits a 120 kB × 40 prompt set; a 1,000,001-byte prompt → `ItemOutcome{failed}` with the D27 reason; a batch with skipped items writes their outcomes only at release; page round-trip; marker: first writer creates, same `writer_ref` re-emits, other `writer_ref` gets nothing; a write outside `runs/<run_id>/` other than the two named ones is refused | M | SB-01b | sonnet |
| SB-C2 | structured_book | plugin | `core/text.rs`, `core/furniture.rs`: ExtractHeadings, header/footer recovery, StripHeaderFooter, CleanPageText, MergeChapterPages with page-offset map, CountWords, paragraph split (3+ newline runs kept) | `S/jobs/common/{headings.go,structure_text.go}`, `S/providers/chandra_parse.go` | `P/plugins/book/source/core/{text.rs,furniture.rs}` (+mirror), fixtures under `core/fixtures/furniture/` (+mirror) | `common/structure_helpers_test.go` and headings tests ported; furniture tests on a 5-page running-head fixture | M | SB-01b | sonnet |
| SB-C3 | structured_book | plugin | `core/evidence.rs`, `core/evidence_text.rs`: AnalyzePageEvidence, ValidateCandidatePage, back-matter derivation, on Markdown headings plus recovered furniture | `S/agents/toc_entry_finder/tools/{ocr_evidence.go,ocr_evidence_extract.go,ocr_evidence_text.go,ocr_evidence_test.go}`, `S/jobs/process_book/job/{link_toc_execution.go,link_toc_structure.go}` | `P/plugins/book/source/core/{evidence.rs,evidence_text.rs}` (+mirror) | every `ocr_evidence_test.go` case ported; label-dependent cases rewritten and listed in a header comment; reject reasons byte-identical (goldens from SB-G0) | L | SB-C2, SB-G0 | opus |
| SB-C4a | structured_book | plugin | `core/search_link.rs`: toc_entry_finder grep_text, get_heading_pages, get_page_ocr with evidence, check_candidate | `S/agents/toc_entry_finder/tools/{grep_text.go,get_heading_pages.go,get_page_ocr.go}` | `P/plugins/book/source/core/search_link.rs` (+mirror) | `grep_text_test.go`, `get_heading_pages_test.go`, `get_page_ocr_test.go` ported; JSON names match the Go structs; deserializes `TaskFile` in tests | M | SB-C1, SB-C2, SB-C3 | sonnet |
| SB-C4b | structured_book | plugin | `core/search_front.rs`: toc_finder grep report and `load_page` (C7 contract), chapter_finder grep/heading variants, gap context | `S/agents/toc_finder/tools/{grep_report.go,load_ocr_text.go,load_page_image.go}`, `S/agents/chapter_finder/tools/*.go`, `S/agents/gap_investigator/tools/{get_gap_context.go,get_page_ocr.go}` | `P/plugins/book/source/core/search_front.rs` (+mirror) | cases: observations refusal with Shelf's message; chapter_finder variant differences tested; JSON names match | M | SB-C1, SB-C2, SB-C3 | sonnet |
| SB-C5 | structured_book | plugin | `core/canonical.rs`: book.json format, `shelf-research-v1` digest, content_hash, validate_quote, search/read, list_structure, research readiness, `latest_run.json` reading (D23) | `S/researchmcp/{client.go,tools.go,types.go}` | `P/plugins/book/source/core/canonical.rs` (+mirror) | golden digest equals Go (SB-G0); validate_quote: trim, empty, version mismatch (no search), 20 cap, multi-byte rune offsets | M | SB-G0 | sonnet |
| SB-C6 | structured_book | plugin | `core/prompts_front.rs`: metadata, toc_finder (PreviousAttempt from `user.tmpl` with data, including schema-repair text as `Reasoning`), extract_toc | `S/prompts/metadata/{user.tmpl,workunit.go}`, `S/agents/toc_finder/{user.tmpl,prompt.go}`, `S/prompts/extract_toc/{user.tmpl,workunit.go}` | `P/plugins/book/source/core/prompts_front.rs` (+mirror) | SB-G0 goldens match byte for byte | M | SB-G0 | sonnet |
| SB-C7 | structured_book | plugin | `core/prompts_finalize.rs`: toc_entry_finder (window, retry hint ≤2000 B for rejects, schema repairs and Task-failure reasons, back-matter), pattern_analyzer, chapter_finder, gap_investigator | `S/agents/toc_entry_finder/{prompt.go,user.tmpl}`, `S/jobs/process_book/job/{link_toc_agents.go,job.go}`, `S/agents/{pattern_analyzer,chapter_finder,gap_investigator}/prompt.go` | `P/plugins/book/source/core/prompts_finalize.rs` (+mirror) | goldens match, including `prompt_test.go` cases and the rendered link retry prompt; exposes `link_retry_prompt(&TaskFile, reason)` for SB-S3 | M | SB-G0, SB-C3 | sonnet |
| SB-C8 | structured_book | plugin | `core/prompts_structure.rs`: BuildClassifyPrompt, BuildPolishPrompt | `S/jobs/common/{structure_classify.go,structure_prompts.go}` | `P/plugins/book/source/core/prompts_structure.rs` (+mirror) | goldens match | S | SB-G0 | sonnet |
| SB-C9 | structured_book | plugin | `core/validate.rs`: rebuild logical objects from flat fields plus `*_json` (null list → `[]`; null non-list field left out when its schema type disallows null; CTX and port-only fields stripped), validate against `include_str!`'d `core/fixtures/schemas/*.json`, port `structuredRepairPrompt`, repair budget 2 per attempt | `S/providers/structured_output.go`, `S/agents/toc_finder/tools/write_toc_result.go`, SB-G0 schema dumps | `P/plugins/book/source/core/validate.rs` (+mirror) | one reject case per constraint in decision 6 (levels, enums, ranges, maxItems, maxLength, nested required, additionalProperties); `authors: null` accepted as `[]`; toc_finder `toc_found=false` with `toc_page_range: null`, `structure_summary: null` accepted (fields omitted); `pages_checked` stripped before validating; repair text equals the SB-G0 golden | M | SB-G0 | sonnet |
| SB-C10 | structured_book | plugin | `ocrcount/`: `gents/ocr`'s PDF page counter and `ranges`, verbatim, with shim and drift test (decision 9, F-42) | `K/packs/gents/ocr/plugins/ocr/source/{pdf.rs,pdflazy.rs,pdfobj.rs,pdfdecode.rs,src.rs,plan.rs,input.rs,resume.rs}` | `P/plugins/book/source/ocrcount/**`, `P/plugins/book/sync-ocrcount.sh` | `sync-ocrcount.sh --check` exits 0; drift `#[test]` compares each copied file (and the `Backend::open`/`page_count` and `ranges` bodies) with `../../../ocr/plugins/ocr/source/` in the same repository; the shim provides exactly the items the copies import (`input::Options` as used by `pdflazy.rs`, `resume::Fnv`) and its header lists them; `page_count` equals the number of pages and `ranges` equals the `pages` of the OcrChunks `gents/ocr` plans, for every PDF under `K/packs/gents/ocr/plugins/ocr/tests/fixtures/` (`many.pdf`, `scan.pdf`, `layout.pdf`, `text.pdf`) (expected values taken from ocr's own `plan-mode.json`/graph cases or from one recorded `gents pack test`/runtime run of ocr, written in the test) | M | SB-01a | sonnet |
| SB-S1 | structured_book | plugin | `stages/ingest.rs`: ingest (run_id rule, magic-byte format, streamed sha256 compare, reuse, `link_mode` copy/hard link into `books/<sha>/runs/<run_id>/`, image folders copied flat as `NNNN.<ext>` in natural order with `OcrJob.files`, `page_count`/`ranges` via `ocrcount`, refusal when the count fails, run.json with `finish_expected = members(format, variant)+1`, finish opener via `finish_signal`, OcrJob, EPUB route, image list ≤256, multi-path refusal, pre-folder refusals write only the lone terminal row) and from_fetch (`null` unless `structure` and ok/duplicate; `run_id = "sb-" + run_id`) | `S/ingest/job.go`, `H/download-prior-art.md` §6, §2.8 "OCR plan agreement" | `P/plugins/book/source/stages/ingest.rs`, `P/plugins/book/source/stages/testdata/ingest/**`, `P/plugins/book/tests/ingest-*.json` | native tests: pdf, images (`p2.png`, `p10.png`, a stray `.txt` and a nested folder → 2 pages in numeric order, `files = ["0001.png","0002.png"]`), 257 images refused, epub, reused (terminal `status_ref`), force, bad prefix and `sb-a/b` refused with no marker and no opener, two paths refused, uncountable PDF refused, copy default; `chunks_total` and `ranges` for the e2e PDF and every ocr fixture PDF (`many.pdf`, `scan.pdf`, `layout.pdf`, `text.pdf`) equal SB-C10's expected plan; JSON cases (unbound): from_fetch ok/duplicate/unsupported mime/`structure: false` → `null`, `status: failed` → `null` | M | SB-C1, SB-C10 | sonnet |
| SB-S2 | structured_book | plugin | `stages/ocr_join.rs`, `stages/pages.rs`: chunk_slot, chunk_text, plan_check, stage:ocr_plan, chunk_join, page_assemble (split_pages port, BookPage, figure refs, source → page through `RunFile.files`, ocr_failure_policy, D22 reasons, front/ocr signals) | `S/jobs/process_book/job/{ocr.go,state.go}`, `S/jobs/common/{ocr.go,quarantine_ocr.go}`, `K/packs/gents/ocr/plugins/ocr/source/{graph.rs,plan.rs,ctx.rs}` | `P/plugins/book/source/stages/{ocr_join.rs,pages.rs}`, `P/plugins/book/source/stages/testdata/{ocrjoin,pages}/**`, `P/plugins/book/tests/{ocrjoin,pages}-*.json` | JSON cases (unbound): foreign `run_id` → `null` from chunk_slot, chunk_text and plan_check; workspace three levels up; text slot missing at timeout → `BookChunk{error}`; native tests: 812-page chunk math equals `ocrcount::ranges`; image-folder front `expected = min(30,N)`; `stage:ocr_plan` with a matching plan → nothing, with one chunk too many, one too few, and one differing `pages` → terminal failed naming the difference; failed chunk → failed book (default) and → page quarantine (policy); `complete:false` with the byte-cap and the time-budget cause → the D22 reasons; foreign folder (no run.json) → nothing; progress `status_ref`s unique within one call | M | SB-C1, SB-C2, SB-C10 | sonnet |
| SB-S3 | structured_book | plugin | `stages/gates.rs`: gate_open, batch_close, stage_close, task_failed, task_retry, stage:task_retry, stage:front/ocr/link_gate/finish, generic `_next` release (from stored `user_prompt`, `closed` marks, skipped-at-release), terminal protocol | `S/jobs/process_book/job/{state.go,job.go}` | `P/plugins/book/source/stages/gates.rs`, `P/plugins/book/source/stages/testdata/gates/**`, `P/plugins/book/tests/gates-*.json` | port `completion_failure_test.go`, `fail_book_test.go`; JSON cases (unbound): full and partial groups; openers ignored in counts; task_failed: success → `null`, failure → bare `TaskFailed` document with `run_id` from task_ref; task_retry with no `TaskFailed` → `null`; native tests: front with a missing member → nothing under `fail`, continue under `quarantine` only for quarantined holes; link gate with 1 of 2 members → failed; all items lost → stage closes failed after timeout; single toc_finder Task lost → policy path; stage:task_retry: result present → nothing, superseded `task_ref` → nothing, under budget → StageTask with new `handoff_id` (link: hint from C7; others: same prompt), over budget → ItemOutcome failed; "marker written, commit failed → retry re-emits"; "marker present, no row → finish emits marker's status"; finish emits once per `members(format, variant)` incl. EPUB with `variant: ocr-only`; nothing released after terminal | M | SB-C1, SB-C7 | opus |
| SB-S4 | structured_book | plugin | `stages/metadata.rs`: stage:metadata (first 20 non-empty pages of the front prefix), result (schema, persist all fields, `author = authors[0]`), metadata_closed | `S/jobs/common/metadata_ops.go`, `S/prompts/metadata/schema.go` | `P/plugins/book/source/stages/metadata.rs`, `P/plugins/book/source/stages/testdata/metadata/**` | port `metadata_test.go`, `metadata_gate_test.go`; native tests: ok; schema reject → repair 1, 2 → attempt + 1; attempt 3 exhausted → failed book | S | SB-C1, SB-C6, SB-C9 | sonnet |
| SB-S5 | structured_book | plugin | `stages/toc.rs`: toc_finder (range check, PreviousAttempt from pages_checked/structure_notes, schema repair through `PreviousAttempt.Reasoning`, Task-failure retry with the same prompt, policy), toc_extract released through the C1 fan-out helper (normalizeTocExtractEntries, replace-set revisions) | `S/jobs/process_book/job/{toc_finder.go,toc_extract.go}`, `S/jobs/common/toc.go` | `P/plugins/book/source/stages/toc.rs`, `P/plugins/book/source/stages/testdata/toc/**` | `toc.go` normalization tests ported; not found → retry → require (failed) / fallback; toc_finder `feedback` always empty; toc_extract Task lost → `toc_extract_closed` → `require` failure within one batch timeout (opener present); "reject → retry text, and where it goes" table in the module header | M | SB-C1, SB-C6, SB-C9 | sonnet |
| SB-S6 | structured_book | plugin | `stages/link.rs`: fan-out with task files, post-hoc validateTocLinkEvidence plus retry hint in user_prompt (rejects, schema repairs), fail-closed close, progress counters; the entry comes from the task file | `S/jobs/process_book/job/{link_toc.go,link_toc_agents.go,link_toc_execution.go,link_toc_structure.go}`, `S/jobs/common/toc_link_retry.go` | `P/plugins/book/source/stages/link.rs`, `P/plugins/book/source/stages/testdata/link/**` | port `link_toc_test.go`, `toc_link_skip_test.go`; native tests: accept, reject with Shelf's text, missing scan_page rejected as Shelf, schema reject → hint in user_prompt, 3rd reject → TocLink failed → book failed, zero entries → pattern, feedback always empty; "reject → retry text" table | M | SB-C1, SB-C3, SB-C7, SB-C9 | opus |
| SB-S7a | structured_book | plugin | `stages/finalize_pattern.rs`: pattern prepare/apply, sanitizers, generateSequence, roman numerals, estimatePageLocation, generateEntriesToFind, `pattern.json`, exhaustion → 0 entries, `StageStart{discover}` | `S/jobs/process_book/job/{finalize.go,finalize_pattern.go,finalize_helpers.go}` | `P/plugins/book/source/stages/finalize_pattern.rs`, `P/plugins/book/source/stages/testdata/pattern/**` | goldens from SB-G0 for sanitizers and entries-to-find; native tests: no ToC → `Chapter {n}`; no patterns → `StageStart{gap_closed}`; pattern exhausted → skip; n > 0 → `StageStart{discover}` | M | SB-C1, SB-C7, SB-C9, SB-G0 | opus |
| SB-S7b | structured_book | plugin | `stages/finalize_gaps.rs`: stage:discover (release), discover check (out-of-range → not_found) and dedupe, findFinalizeGaps, `gaps.json`, applyGapFix, resortEntriesByPage with persisted TocEntry revisions, the zero-entry `toc_policy` branch (sole owner) | `S/jobs/process_book/job/{finalize_discover.go,finalize_validate.go,finalize_helpers.go,finalize_dedupe_test.go}` | `P/plugins/book/source/stages/finalize_gaps.rs`, `P/plugins/book/source/stages/testdata/gaps/**` | port `finalize_dedupe_test.go`; gap detection cases (MinGapSize 15, hand-derived per SB-G0); resort writes revisions with `(i+1)*100`; zero entries: require/fallback → failed, whole_book → one chapter | M | SB-C1, SB-C7, SB-C9, SB-G0 | sonnet |
| SB-S8a | structured_book | plugin | `stages/structure_build.rs`: skeleton (ch_%03d, parents, end pages), extract mechanical text, merge, reuse key, `StageStart{classify}` | `S/jobs/process_book/job/{structure.go,structure_build.go,structure_extract.go}`, `S/jobs/common/structure_text.go` | `P/plugins/book/source/stages/structure_build.rs`, `P/plugins/book/source/stages/testdata/structbuild/**` | `common/structure_helpers_test.go` cases ported; reads `toc/final.json` and never branches on `toc_policy`; a `ch_001` fallback final file builds one chapter | M | SB-C1, SB-C2 | sonnet |
| SB-S8b | structured_book | plugin | `stages/structure_llm.rs`: stage:classify (release), classify coverage, polish plan with non-audio items resolved `skipped` at their batch's release, whole-text ApplyEdits then paragraph derivation, fail-closed | `S/jobs/process_book/job/{structure_classify.go,structure_polish.go}`, `S/jobs/common/{structure_classify.go,structure_text.go,state_persist_chapters.go}` | `P/plugins/book/source/stages/structure_llm.rs`, `P/plugins/book/source/stages/testdata/structllm/**` | port `structure_polish_reuse_test.go` where it applies (reuse not ported, D13); response_format tests not ported (stated); coverage rejects; `polished_text` byte-equal to Shelf's `ApplyEdits` incl. the spanning-occurrence-first counter-example and a 3-newline run; `paragraph_pairing` set; 40 audio chapters mixed with 30 non-audio → no `skipped` outcome before its batch's release; "reject → retry text" table | M | SB-C1, SB-C2, SB-C8, SB-C9 | opus |
| SB-S9 | structured_book | plugin | `stages/canonical.rs`: commit (Chapter without mechanical_text, Paragraph split, ≤400 paragraphs/3 MiB per part, part continuation, **only the final part writes the ItemOutcome**), commit_closed (members check, run canonical, digest, promote into the book folder's `canonical/`, BookStructure, finish Signal) | `S/jobs/process_book/job/structure_completion.go`, `S/researchmcp/client.go:loadSnapshot` | `P/plugins/book/source/stages/canonical.rs`, `P/plugins/book/source/stages/testdata/canonical/**` | native tests on a temp book folder: fixture digest equals the SB-C5 golden; atomic promote into `canonical/book.json` plus `canonical/<digest-hex>.json`; a 1,000-paragraph chapter commits in 3 parts under 4 MiB each with exactly one ItemOutcome (from part 3) and part p re-deriving its slice; the real-bind case is SB-T1's `runtime_commit.json` | M | SB-C1, SB-C2, SB-C5 | sonnet |
| SB-S10 | structured_book | plugin | `stages/epub_import.rs`: OPF, nav/NCX, spine, classifyChapter, to Toc, TocEntry, `TocFiles`, `ChapterFile`s and `BookMetadataFile` | `S/epubimport/{import.go,parser.go}` plus tests; `K/packs/gents/ocr/plugins/ocr/source/{epub.rs,xml.rs}` (wasm-proven) | `P/plugins/book/source/stages/epub_import.rs`, `P/plugins/book/source/stages/testdata/epub/**` | parser tests ported; the ocr pack's `book.epub` gives the SB-G0 golden chapter list | L | SB-C1, SB-G0 | sonnet |
| SB-P1 | structured_book | plugin | `tools/search.rs`: book_search ops over a bound run folder | `S/agents/*/tools/*.go` (schemas, descriptions) | `P/plugins/book_tools/source/tools/search.rs`, `P/plugins/book_tools/TOOL.search.md`, `P/plugins/book_tools/tests/search-*.json`, `P/plugins/book_tools/tests/fixtures/run/**` (read-only fixture run folder) | one case per op with `bind` on the fixture run (writes nothing; passes under both tool entries); `get_page_ocr` returns `write_result_ready`/`write_result_args` as Shelf; `check_candidate` returns the exact reject reason; a non-`op` input gives a success-shaped `{"error": ...}`; the fixture tree is unchanged after the run | M | SB-C1, SB-C4a, SB-C4b | sonnet |
| SB-P2 | structured_book | plugin | `tools/research.rs`: book_research ops over `books/<sha>/canonical`; TOOL.md for other packs | `S/researchmcp/tools.go` | `P/plugins/book_tools/source/tools/research.rs`, `P/plugins/book_tools/TOOL.research.md`, `P/plugins/book_tools/tests/research-*.json`, `P/plugins/book_tools/tests/fixtures/book/**` | one case per op (bound, read-only); clamps: top_k 10/50, snippet 500/1200, read 4000/12000, list limit 50/200, context_before 300/2000; readiness gate; `get_book` field set (`author`, `status_reason` from `latest_run.json`, `source_filename`, ...); validate_quote trim/empty/version mismatch (no search)/20 cap/rune offsets; old digest → `version_match:false`; `read_passage_at_version` | M | SB-C1, SB-C5 | sonnet |
| SB-B1 | structured_book | behavior-port | metadata | `S/prompts/metadata/{system.tmpl,schema.go}` | `P/agent_behaviors/metadata/system_prompt.md`, `P/tasks/metadata/prompt.md`, `P/.build/fragments/metadata.json`, `P/.build/notes/metadata.md` | `check-prompt.sh ... C1,C4,C8` exits 0; `check-fragment.sh` exits 0 (incl. its `pack check` on a temporary merge) | S | SB-01c, SB-02 | haiku |
| SB-B2 | structured_book | behavior-port | toc_finder (C3, C7) | `S/agents/toc_finder/{system.tmpl,schema.go,tools/write_toc_result.go}`, `S/agents/toc_finder_factory.go` | `P/agent_behaviors/toc_finder/**`, `P/tasks/toc_finder/prompt.md`, `P/.build/fragments/toc_finder.json`, `P/.build/notes/toc_finder.md` | `check-prompt.sh ... C1,C2,C3,C7,C8` and `check-fragment.sh` exit 0; surface has `pages_checked`, `structure_notes_json`; footer passes `{{ doc.workspace }}/runs/{{ doc.run_id }}` | M | SB-01c, SB-02 | sonnet |
| SB-B3 | structured_book | behavior-port | toc_extract | `S/prompts/extract_toc/{system.tmpl,schema.go}` | `P/agent_behaviors/toc_extract/**`, `P/tasks/toc_extract/prompt.md`, `P/.build/fragments/toc_extract.json`, `P/.build/notes/toc_extract.md` | `check-prompt.sh ... C1,C8`, `check-fragment.sh` exit 0 | S | SB-01c, SB-02 | haiku |
| SB-B4 | structured_book | behavior-port | toc_entry_finder | `S/agents/toc_entry_finder/{system.tmpl,schema.go,tools/write_result.go}`, `S/agents/toc_entry_finder_factory.go` | `P/agent_behaviors/toc_entry_finder/**`, `P/tasks/toc_entry_finder/prompt.md`, `P/.build/fragments/toc_entry_finder.json`, `P/.build/notes/toc_entry_finder.md` | `check-prompt.sh ... C1,C2,C3,C5,C8`, `check-fragment.sh` exit 0; `scan_page` optional; no `entry_ref` field; tools hold `gents/book_search` | M | SB-01c, SB-02 | sonnet |
| SB-B5 | structured_book | behavior-port | pattern_analyzer | `S/agents/pattern_analyzer/{system.tmpl,prompt.go}` | `P/agent_behaviors/pattern_analyzer/**`, `P/tasks/pattern_analyzer/prompt.md`, `P/.build/fragments/pattern_analyzer.json`, `P/.build/notes/pattern_analyzer.md` | `check-prompt.sh ... C1,C8`, `check-fragment.sh` exit 0 | S | SB-01c, SB-02 | haiku |
| SB-B6 | structured_book | behavior-port | chapter_finder (stage `discover`) | `S/agents/chapter_finder/{system.tmpl,tools/write_result.go}`, `S/agents/chapter_finder_factory.go`, `.build/notes/_sites.md` | `P/agent_behaviors/chapter_finder/**`, `P/tasks/chapter_finder/prompt.md`, `P/.build/fragments/chapter_finder.json`, `P/.build/notes/chapter_finder.md` | `check-prompt.sh ... C1,C2,C3,C8`, `check-fragment.sh` exit 0 | S | SB-01c, SB-02 | haiku |
| SB-B7 | structured_book | behavior-port | gap_investigator | `S/agents/gap_investigator/{system.tmpl,tools/write_fix.go}`, `S/agents/gap_investigator_factory.go`, `.build/notes/_sites.md` | `P/agent_behaviors/gap_investigator/**`, `P/tasks/gap_investigator/prompt.md`, `P/.build/fragments/gap_investigator.json`, `P/.build/notes/gap_investigator.md` | `check-prompt.sh ... C1,C2,C3,C6,C8`, `check-fragment.sh` exit 0 | S | SB-01c, SB-02 | haiku |
| SB-B8 | structured_book | behavior-port | structure_classify | `S/jobs/common/structure_prompts.go` | `P/agent_behaviors/structure_classify/**`, `P/tasks/structure_classify/prompt.md`, `P/.build/fragments/structure_classify.json`, `P/.build/notes/structure_classify.md` | `check-prompt.sh --go-const ClassifySystemPrompt ... C1,C8`, `check-fragment.sh` exit 0; enums copied into Output | S | SB-01c, SB-02 | haiku |
| SB-B9 | structured_book | behavior-port | structure_polish | `S/jobs/common/structure_prompts.go` | `P/agent_behaviors/structure_polish/**`, `P/tasks/structure_polish/prompt.md`, `P/.build/fragments/structure_polish.json`, `P/.build/notes/structure_polish.md` | `check-prompt.sh --go-const PolishSystemPrompt ... C1,C8`, `check-fragment.sh` exit 0; limits stated | S | SB-01c, SB-02 | haiku |
| SB-W1 | structured_book | wiring | Pipeline fragment: the §2.6 event sources (incl. `sb-ocr-plan`), 15 callbacks and bindings with the §2.6 `input_fields`, from the contract's port names and `CALLBACK_PORTS` | §2.6, `core/contract.rs`, `K/packs/gents/ocr/pack_config.json` | `P/.build/fragments/pipeline.json` | `check-fragment.sh` exits 0 (incl. `pack check` on a temporary merge); every `*Result` bound; foreign sources unfiltered; `sb-ocr-plan` has a timeout and no `expected_count`; `sb-fire` correlated on `source_handoff_id` with its port on `task_ref`; `sb-from-fetch` port on `fetch_run_ref`; every grouped port on `cause_ref`; every grouped source has correlation and (except `sb-task-join`) a timeout; optional ports where §2.6 says so; outputs equal `CALLBACK_PORTS`; `input_fields` equal the §2.6 table; `max_attempts: 3` everywhere; secret-name check passes | M | SB-01b, SB-01c, SB-02 | opus |
| SB-W2a | structured_book | wiring | Merge fragments into `pack_config.json` by id | all `P/.build/**` | `P/pack_config.json`, `H/build-notes/structured_book/*.md` (notes copied), `H/build-notes/structured_book/merge.log` | merged by id; no placeholder behavior left (every one replaced by its fragment); the `make -C $W/SB-W2a test-structured_book` output is saved to `merge.log` (it may be red: cross-task fixes are W2b's); `.build/` removed after the notes are copied | S | SB-02, SB-C1..SB-C10, SB-S1..SB-S6, SB-S7a, SB-S7b, SB-S8a, SB-S8b, SB-S9, SB-S10, SB-P1, SB-P2, SB-B1..SB-B9, SB-W1 | sonnet |
| SB-W2b | structured_book | wiring | Integration triage; may edit any `P/**` file | `P/**`, `H/build-notes/structured_book/merge.log` | `P/**`, `H/build-notes/structured_book/integration.md` | `make -C $W/SB-W2b test-structured_book` passes; every cross-task fix logged | M | SB-W2a | opus |
| SB-T1 | structured_book | test | Pack tests: replace the placeholder `install.json`, `defs.json`, `runtime_pages.json`; add `runtime_commit.json` and fixtures | §2.10, `K/packs/gents/ocr/tests/*.json`, `K/scripts/test-pack.sh` | `P/tests/**`, `P/manifest.json` (`assets`, append only) | `make -C $W/SB-T1 test-structured_book` green with no model; runtime cases use `${REPOSITORY}`, `"copy"`, `"access":"read_write"`, structured_book alone in a fresh home; `runtime_commit.json` promotes into the book folder under a real `bind_dir`; every §2.10 `defs.json` assertion present | M | SB-W2b | sonnet |
| SB-D1 | structured_book | docs | README (use, install order with `--grant-authority`, slots and sampling, workspace (book folder, run folders), `link_mode`, §2.8 diagram, C1-C8 and D1-D27, serial throughput per pack incl. the per-Task `sb-task-failed` call, finish timeout and how to raise it, goal opt-in, limits, stuck-after-restart and the `stage:finish` repair, `gents/ocr` without `max_attempts`, the per-foreign-document callback cost of unfiltered sources, Shelf citations do not carry over) plus both packs' rows in `K/README.md` | §0-§2, §5, `H/build-notes/structured_book/*.md` | `P/README.md`, `K/README.md` (pack index rows only) | `## ` sections present; every C# and D# listed | S | SB-W2b | sonnet |
| BD-01 | browser_download | wiring | Scaffold a pack that passes `make test-browser_download`: manifest, two crates, `common/http.rs` as a pure round API, `common/contract.rs` (per-callback port lists, wire shape as §2.7), resolver registry and trait, fetch/finalize skeleton | §3.1, §3.4, §3.5, `K/packs/gents/ocr` layout, `K/packs/gents/web_deep_research/manifest.json` | `Q/manifest.json`, `Q/pack_config.json` (singletons, **2 placeholder behaviors and contexts** `dl-source-finder`, `dl-fetch-agent`), `Q/README.md` skeleton (with a `## ` heading), `Q/rust-toolchain.toml`, `Q/plugins/download_resolve/{Cargo.toml,Cargo.lock,TOOL.md}`, `Q/plugins/download_resolve/source/{main.rs,common/http.rs,common/policy.rs,common/contract.rs,resolver.rs}` plus a stub per module, `Q/plugins/download_fetch/{Cargo.toml,Cargo.lock,TOOL.md,sync-common.sh}`, `Q/plugins/download_fetch/source/{main.rs,transfer.rs,finalize.rs}` stubs and the mirrored `common/` with its drift `#[test]`, placeholders for every asset, `Q/.build/merge.sh` (by id) | `make -C $W/BD-01 test-browser_download` passes; `Resolver` trait and `Candidate` fixed; `round()` is pure (a cargo test drives two canned rounds); wire-shape test (single port → bare document or `null`); stubs return success-shaped errors | M | X-01 | sonnet |
| BD-02 | browser_download | schema | 6 SDL files | §3.2, `H/download-prior-art.md` §6 | `Q/schemas/*.graphql` | acceptance rule; no `!`; names match the manifest; secret-name check passes | S | BD-01 | haiku |
| BD-C1 | browser_download | plugin | Resolve framework: routing (job / search result / terminal / agent plan), ranking, OA refusal, URL policy, User-Agent, required `library_path`; resolvers openalex, crossref, url | prior-art §2-4, §7 | `Q/plugins/download_resolve/source/{route.rs,rank.rs,openalex.rs,crossref.rs,url.rs}`, `Q/plugins/download_resolve/tests/framework-*.json` (no-network paths only) | `cargo test` with canned rounds: hit, miss, no-OA refusal; no request carries `mailto` or an email (asserted); JSON cases: missing `library_path` → `DownloadPlan{refused}`, non-HTTPS URL → refused, terminal mode → bare `FetchedSource` with `source_id = "<run_id>:<status>"` | M | BD-01 | sonnet |
| BD-C2 | browser_download | plugin | Resolvers internet_archive, hathitrust, gutenberg | prior-art §4 | `Q/plugins/download_resolve/source/{internet_archive.rs,hathitrust.rs,gutenberg.rs}` | `cargo test` with canned rounds: `in.gov.ignca.23391` metadata picks `23391.pdf`; restricted and lending refused | M | BD-01 | sonnet |
| BD-C3 | browser_download | plugin | Resolvers arxiv, hal, zenodo, oapen/doab, perseus, lacuscurtius | prior-art §4 | `Q/plugins/download_resolve/source/{arxiv.rs,hal.rs,zenodo.rs,oapen_doab.rs,perseus.rs,lacuscurtius.rs}` | `cargo test` with canned rounds: hit and miss per resolver; license fields mapped | M | BD-01 | sonnet |
| BD-F1a | browser_download | plugin | `transfer.rs`: rounds, Range, manual redirects (≤8), 16 MiB/60 s cursor, part file, Retry-After, spacing | prior-art §1.4, §3, §5; `G/crates/gents/src/plugin/{http_calls.rs,rounds.rs}`; `K/packs/gents/ocr/plugins/ocr/source/remote.rs` | `Q/plugins/download_fetch/source/transfer.rs` | `cargo test` with canned rounds and an injected clock: first round, continue, redirect chain, server ignoring Range above 1 MiB refused, 60 s cutoff | M | BD-01 | opus |
| BD-F1b | browser_download | plugin | `finalize.rs`: sniff, `citation_pdf_url`, SHA-256 re-read, md5/sha1/size, dedup, placement, attempts provenance, FetchedSource (`source_id` both forms); agent-mode host check | prior-art §3, §5 | `Q/plugins/download_fetch/source/finalize.rs`, `Q/plugins/download_fetch/source/testdata/finalize/**` | native tests over a temp library copy (finalize moves and deletes part files, F-41): agent part ok, agent wrong host refused, HTML part refused, dedup, checksum mismatch → failed; landing page → PDF retry round; JSON cases only for refusals before any file is touched | M | BD-01 | sonnet |
| BD-B1 | browser_download | behavior-port | source_finder (new) | prior-art §2-4; `K/packs/gents/web_deep_research` | `Q/agent_behaviors/source_finder/system_prompt.md`, `Q/tasks/source_finder/prompt.md`, `Q/.build/fragments/source_finder.json` | jq: tool allow-list is exactly `web_search`, `web_scrape_url`, `write_source_search_result`; trigger `concurrency: "queued_serial"`; `grep -c` finds each of the 4 rule sentences; no denylisted host appears; no goal fields | S | BD-02 | sonnet |
| BD-B2 | browser_download | behavior-port | fetch_agent (v0.20 fallback, §3.6) | §3.6; guide §11 (host bash) | `Q/agent_behaviors/fetch_agent/system_prompt.md`, `Q/tasks/fetch_agent/prompt.md`, `Q/.build/fragments/fetch_agent.json` | jq: bash `allowed_argv_prefixes` equals §3.6 exactly, `root` is `${GENTS_DOWNLOAD_LIBRARY:-.}`, MCP tools exactly `web_search`, `web_scrape_url`; trigger `concurrency: "queued_serial"`; the same 4 rule sentences; writes only under `.parts/` (stated) | S | BD-02 | sonnet |
| BD-W1 | browser_download | wiring | Pipeline fragment, merge by id, final manifest, integration | §3.4, §3.6 | `Q/.build/fragments/pipeline.json`, `Q/pack_config.json`, `Q/manifest.json`, `Q/**` (integration fixes, logged) | `make -C $W/BD-W1 test-browser_download` passes; no placeholder left; outputs equal `common/contract.rs` port lists; no `serial` trigger; `.build/` removed | M | BD-02, BD-C1, BD-C2, BD-C3, BD-F1a, BD-F1b, BD-B1, BD-B2 | opus |
| BD-T1 | browser_download | test | install.json and defs.json | §3.5 | `Q/tests/**` | `make -C $W/BD-T1 test-browser_download` green, with `install.json` installing after X-02's service registration; fallback if X-02 was refused: `defs.json` only and the gap recorded in `Q/README.md` | S | BD-W1, X-02 | sonnet |
| BD-D1 | browser_download | docs | README: scope, resolvers, rights rules, library and allowed folders, `GENTS_DOWNLOAD_LIBRARY`, registering `web-research-mcp` before install, `--grant-authority`, no-email rule, fetch modes, `queued_serial` finder, the v0.20 trade-off and the agent-mode exfiltration path (F-35) | §3, prior-art | `Q/README.md` | `## ` sections; check passes | S | BD-W1 | sonnet |
| X-E2E | both | test | End-to-end script and run on Thurston 1894 | §4 | `H/e2e/run.sh`, `H/e2e/expect.md`, `H/e2e/RESULTS.md` | every **must** criterion green; each **should** failure recorded with its reason and a filed follow-up; `run.sh` uses `gents version` and step 5b | M | SB-T1, SB-D1, BD-T1, BD-D1, X-02 | opus |
| SB-X1 | structured_book | plugin | (follow-up) EPUB export (`book_epub`), callback on `EpubExportJob` | `S/epub/*`, `S/server/endpoints/books_export_epub.go` | `P/plugins/book/source/stages/epub_export.rs`, `P/schemas/epub_export*.graphql` (manifest/config edits in its own later merge) | epubcheck-clean output for the fixture book | L | SB-W2b | sonnet |

Critical path: X-00a → X-01 → SB-01a → SB-01b → SB-G0 → SB-C2/C3 → SB-C4a → SB-P1 → SB-W2a → SB-W2b → SB-T1 → X-E2E.
SB-C10 runs in parallel from SB-01a. SB-01 (revision 2) is now SB-01a + SB-01b + SB-01c; no task keeps the id `SB-01`.

---

## Decisions for the operator

1. **Runtime.** (a) Build gents `origin/main` at `d4df8a02b` (or later; that
   is also the floor, because it has the #2343 cursor fix and the #2379
   hard-link fix) and run all three packs on it (recommended; it is what
   packs CI tests against). (b) Use the `v0.20.0` tag: structured_book is
   for **small libraries only and unverified on macOS** (spike R22: the
   tag's 10,000-row rescan limit and its hard-linking file bind), with
   `link_mode: copy` only, and browser_download works only with
   `fetch_mode: "agent"`. The installed `~/bin/gents` (`f44a0cef0`) cannot
   run any of the three; it fails `pack check` even on `gents/ocr`.
2. **Agent fetch.** `fetch_mode: "agent"` gives a model host `curl` with
   network, outside the WASM sandbox. The argv prefix limits only the head
   of the command, so the agent can also upload any file under
   `GENTS_DOWNLOAD_LIBRARY` to any HTTPS host (F-35), and its rights checks
   are model-reported (`needs_review: true`). Accept it (on the tag, or on
   main as an alternative to plugin fetch), or wait for a gents release with
   `http_calls`.
3. **Unknown-license fetch for the e2e item.** IA gives no license field for
   the 1894 Thurston scan, so the e2e job sets `allow_unknown_license: true`
   on the basis of the publication date. Confirm that this is acceptable.
4. **Upstream work.** Whether to file the gents issue for concurrent callback
   execution (F-17), and whether to approve the later `gents/ocr` change
   (copy `path` onto `OcrDocument`; add `max_attempts` to its callbacks).
5. **Shared packs harness.** Approve task X-02, a change to
   `packs/scripts/test-pack.sh` (used by every pack) so documents-pack
   installs register declared external services, as graph installs already
   do. Without it, browser_download's `install.json` cannot run in `make
   test` (its install needs a registered `web-research-mcp`, F-40) and BD-T1
   ships `defs.json` only.

---

## Revision log

Review ids: `FID-*` = REVIEW-fidelity.md, `FEAS-*` = REVIEW-gents-feasibility.md,
`WL-*` = REVIEW-work-list.md. "Adopted" means the review's fix was applied as
written; "modified" means the goal was kept and the mechanism changed, with
the code evidence.

### Blocking

| Id | Resolution |
| --- | --- |
| FID-F1 | Modified. (1) Gate completeness is checked in the **bound** `stage:<gate>` handler, not in `gate_open`, because grouped calls are never bound (F-3); missing link/finish members fail the book; front is allowed partial. (2) Every model stage, single-item ones included, goes through item → batch → stage with openers (decision 5). (3) Task-failure retries via `FireOutcome` (fact row 9 was wrong; F-7): `emit_outcome: true`. *Revision 2: the `session_id_template` correlation was wrong (F-26) and `emit_outcome` needs `handoff_id` (F-25); see FEAS-RB-1/RB-2 below.* The review's "bound stage creates `StageTask{attempt+1}`" cannot work because `FireOutcome` has no `workspace` (schema checked), so the retry is a join of a `TaskFailed` marker with the original `StageTask` (§2.6, §2.8). Spike R17; fallback D14 with 1,800 s timeouts for single-item stages. Finish watchdog added (decision 4.4) |
| FID-F2 | Adopted. Default `toc_policy: require`; `fallback` and `whole_book` opt-in (D1); zero-entry case defined in `gap_closed`; cases in SB-S7b and SB-S8a |
| FID-F3 | Adopted. `ocr_failure_policy: fail` (default) or `quarantine` (per page where rows exist); acceptance changed in SB-S2; D2 |
| FID-F4 | Adopted. Metadata exhausted → failed; `degraded` means quarantined pages under the opt-in policy only |
| FID-F5 | Adopted. `core/schemas/*.json` from SB-G0, `core/validate.rs` (new task SB-C9), `jsonschema` crate checked in SB-01 with a hand-written fallback; repair feedback counts in the budget; reject cases in S4-S8b |
| FID-F6 | Adopted (explicit arguments, change C7). The read_write alternative is rejected: it would make a model tool writable for one prompt's bookkeeping |
| FID-F7 | Adopted. Latest canonical always; `digest` only on `read_passage_at_version`; decision 10 corrected; SB-P2 case |
| FID-F8 | Modified. metadata only when variant ≠ ocr-only; `photo-book` alias (D16); single finish: unique `signal_ref` (FEAS-B7) plus a `status.json` marker in the **bound** `stage:finish`, because the review's O_EXCL in `gate_open` is impossible (grouped call is unbound, F-3); variant × finish cases in SB-S3 |
| FEAS-B1 | Adopted. Every `*_key` renamed to `*_ref` (`task_ref`, `item_ref`, `batch_ref`, `stage_ref`, `gate_ref`, `chunk_ref`, `cause_ref`, `entry_ref`, `gap_ref`, `unique_ref`); C6 now `entry_doc_id → entry_ref`; naming rule (decision 11) and `defs.json` assertion for both packs. Verified `is_secret_env_name` and both binding checks on main |
| FEAS-B2 | Adopted. Surfaces fill only `run_id`, `stage`, `task_ref`, `item_ref`, `workspace` (all String); attempt parsed from `task_ref`, batch fields from the task file; `StageTask` uses `"-"`, never null; browser_download fills only `library_path` and `plan_json`. Verified `defra_write/mod.rs` and `input.rs::literal` |
| FEAS-B3 | Adopted. `workspace`/`workspace_original` and `library_path`/`library_root` declared; `original_field` dropped from the two read-only tools. Verified `pack.rs:455-480` |
| FEAS-B4 | Adopted. `--grant-authority` on all three installs (§0.1, §2.1, §4, READMEs). Verified `plugin/store.rs:197` |
| FEAS-B5 | Modified. Serial execution verified (`scan.rs:589-600`). The review's "min_count = expected" for correctness gates is rejected: by `group_candidate_eligible`, such a group that never fills stays dormant forever, which is a silent hang. Instead `min_count: 1`, generous timeouts, and the bound handler fails visibly on a partial set; plus the finish watchdog. Fetch calls capped at 60 s; throughput in README; gents issue is an operator decision |
| FEAS-B6 | Adopted. Openers per batch and per stage (decision 5.2) and the finish opener at ingest. Verified `evaluate_group` returns `Empty` before creating state |
| FEAS-B7 | Adopted. Unique `task_ref` on results, `signal_ref`, `outcome_ref`, `batch_ref`, `spawn_ref`; duplicate handlers fail their own transaction (F-19, verified `commit_success`); R19 confirms the surface side |
| FEAS-B8 | Adopted. Pinned to `origin/main` ≥ `c9d26b8d1` (verified ancestor of main, not of v0.20.0); e2e seeds with `gents document create` and retries; `defs.json` checks foreign `input_fields` |
| WL-B1 | Adopted. SB-01 and BD-01 ship check-green, test-green packs with every asset as a placeholder; later tasks only replace content; acceptance is `gents pack test` in the worktree; SB-02 escape clause removed |
| WL-B2 | Adopted. One worktree per task; SB-01 commits `Cargo.lock`, nobody else commits changes to it (the review's rules addition "never commit Cargo.lock" read as this) |
| WL-B3 | Adopted. New task SB-G0 with `go test -overlay`; C3, C5-C9, S7a, S7b, S10 depend on it. Verified `module github.com/jackzampolin/shelf` and the unexported targets (`appendLinkTocRetryHint`, `(*Job).findFinalizeGaps`, `(*Job).generateEntriesToFind`) |
| WL-B4 | Adopted. `core/contract.rs` (§1.1) owned by SB-01 with file structs, payloads, port-name consts, stage table and refs |
| WL-B5 | Adopted. `metadata_prepare` dropped; the front gate starts metadata and toc_finder; front `expected` counts intersecting chunks (images `min(30,N)`); liveness fixed by openers and single-item batches (verified `graph.rs` `_ => vec![""]`); per-gate partial policy stated in decision 4.3 |

### Non-blocking (fidelity)

| Id | Resolution |
| --- | --- |
| FID-N1 | Adopted: gap 3, pattern row, discover invalid → not_found; Task failures share budgets |
| FID-N2 | Adopted: C8; tool name written per file; retry text in exactly one place per stage; rendered link retry golden in SB-C7 |
| FID-N3 | Adopted: D4 recorded; new `check_candidate` op (SB-C4a, SB-P1); C5 tells the agent to use it |
| FID-N4 | Adopted with spike: `goal_token_budget` (F-23) per agent, R20; D18 |
| FID-N5 | Adopted: D5; e2e records peak context |
| FID-N6 | Adopted: D6 and the slot description |
| FID-N7 | Adopted: `gap_closed` writes TocEntry revisions (SB-S7b) |
| FID-N8 | Adopted: paragraph-first ApplyEdits, non-audio passthrough, byte-equal test (§2.8, SB-S8b) |
| FID-N9 | Adopted: `extract_complete`, `polish_complete`, `polish_retries` added; all-or-nothing writes verified (F-19) and stated as the replacement |
| FID-N10 | Adopted: D11 and decision 10 |
| FID-N11 | Adopted: consumer rule in §2.4; `book_research` reads only the canonical file |
| FID-N12 | Adopted: `source_paths`, refused above one (D12, SB-S1 case) |
| FID-N13 | Adopted: D13 recorded |
| FID-N14 | Adopted: SB-P2 acceptance lists every bound, readiness and `get_book` fields |
| FID-N15 | Adopted: `author`; `scan_page` optional; D7; D8; counters on `BookStatus` (D9); D10 |

### Non-blocking (feasibility)

| Id | Resolution |
| --- | --- |
| FEAS-N1 | Adopted: batches by count and ≤3 MiB; commit parts; R16 for page_assemble |
| FEAS-N2 | Adopted: sequential batch release (≤32 in flight per stage per book); `max_queue_depth 4096` in e2e; D3 |
| FEAS-N3 | Adopted: `max_attempts: 3` on every callback (verified default 1, `plugin.rs:994`); idempotent writes; "stuck after restart" in README |
| FEAS-N4 | Adopted: image folders above 256 files refused (one chunk per file, verified `graph.rs`; the 256 bound verified in `expected_count`), so the `ocr` gate never quiesces; R12 threshold corrected from 1,000 |
| FEAS-N5 | Deferred, not rejected: it edits `gents/ocr`, so it is a later phase and an operator decision; the join stays |
| FEAS-N6 | Adopted as sizing: 24 h/72 h timeouts; per-gate sources keep grouped collections few |
| FEAS-N7 | Adopted: `*_json` stay `String` (verified `reject_empty_json_arrays`) |
| FEAS-N8 | Adopted with spike R18: one crate per pack, `sync-core` removed. *Superseded in revision 2 by WL-N3 (F-30): four crates, sync scripts* |
| FEAS-N9 | Adopted: `runtime_pages.json` mechanics and pack-level fixtures in `assets` |

### Work-list review, remaining items

| Id | Resolution |
| --- | --- |
| WL-G1 | Modified: `run_id` must start with `sb-`, and both OCR sources filter `run_id: {_like: "sb-%"}` (covers `OcrDocument`, which has no path, unlike the review's path filter); no-op on foreign runs; `structure: true` opt-in on `DownloadJob`/`FetchedSource`; R14 |
| WL-G2 | Adopted: R15, R16 |
| WL-G3 | Adopted: stage table in §2.5 and in `contract.rs`; asserted by `defs.json` |
| WL-G4 | Adopted: `_example.json` and `check-fragment.sh` in SB-01 |
| WL-G5 | Adopted: singletons in the day-one `pack_config.json` |
| WL-G6 | Resolved by FEAS-N8 (no copies); fallback `sync-core.sh` would be written by SB-01; CI claim dropped. *Revision 2: the sync scripts are primary (WL-N3)* |
| WL-G7 | Adopted via day-one placeholders |
| WL-G8 | Adopted: no Makefile edit (the pattern rule exists) |
| WL-G9 | Adopted: SB-D1 owns both rows in `K/README.md` |
| WL-G10 | Adopted: SB-T1 acceptance |
| WL-G11 | Adopted: `complete:false` case and `split_pages` port in SB-S2 |
| WL-G12 | Adopted: Chapter docs without `mechanical_text`; ≤400 paragraphs/3 MiB per commit item |
| WL-G13 | Adopted: X-01; SB-01 and BD-01 depend on it |
| WL-G14 | Adopted: C1 writes a task file for every item |
| WL-§3 overlaps | `main.rs` stubs: P1/P2 now own `tools/*.rs` modules, not `main.rs`; copies gone (N8); manifest final shape in SB-01; Makefile untouched; `K/README.md` SB-D1 only |
| WL-§4 ordering | Adopted: C3←C2; C4a/C4b←C1,C2,C3; C7←C3; P1←C1; P2←C1; S9 and S6, S3 compile against `contract.rs`; W1←01; BD-C2/C3←BD-01 (which owns `http.rs`); 01/BD-01←X-01 |
| WL-§5 splits | Adopted: X-00a/b/c, C4a/b, S7a/b, S8a/b, W2a/b, F1a/b; S10 and C3 kept whole |
| WL-§6 acceptance | Adopted for every listed task (named Shelf tests, check scripts, enumerated SB-P2 cases, SB-G0 goldens, X-E2E must/should, email regex) |
| WL-§7 tiers | Adopted: B4 sonnet; SB-02 haiku; S8b opus; BD-W1 opus; B2 kept sonnet; new BD-B2 sonnet |
| WL-A1 | Adopted: counts asserted from tables |
| WL-A2 | Adopted: `from_fetch` dispatch on `source_id` |
| WL-A3 | Adopted: crate list in SB-01 acceptance |
| WL-A4 | Adopted: "reject → feedback string" tables in S5 and S8b |
| WL-A5 | Adopted: D11 and SB-D1 |

### Operator-requested constraints (from the task)

| Item | Resolution |
| --- | --- |
| Target gents main, fetched | `origin/main` fetched again for revision 2 (`d4df8a02b`, unchanged since the round-1 reviews); floor raised to it; feature matrix in "Runtime target" |
| v0.20 fallback for downloads | The suggested "downloads via the web-research MCP" cannot deliver file bytes (F-24: no download tool; Markdown only; envelope hash). The fallback is `fetch_mode: "agent"` (§3.6): MCP to find, restricted host `curl` to fetch, deterministic plugin finalize (no network needed) |
| Legitimate sources only | Resolvers, prompts and the agent-mode host check allow only OA/public-domain/repository sources and operator-supplied URLs; no shadow-library resolver anywhere; `defs.json` asserts it |

### Round 2: items from the "Round 1 re-review" sections

Ids: `FID-R1-B*`/`FID-R1-N*` = REVIEW-fidelity.md (blocking / numbered
non-blocking), `FEAS-RB-*`/`FEAS-NB-*` = REVIEW-gents-feasibility.md,
`WL-N*`/`WL-W-*` = REVIEW-work-list.md. Every fix below was re-read in
source at gents `d4df8a02b` (and the tag where stated) before it was written.

**Blocking**

| Id | Resolution |
| --- | --- |
| FID-R1-B1 | Adopted. `StageTask.handoff_id` (= `task_ref`; `"-"` on TaskFailed) in §2.4; every StageTask writer sets it (decision 6); `sb-fire` correlates on `FireOutcome.source_handoff_id` and lists it in `input_fields` (§2.6); `FireOutcome.attempt` mirrors `StageTask.attempt` (§2.6 note); `defs.json` assertion (§2.10); F-7 corrected, F-25 added. Verified `desired_state.rs:565-626`, `trigger_engine/mod.rs:626,636-643`, `durable.rs:197-211`, and tag `desired_state.rs:518`, `mod.rs:636`. R17 reduced to the runtime half |
| FID-R1-B2 | Adopted, modified in naming. `BookStatus.status_ref @index(unique: true)`: `"<run_id>/terminal"` on terminal rows; progress rows get `"<run_id>/progress/<writer_ref>"`, so no row has a null unique value (DefraDB's null handling in a unique index is not visible from gents, so the design does not rely on it). Terminal rows are written alone; the marker holds `{status, reason, stage, writer_ref}`; the same writer re-emits on retry; `stage:finish` always emits (marker content, or computed) and a duplicate fails harmlessly (decision 4.4-4.5, §2.8). SB-S3 cases "marker written, commit failed → retry re-emits" and "marker present, no row → finish emits" added; SB-C1 tests the marker protocol |
| FEAS-RB-1 | Adopted. `session_id_template` removed from all triggers (§2.5, decision 6); fresh session per StageTask doc; `defs.json` asserts no template; F-26 added. Verified `trigger_engine/mod.rs:578-595,650-655`, `lifecycle/materialize.rs:1318-1334`, `durable.rs:20-30`, help text `help.rs:236,353`, tag `materialize.rs:1332` |
| FEAS-RB-2 | Adopted (same change as FID-R1-B1), plus: `sb-fire` event source and binding correlation `source_handoff_id`, its port on `task_ref`, because `validate_handler` refuses outputs without a source correlation (`callback/plugin.rs:69-73`, verified) |
| FEAS-RB-3 | Adopted. `BookJob.fetch_run_ref`; the `sb-from-fetch` port uses `correlation_field: "fetch_run_ref"` and the plugin writes `run_id = "sb-" + run_id` (§2.3, §2.4, §2.6). Verified `callback/plugin.rs:132-147` ("the plugin changed …, which the runtime writes"). `defs.json` names this exception |
| FEAS-RB-4 | Adopted. `cause_ref` added to `StageTask` and `ItemOutcome` (§2.4); rule in §2.3 that every grouped-port collection declares `cause_ref`; `defs.json` asserts port correlation fields exist in the SDL (F-27: `validate_handler` does not check them, verified `callback/plugin.rs:45-76`) |
| FEAS-RB-5 / WL-N6 | Adopted, extended. Target and floor `d4df8a02b` ("Runtime target", operator decision 1, §4 step 1). Verified `967339dfd` is on `origin/main` and `named_in_pinned_folder` exists there and not at the tag (F-33). Beyond the review: ingest **copies by default** (`BookJob.link_mode`), hard link is opt-in on main only, because a plugin cannot detect the gents build; this also removes the dependence on R8. R24 tests the hard-link option on macOS |
| WL-N1 | Adopted. SB-01 ships 9 placeholder behaviors and contexts with final ids, a stub for every `core/*` and stage module, and `merge.sh` merges by id (§2.2, SB-01); BD-01 the same for its 2 behaviors. Acceptance everywhere is `make -C <worktree> test-<pack>`, which builds first. Verified `pack/inference.rs:306`, `test.rs:44-52`, `check.rs:75-90`, `test-pack.sh:102-112` (F-28). SB-01 sized L |
| WL-N2 | Adopted. `sb-fetched`, `sb-ocr-chunk`, `sb-ocr-doc` unfiltered; `from_fetch`, `chunk_slot`, `chunk_text` return no output on optional ports (§1, §2.6); S1/S2 cases; R14 rewritten; R1 rewritten as a lone fresh-home install with the bridge pack as fallback for all three sources. Verified `check.rs:228-260` and the optional-port path `callback/plugin.rs:110-126` (F-29) |
| WL-N3 | Adopted. Four sources: `plugins/book` (`book_pipeline`), `plugins/book_tools` (`book_search` + `book_research`, identical `bind_dir` and limits), `plugins/download_resolve`, `plugins/download_fetch`; shared code copied by sync scripts that live under a plugin source (so no `assets` entry is needed, F-10), with a drift `#[test]` (decision 9, §2.1, §2.7, §3.1). R18 dropped. Verified `test.rs:93-140` and `build.rs::mirror` (`:358-378`: a directory symlink is read as a file) |
| WL-N4 | Adopted. `common/http.rs` is a pure `round()`; resolver and transfer tests are `cargo test`s with canned rounds; JSON cases only for no-network paths (§3.5, BD-01, BD-C1..C3, BD-F1a/b). Verified `plugin/rounds.rs:95-110` (F-31) |
| WL-N5 | Adopted. No `goal_token_budget` (and, from FEAS-NB-1, no `goal_objective_template`); D18 rewritten; R20 informational. Verified `test-pack.sh:192-193` (F-32) |
| WL-N7 | Adopted. Only the final commit part writes the chapter's `ItemOutcome`; part p re-derives its slice from `chapters/<id>.json` (§2.8 COMMIT); SB-S9 case. The R16 fallback (`WorkItem{pages, cursor}`) follows the same rule: only the last page part writes the chunk's signals |
| WL-B1 (round 0, re-opened) | Resolved by WL-N1 |
| WL-B2 (round 0, process gaps) | Resolved by WL-W-1 and WL-W-2 |

**Non-blocking, fidelity**

| Id | Resolution |
| --- | --- |
| FID-R1-N1 | Adopted: `RunFile.finish_expected` and one `finish_signal(member)` builder (decision 4.5, §1.1); SB-S3 case per variant |
| FID-R1-N2 | Adopted: exact front prefix; under `fail` a missing member releases nothing, under `quarantine` only quarantined holes pass (decision 4.3, §2.8, D8 rewritten). Shelf `state.go:57-61`, `state_book_pages.go:127-137` |
| FID-R1-N3 | Adopted: repair budget 2 per attempt in `task_ref` (`<attempt>.<repair>`), keeping Shelf's up to 9 generations (decision 6, §2.8, D24). Verified `maxStructuredRepairAttempts = 2` (`providers/structured_output.go:14`) |
| FID-R1-N4 | Adopted: null list → `[]` and stripping of CTX/port-only fields before validation (decision 6, SB-C9). Verified `defra_write/input.rs:104-106` |
| FID-R1-N5 | Adopted: `ApplyEdits` on the whole chapter text, paragraphs derived afterwards, `paragraph_pairing` recorded; counter-example and 3-newline test in SB-S8b. Verified Shelf `common/structure_text.go:164-173` |
| FID-R1-N6 | Adopted: link and toc_finder route schema repairs and Task-failure reasons through their hint builders; `feedback` stays empty for them (§2.5) |
| FID-R1-N7 | Adopted: D22 and the named failure reason (§2.8) |
| FID-R1-N8 | Recorded as D21. The suggested per-book timeout is rejected: group `timeout_secs` is a static integer, only `expected_count` can name a source field (`document_config/event_trigger.rs:242-366`, F-34). The README shows how to raise it after install |
| FID-R1-N9 | Adopted: every release handler checks the terminal marker first (decision 4.4, §2.8 "After a failure") |
| FID-R1-N10 | Recorded as D23; `latest_run.json` reported as `get_book.status_reason` (SB-C5, SB-P2) |
| FID-R1-N11 | Recorded in D14 (Shelf `job.go:312-345`, verified) |
| FID-R1-N12 | Adopted as R23 (same as FEAS-NB-5 and WL-W-11) |

**Non-blocking, feasibility**

| Id | Resolution |
| --- | --- |
| FEAS-B2 residual | Adopted: `DownloadJob.library_path` required, refused in `dl-resolve` (§3.2, BD-C1 case) |
| FEAS-NB-1 | Adopted (second option): no goal templates at all; an agent Task is one ordinary request (§2.2, D18). Verified the field help `self_config/command.rs:2569` ("requires built_ins.enable_goal_tools") |
| FEAS-NB-2 | Adopted: `root: "${GENTS_DOWNLOAD_LIBRARY:-.}"` (with a default, because `${VAR}` alone is an error when unset at check time); README and e2e step 4 export it. Verified `pack/interpolate.rs` on main and tag |
| FEAS-NB-3 | Adopted: §3.6 and operator decision 2 state the exfiltration path. Verified `first_matching_prefix` (`toolset/shared/command.rs:775-782`, F-35) |
| FEAS-NB-4 | Adopted: `closed` flag in task files set by `<stage>_next` and `<stage>_closed`; result handlers refuse closed items (§1) |
| FEAS-NB-5 | Adopted as R23, corrected: a timed-out group below `min_count` is marked **dormant**, not dropped (`trigger_engine/event_delivery.rs:388-397`); whether recovery skips dormant groups is what R23 measures |
| FEAS-NB-6 | Adopted: R17 stays first in X-00a and now needs only the runtime half |
| FEAS-NB-7 | Adopted: §2.8 "Throughput" and R21 say "probably per pack" |

**Non-blocking, work-list**

| Id | Resolution |
| --- | --- |
| WL-W-1 | Adopted: worktree cut rule and the orchestrator as integrator (§6 rules) |
| WL-W-2 | Adopted: `H/spike/{sb,bd,vision}.md`; X-01 concatenates into `SPIKE.md` |
| WL-W-3 | Adopted: X-00a builds `GENTS_BIN` in `../gents-pin`; X-00b now depends on X-00a for it |
| WL-W-4 | Adopted: (a) `TaskFile.user_prompt` and `prompt_bytes`; (b) `PatternFile`, `GapsFile`; (c) S10 listed as writer of `TocFiles`, `ChapterFile`, `BookMetadataFile`; (d) schemas only in `core/fixtures/schemas/` (SB-G0), `include_str!` in SB-C9 (§1.1, SB-G0, SB-C9) |
| WL-W-5 | Adopted: `stage:discover` (S7b) and `stage:classify` (S8b), started by `StageStart`s from S7a and S8a (§2.7, §2.8) |
| WL-W-6 | Adopted as the R17 fallback: `sb-items-single` (1,800 s) and `sb-items` (14,400 s) by filter on our own collection (§2.6) |
| WL-W-7 | Adopted: SB-G0 acceptance classifies each target as pure, httptest or hand-derived; `findFinalizeGaps` hand-derived. Verified `jobs/common/toc.go` loads through `svcctx.DefraClientFrom` |
| WL-W-8 | Adopted: SB-01 enumerates fixture names; SB-T1 may append to `assets` only (§2.1, §6 rules) |
| WL-W-9 | Adopted: zero-entry policy owned by S7b only; S8a reads `toc/final.json` (§2.7, SB-S8a) |
| WL-W-10 | Adopted: the `"-"` rule covers only the fill sources and `handoff_id`; `feedback` is empty or absent (§2.4) |
| WL-W-11 | Adopted as R23 |
| WL-W-12 | Adopted: §4 step 9 uses real JSON |
| WL-W-13 | Resolved by WL-N3 (one `cargo test` per source) |

**Other changes made while applying the above**

| Change | Why |
| --- | --- |
| `task_retry` no longer writes `StageTask`/`ItemOutcome`; it writes a `StageStart{task_retry}` and the bound `stage:task_retry` decides (§2.6, §2.8, SB-S3) | An unbound grouped call cannot see whether a result already arrived. If its "budget used, failed" `ItemOutcome` won the unique-`outcome_ref` race against a late valid result, the good result would be lost. The bound handler checks `results/` and `closed` first |
| Event-source count corrected from 26 to 25 (26 with the W-6 split) | Recounted from the §2.6 table |
| `TaskFile.closed`, `Chapter.paragraph_pairing`, `BookJob.link_mode` | Needed by FEAS-NB-4, FID-R1-N5 and FEAS-RB-5 |

### Round 3: items from the "Round 2 re-review" sections

Ids: `FID-R2-B1`, `FID-R2-N<k>` (numbered non-blocking list) = REVIEW-fidelity.md;
`FEAS-R2-<k>`, `FEAS-R2-N<k>` = REVIEW-gents-feasibility.md; `WL-R2-B<k>`,
`WL-W2-<k>` = REVIEW-work-list.md. gents `origin/main` was fetched again
(still `d4df8a02b`); every fix below was re-read in source there (and at the
tag or packs `cd00bb8` where stated) before it was written.

**Blocking**

| Id | Resolution |
| --- | --- |
| FID-R2-B1 | Adopted, modified in mechanism. (1) Ingest counts with `gents/ocr`'s own counter, copied verbatim into `plugins/book/source/ocrcount/` (new task SB-C10, sync script and drift `#[test]` against packs `ocr/plugins/ocr/source/`, decision 9). The review asked to copy `pdf.rs` and its dependencies; `pdf.rs` imports the hayro renderer, layout, OCR and remote modules (`pdf.rs:5-30`), so only the counting path is copied: `pdflazy.rs`, `pdfobj.rs`, `pdfdecode.rs`, `src.rs`, the `Backend::open`/`page_count` body (`pdf.rs:482-516`, whose fallback is `hayro_syntax::Pdf`, added as a dependency at the ocr crate's version) and `plan.rs::ranges` with its constants (`plan.rs:16-18, 58-75`), plus a shim for `input::Options` and `resume::Fnv` (the only outside imports, `pdflazy.rs:10`, `src.rs:9`). SB-S1 case: `chunks_total`/`ranges` equal the plan for the e2e PDF and ocr's fixture PDFs (expected values from ocr's `plan-mode.json`). (2) Image folders: only counted files, flat, `NNNN.<ext>`, passed as `OcrJob.files`, which ocr reads in that order instead of walking (`main.rs:400-415`); `page_assemble` maps `OcrChunk.source` through `RunFile.files` (§1.1, §2.8, D26). (3) Uncountable source → immediate refusal (D25). **Beyond the review:** a plan-agreement check, `sb-ocr-plan` (a grouped source on OcrChunk with a 30 min timeout and no count, F-43; all chunks of a plan land in one transaction, F-42/F-19) → `stage:ocr_plan`, fails the book with the first difference instead of a 72 h hang, so any remaining disagreement is visible within 30 minutes (§2.6, §2.8 "OCR plan agreement", SB-S2) |
| FEAS-R2-1 | Adopted, second option generalised: **`workspace` is the book folder** `books/<sha>` on every pipeline document (§1), and handlers address `runs/<run_id>/` inside it. So `stage:commit_closed` promotes into `canonical/` by a rename inside its one preopen, and `terminal()` writes `latest_run.json` (D23), with no extra stage. A §1 write rule names the only two writes outside `runs/<run_id>/`; SB-C1 tests it. The run folder stays the `book_search` binding (footer `{{ doc.workspace }}/runs/{{ doc.run_id }}`); `run_id` is restricted to one path segment (§2.3). Real-bind coverage: SB-T1 `runtime_commit.json` (a runtime case, because a `pack test` plugin case with `bind` would write into the committed fixture, WL-R2-B2/F-41). Verified `plugin/bound.rs:1-18`, `plugin/executor.rs:209-268` (F-37) |
| FEAS-R2-2 | Adopted (primary fix). `contract::CALLBACK_PORTS` + `callback_of(key)`; `main.rs` prints the bare document (or `null`) for single-port callbacks and a keyed object without absent keys otherwise (§2.7); the 9 single-port structured_book callbacks and the 4 browser_download ones are listed (§2.7, §3.4); `sb-task-failed` prints `null` on success; dispatch and wire-shape tests in SB-01b and BD-01; `defs.json`/`cargo test` check the port sets against `pack_config.json`. The alternative (a second port everywhere) is not taken: for `sb-task-failed` the second port's collection would need a `task_ref`-valued correlation field it does not have. Verified `callback/plugin.rs:80-126` (F-38) |
| FEAS-R2-3 | Adopted. `dl-source-finder` and `dl-fetch-agent` use `queued_serial` (§3.3, §3.6, BD-B1, BD-B2); `defs.json` in both packs refuses `serial`. Verified `trigger_engine/mod.rs:836-850`, `event_source/durable.rs:143-170`, help `help.rs:353`, and `QueuedSerial` at the tag (`document_config/trigger.rs:89`) (F-39) |
| WL-R2-B1 | Adopted. Rule "Mirrors": a task changing `core/` (or `common/`) runs the sync script and commits the mirror; every C task and SB-G0 lists `(+mirror)` targets; mirrors are an exception to "do not edit"; the integrator runs every `sync-*.sh --check` after each merge; tests in `core/` read only below `core/` (decision 9, §6). P1/P2 now compile against a current mirror. Verified `test-pack.sh:145-160` (in-place native `cargo test`) |
| WL-R2-B2 | Adopted. Stateful paths are native `cargo test`s over a temp-dir copy of `stages/testdata/<module>/**` with `Ctx.now` pinned; JSON cases only for write-free calls, `expect` only without paths or clocks; refusals are success outputs; fixture paths added to every S, P and BD-F1b task; acceptance adds "no modified tracked file after the run" (§2.10, §6). `Ctx { ws, now }` replaces the bare `ws` argument (§1.1, §2.7). Verified `GC/pack/test.rs:150-180`, `plugin/bound.rs:68-93` (F-41) |
| WL-R2-B3 | Adopted. New task X-02 (harness registers external services for documents installs and runtime cases); BD-T1 and X-E2E depend on it, with the jq-only fallback; e2e step 5b registers the service; R22 includes it; install order note in decision 1; operator decision 5. Verified `document_config/references.rs:394-400`, `test-pack.sh:243-315, 386-417` (F-40) |

**Non-blocking, fidelity**

| Id | Resolution |
| --- | --- |
| FID-R2-N1 | Adopted: `toc_finder_closed` releases toc_extract through the C1 fan-out helper with all openers (§2.7, §2.8); SB-S5 case "Task lost → `require` failure within one batch timeout" |
| FID-R2-N2 | Adopted (second option): skipped polish outcomes are written at their batch's release (C1 helper, §2.8); SB-S8b case with 40 audio and 30 non-audio chapters |
| FID-R2-N3 | Adopted: `stage:task_retry` acts only when the failed `task_ref` is the newest in the task file's attempt history (§2.6); SB-S3 case |
| FID-R2-N4 | Adopted: README names `stage:finish` and its repair (§5 note, SB-D1) |
| FID-R2-N5 | Adopted: progress `status_ref` gets `/<n>` (decision 4.4, §1.1); SB-C1/S2 cases |
| FID-R2-N6 | Adopted: D22 names both causes and takes the reason from `error`/`warnings`; X-00a measures `complete:false`. Verified packs `ctx.rs:16`, `graph.rs:120-140` (F-45) |
| FID-R2-N7 | Adopted: `members(format, variant)`, EPUB ignoring the variant (decision 4.5, §2.8, SB-S3 case); §2.5 corrected so a toc_finder Task-failure retry reuses the prompt (Shelf `createBookOpRetryUnit` passes `nil`); content and repair retries keep `PreviousAttempt` (D7) |
| FID-R2-N8 | Adopted: `run_id` must match `^sb-[A-Za-z0-9._:-]+$`; `item_ref` refuses `/`; `task_ref` parsed as exactly 4 segments (§2.3, §1.1) |
| FID-R2-N9 | Adopted: a null non-list field is left out when its schema disallows null (decision 6, SB-C9 case with toc_finder `toc_found=false`). Shelf `toc_finder/tools/write_toc_result.go:24-49` |
| FID-R2-N10 | Adopted (drop): `TocLinkResult.entry_ref` removed; the entry comes from the task file (§2.4, SB-02, SB-B4, SB-S6) |
| FID-R2-N11 | Adopted: D8 records the fewer-than-20-non-empty-pages difference |
| FID-R2-N12 | Adopted: the release rule exempts `stage:finish` and treats `stage:task_retry` as an ordinary `stage:*` handler; pre-folder ingest refusals write only the lone terminal row and no finish opener (decision 4.4, §2.8) |

**Non-blocking, feasibility**

| Id | Resolution |
| --- | --- |
| FEAS-R2-N1 | Adopted: `input_fields` table in §2.6 (fields checked against our SDL in §2.4 and packs `ocr_chunk.graphql`, `ocr_document.graphql`, prior-art §6 for FetchedSource); `defs.json` assertions (§2.10) |
| FEAS-R2-N2 | Adopted: §1 states the retry-on-commit-failure cost, §2.8 sizes it. Verified `callback/plugin.rs:230-248`, `workspace/journal.rs:62-78`, `callback/run.rs:719-775` |
| FEAS-R2-N3 | Adopted: `source_id = "<run_id>:<status>"` on rows without bytes (§3.2, BD-C1, BD-F1b) |
| FEAS-R2-N4 | Adopted: "Runtime target" and operator decision 1 say "small libraries only, macOS unverified" for the tag; R22 tests both risks. Verified tag `callback/scan.rs:22,579-599` (`SEEN_DOCS_SEED_LIMIT = 10_000`) and tag `plugin/bound.rs:12-16` |
| FEAS-R2-N5 | Adopted: README note (§5) and the later ocr change (operator decision 4). Verified packs `ocr/pack_config.json` (no `max_attempts`) (F-45) |
| FEAS-R2-N6 | Adopted: sized in §2.8 "Throughput" and X-00a |
| FEAS-RB-3 note | Adopted: `defs.json` asserts event-source and handler correlation fields are equal for per-document bindings (§2.10) |
| FEAS-RB-5 note | Adopted: covered by FEAS-R2-N4 and R22 |

**Non-blocking, work-list**

| Id | Resolution |
| --- | --- |
| WL-B1 residual | Adopted: SB-01a and BD-01 README skeletons carry a `## ` heading |
| WL-B3 residual | Adopted: `ValidateCandidatePage` named as pure in SB-G0 |
| WL-W2-1 | Adopted, narrowed: SB-S3 depends on SB-C7 (link's hint, `link_retry_prompt`); SB-C6 is not needed because a toc_finder Task-failure retry reuses the prompt (FID-R2-N7) |
| WL-W2-2 | Adopted: SB-G0 depends on SB-01b (it needs the pack, `sync-core.sh` and the contract types its inputs are shaped as) |
| WL-W2-3 | Adopted: SB-01 split into SB-01a (skeleton, sonnet M), SB-01b (contract and dispatch, opus M), SB-01c (scripts, sonnet S); B tasks need 01c and SB-02, C/S tasks need 01b |
| WL-W2-4 | Adopted: the 10 `TaskFile` context variants enumerated from Shelf's structs (§1.1); `SB-K<n>`/`BD-K<n>` tasks own contract changes (§6 rules) |
| WL-W2-5 | Adopted (both): SB-W2a's acceptance saves a possibly red `merge.log`, W2b turns it green; `check-fragment.sh` runs `pack check` on a temporary single-fragment merge, so B tasks and W1 depend on SB-02 |
| WL-W2-6 | Adopted: 1,000,000-byte prompt cap in the C1 helper with a case (§2.5, D27). Verified `template/mod.rs:40,89` (F-44) |
| WL-W2-7 | Adopted: `gents version` (§4 step 1, X-00a, X-E2E) |
| WL-W2-8 | Adopted: X-01 creates `feat/structured-book-packs` from `origin/master` and records the SHA; the integrator works in `$W/integrate`, never in `K` |
| WL-W2-9 | Adopted: absolute `W`; every command uses `$W/<id>` |
| WL-W2-10 | Adopted: `git clone --shared` into scratch for both `GENTS_BIN` and `GENTS_TAG_BIN`; nothing written under `G` |
| WL-W2-11 | Adopted: SB-G0 shapes inputs as `contract.rs` types and documents the mapping; otherwise the consuming C task owns the adapter |
| WL-W2-12 | Adopted: decision 9 says "once per plugin entry" (F-41) |

**Other changes made while applying the above**

| Change | Why |
| --- | --- |
| New facts F-37 to F-45 | Each fix above cites code; the facts table is where the citations live |
| New callback `plan_check` and key `stage:ocr_plan` (owned by SB-S2); counts 26/27 sources, 15 callbacks | FID-R2-B1 safety net |
| New tasks SB-01a/b/c (split of SB-01), SB-C10, X-02; ids of all other tasks kept | WL-W2-3, FID-R2-B1, WL-R2-B3 |
| D25-D27 | Record the port-only failure modes added by FID-R2-B1 and WL-W2-6, and the image renaming (D26) |
