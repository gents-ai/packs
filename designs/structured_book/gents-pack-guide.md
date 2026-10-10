# Authoring a Gents pack (v0.20 / gents `main`)

How to write, build, test, install and run a Gents pack. Everything here was
read from source, not from memory. The sources were:

- gents at `e8774ade3` (main, 2026-10-06; 94 commits after the `v0.20.0` tag
  `a5d02f106`), mostly `crates/gents/src/{pack.rs,pack/,plugin.rs,plugin/,callback/plugin.rs,document_config/,graph_pipeline/types.rs,template/}`
  and `crates/gents-cli/src/commands/pack/{check,test,build,scenario}.rs`.
- packs at `470c291` (main), `packs/gents/*`, `scripts/test-pack.sh`, CI
  workflows.

Where the prose README of either repo disagrees with the code, this guide
follows the code and says so.

---

## 0. Read this first: which `gents` binary

The `gents` on `PATH` (`~/bin/gents` -> `/Applications/Gents.app/Contents/MacOS/gents`)
reports `gents 0.20.0 (f44a0cef0)`. That commit comes *before* the `v0.20.0`
tag, so the binary is older than the packs repo `main` expects. Proof:

```
$ gents pack check packs/gents/ocr
"parsing packs/gents/ocr/manifest.json: unknown field `optional`, expected one of `name`, `description`, `behaviors`"
$ gents pack check packs/gents/web_deep_research
"decoding canonical pack configuration: unknown field `input_schema`, expected one of `name`, `collection`, `schema`, `input_contract`, `to`"
```

`gents/target/release/gents` (built 2026-09-28) fails the same way.
`pipeline` passes with both.

Packs CI builds gents from `main` (`cargo build -p gents-cli --release --locked`
in `.github/workflows/ci.yml`) and tests every pack against it. To author
against current behavior, build gents `main` yourself and pass it explicitly:

```sh
cd <gents checkout> && cargo build -p gents-cli --release --locked
make -C <packs checkout> test-ocr GENTS=<gents checkout>/target/release/gents
```

These features are in source `main` but not in the installed binary (all are
"Unreleased" in `CHANGELOG.md`):

- Plugin outbound HTTP through the host (`http_calls`, #2300).
- One plugin that both reads and writes (`bind_dir.access` + `write_fields`, #2301).
- Plugin tool results that carry images (#2302).
- `allowed_folders` in a scenario's `experiment.json` (#2303).
- `gents plugin dirs add|list|remove`, `gents plugin run --bind-dir`.
- `gents plugin bind|unbind`.
- `gents graph run --entry/--input/--field`. The old binary only has the
  hard-coded `--question`, `--base` and `--head` flags.
- Optional inference slots (`"optional": true`).
- Graph entry `input_schema` and `prepare`.

Stale prose to ignore:

- `packs/README.md` ("Plugins") says an admitted plugin "gets no filesystem,
  network or environment access, whatever it declares" and that the model-tool
  and graph-stage call paths are "not wired yet".
- The doc comment on `PackMetadata.plugins` in `pack.rs` says the same.

Both are wrong as of `main`:

- `plugin/tool.rs` offers plugins as model tools.
- `callback/plugin.rs` and graph plugin nodes run them as stages.
- `plugin/executor.rs` runs each call under `record.ceiling()`, which is the
  grant recorded at install.
- `--grant-authority` records what the plugin declared (`plugin/authority.rs::grant_for`).

The doc comment on `PluginRunner::CEILING` is accurate only for
`PluginRunner::compile()`, which is called without an install record.

---

## 1. What a pack is

A pack is a directory, `packs/<ns>/<name>/` by convention, holding a
`manifest.json` and every file that manifest declares in `assets`. Only
declared files travel. `gents pack build` writes one `.pack` file, a gzip tar
whose first entry is `pack.json`. The pack is named by a `sha256:` digest over
its declared contents, not over the container.

There are four `kind`s (`PackKind` in `pack.rs`):

| kind | Installs | Needs `config` | Notes |
| --- | --- | --- | --- |
| `documents` | Desired-state config documents (behaviors, tools, tasks, triggers, callbacks, surfaces, ...) plus SDL schemas plus its plugins | yes | The only kind that may declare `dependencies`, and those must be `graph` packs |
| `graph` | The same documents, plus a compiled graph (`GraphDefinition` and `GraphRevision`, with derived `EventSource`/`Trigger` documents named `graph-trigger-*`) | yes | Needs `compiler_version`. `gents graph run` drives it |
| `plugins` | Only plugins, into the home's plugin store | no | They can be called by name from any pack in the same home |
| `assets` | Files materialized under `<home>/packs/` | no | |

Installing any kind first installs that pack's plugins into the shared per-home
plugin store, then writes its documents. If writing the documents fails, the
plugin records are rolled back.

---

## 2. `manifest.json`, field by field

The struct is `PackManifest` + flattened `PackMetadata` (`crates/gents/src/pack.rs`).
It uses `deny_unknown_fields`, so a typo is an error.

| Field | Type | Req. | Meaning |
| --- | --- | --- | --- |
| `manifest_version` | int | yes | `1` |
| `name` | string | yes | snake_case; the coordinate is `namespace/name` |
| `namespace` | string | no | Defaults to `gents`. Third-party packs use their own (`acme`). Plugins install under it |
| `version` | semver string | yes | |
| `description` | string | yes | |
| `authors` | [string] | yes | |
| `tags` | [string] | no | Discovery only |
| `kind` | `documents` \| `graph` \| `assets` \| `plugins` | yes | |
| `assets` | [path] | yes | Every file that travels, including `README.md`, `pack_config.json`, prompt sidecars, schemas, `tests/*.json`, `plugins/<name>.afb`, `plugins/<name>/TOOL.md` and the generated `graphs/<id>.plan.json`. Plugin *sources* are not assets; they are built into the `.afb` |
| `config` | path | doc/graph | The `pack_config.json` asset. Sidecar paths in it resolve relative to this file |
| `schemas` | [path] | no | `.graphql` SDL files, applied before the documents |
| `dependencies` | [coordinate] | no | `name` or `ns/name`, never a version. Only `documents` packs may declare them, and each must be a `graph` pack (`pack.rs:685`, `installation/documents.rs:52`). Only `grok_tui_port -> code_review` uses this today |
| `inference_slots` | [slot] | doc/graph with behaviors | See §8 |
| `plugins` | [plugin] | no | See §5 |
| `compiler_version` | string | graph | `"graph-intent-v4"` in every current graph pack |
| `external_dependencies` | [{`service_id`,`description`,`repository_url`,`install_command`}] | no | Documentation only, never executed. A graph-install test registers these services first. `web_deep_research` declares its MCP service this way |

The inference slot struct is `PackInferenceSlot`, with `deny_unknown_fields`:

```json
{ "name": "worker", "description": "...", "behaviors": ["behavior-id", "..."], "optional": false }
```

`behaviors` may be empty only for a slot that just a plugin's `model_slot`
uses. Only such a slot may be `optional: true`.

The authoring standard, enforced by `gents pack check`, `scripts/test-pack.sh`
and CI:

- Every behavior is in exactly one slot, and its `inference_profile_id` is
  `gents:inference-slot:<slot>`.
- The pack authors no `inference_backends`, `inference_profiles`,
  `inference_sampling`, `inference_execution` or `inference_retry_policies`.
- No task sets `goal_token_budget`.
- `agent_principal.default_behavior_id` is never set (`loader.rs::ensure_pack_leaves_default_unselected`).
- Every file in the directory is declared, and every declared file exists
  (except `runs/`, `target/` and dotfiles, which are skipped).
- The README has `## ` sections. A graph pack's README carries the generated
  Mermaid block (`gents pack graph <dir> --write-readme`).
- Every event-source `filter` is valid GraphQL against the runtime and pack
  schemas.
- `experiment.json`, if present, loads.

### Minimal manifests

**documents** (copied from what `gents pack new x --template automation` scaffolds):

```json
{
  "manifest_version": 1,
  "name": "book_stage",
  "namespace": "acme",
  "version": "0.1.0",
  "description": "One-stage document pipeline.",
  "authors": ["acme contributors"],
  "tags": [],
  "kind": "documents",
  "config": "pack_config.json",
  "schemas": ["schemas/book_job.graphql"],
  "dependencies": [],
  "inference_slots": [
    { "name": "worker", "description": "Does the work.", "behaviors": ["book-worker"] }
  ],
  "assets": [
    "README.md",
    "pack_config.json",
    "schemas/book_job.graphql",
    "agent_behaviors/worker/system_prompt.md",
    "tasks/worker_task/prompt.md"
  ]
}
```

**plugins** (copied from `gents pack new x --template plugin-tool`):

```json
{
  "manifest_version": 1,
  "name": "fetcher",
  "namespace": "acme",
  "version": "0.1.0",
  "description": "A fetch plugin.",
  "authors": ["acme contributors"],
  "tags": [],
  "kind": "plugins",
  "dependencies": [],
  "assets": ["README.md", "plugins/fetcher.afb", "plugins/fetcher/TOOL.md"],
  "plugins": [{
    "name": "fetcher",
    "description": "What the model is told this does.",
    "artifact": "plugins/fetcher.afb",
    "source": "plugins/fetcher",
    "language": "rust",
    "instructions": "plugins/fetcher/TOOL.md",
    "input_schema": { "type": "object" }
  }]
}
```

**graph**: the documents manifest plus `"kind": "graph"`,
`"compiler_version": "graph-intent-v4"` and `"graphs/<graph_id_snake>.plan.json"`
in `assets`. `gents pack build` writes that plan file and adds it to `assets`
if it is missing (`build.rs::build_graph_plans`).

**assets**: `kind: "assets"` and `assets`, with no `config`.

---

## 3. `pack_config.json`, field by field

The struct is `PackConfig` (`document_config/pack_config.rs`), with
`deny_unknown_fields`. Each top-level key is an array of canonical documents.

The loader (`pack/loader.rs`) does the following:

- It fills a missing `agent_did` on every document from the install target.
  You never write DIDs. `${GENTS_PACK_AGENT_DID}` interpolates to the owner,
  and `allowed_callers` uses it.
- It interpolates `${VAR}` and `${VAR:-default}` from the environment. It does
  not interpolate the *contents* of sidecar files.
- It hydrates sidecars. A string field that holds `./relative/path.md`
  (`system_prompt`, `prompt_template`) is replaced by that file's contents.
- It pins the pack's own plugins (§5.7).

| Key | Document | Key fields (the rest are optional) |
| --- | --- | --- |
| `agent_principal` | AgentPrincipal | Always `{}` in a pack. Never set `default_behavior_id` |
| `agent_behaviors` | AgentBehavior | `behavior_id`, `context_id`, `display_name`, `description`, `inference_profile_id: "gents:inference-slot:<slot>"` |
| `contexts` | AgentContext | `context_id`, `system_prompt` (sidecar), `tools_id`, `compaction_id`, `display_name` |
| `compactions` | CompactionConfig | `compaction_id`, `threshold` (for example 0.85) |
| `tools` | Tools | `tools_id`, plus the groups below |
| `datastore_tool_surfaces` | DatastoreToolSurface | `surface_id`, `entries[]` (§6.3) |
| `tasks` | Task | `task_id`, `behavior_id`, `prompt_template` (sidecar, minijinja), `goal_objective_template`, `emit_outcome`, `output_schema_ref`, `hooks[]` (`hook_id`, `phase` = `before`, `after_success`, `after_failure` or `finally`, `command` argv, `timeout_secs`), `enabled` |
| `event_sources` | EventSource | `event_source_id`, `source_collection`, `event_kind` (only `"created"` is accepted), `filter` (a GraphQL filter fragment), `correlation_field`, `group` {`expected_count`: N or {`source_field`}, `timeout_secs`, `min_count`}, `workspace_authority` |
| `triggers` | Trigger | `trigger_id`, `task_id`, `source` {`kind`:`event`,`event_source_id`} or {`kind`:`schedule`,`schedule_id`}, `concurrency` (`parallel` (default), `serial`, `queued_serial` or `latest_only`), `session_id_template`, `enabled` |
| `schedules` | Schedule | `schedule_id`, `cadence` {`kind`:`interval`,`interval_secs`} or {`kind`:`cron`,`expression`,`timezone`,`missed_run_policy`} |
| `callbacks` | Callback | `callback_id`, `handler` (`built_in`, `module` or `plugin`; §6.4), `capabilities` |
| `callback_bindings` | CallbackBinding | `binding_id`, `event_source_id`, `callback_id`, `input_fields` (exact source fields passed to the handler; empty passes nothing) |
| `callback_modules` | CallbackModule | The legacy raw-WASM callback module. Use plugins instead |
| `skills` | Skill | `skills/<id>/SKILL.md` (via `gents pack add skill`) |
| `subagent_targets` | SubagentTarget | The targets `agent_new`/`agent_message` may address |
| `graphs` | GraphDefinition | `[{ "graph_id": "..." }]` |
| `graph_capabilities` | StageCapability | `capability_id`, `revision`, `target` {`kind`:`task`,`task_id`} or {`kind`:`plugin`,`plugin`,`digest`,`max_attempts`}, `input_ports[]`, `output_ports[]`, `allowed_callers`, `workspace_authority` |
| `graph_intents` | GraphIntent | `graph_id`, `nodes[]`, `edges[]`, `entries[]`, `results[]`, `limits` (§7) |
| `repository_placements`, `eval_definitions`, `tool_service_registries`, `projection_acp_bindings`, `chain_key_bindings`, `eth_tools` | | Specialized. `tool_service_registries` (MCP services) is accepted by the struct, but official packs leave MCP registration to the operator and document it in `external_dependencies` |
| `inference_*` | | Forbidden in packs |

### `Tools` groups (`document_config/tools.rs`)

```jsonc
{
  "tools_id": "x-tools",
  "built_ins": {
    "enable_goal_tools": true,          // get_goal/update_goal; pair with Task.goal_objective_template
    "enable_goal_creation": false,
    "enable_graph_tools": false,        // graph discovery/run/status/result/cancel
    "enable_memory": false,
    "enable_session_history_tool": true,// default true
    "enable_context_budget": false,
    "enable_schema_tool": false
  },
  "datastore": {
    "datastore_tool_surface_ids": ["x-surface"], // bounded create/query tools (preferred)
    "enable_defra_query": false,                 // generic read-only query
    "defra_query_collections": ["Coll"],
    "write_collections": []                      // generic write endpoint; empty denies all
  },
  "host": {
    "root": "${SOME_ROOT:-.}",
    "files": { "mode": "Off" },                  // Off | ReadOnly | ReadWrite
    "bash":  { "mode": "Off" },                  // Off | ReadOnly | Unrestricted
                                                 // + execution_mode: read_only|workspace_write|artifact_write|unrestricted
                                                 // + network_mode: inherit|disabled|enabled, allowed_argv_prefixes, timeouts...
    "cli": [ { "name": "<host-registered cli tool>" } ]
  },
  "remote": { "services": [ { "mcp_service_id": "svc", "tool_names": ["exact_name"], "required": true, "style": "discovery" } ] },
  "integrations": {
    "plugins": [ { "plugin": "gents/ocr" } ],    // installed plugin offered as a model tool named "ocr"; optional "digest" pin
    "lsp": null
  },
  "subagents": { "enabled": false, "target_ids": [] },
  "self_config": { "enable_self_config": false }
}
```

A missing group exposes nothing in that category. The mode enums are
PascalCase (`ReadOnly`). `execution_mode` and `network_mode` are snake_case.

### Task templates

Task templates use minijinja with strict undefined names
(`template/mod.rs`). The limits are a 64 KiB template and 1 MiB of rendered
output. The scopes are:

- `doc.*`: the source document's fields. `{{ doc.markdown }}` delivers a whole
  field. Reads through a datastore query tool do not (§6.3).
- `event.*`: `trigger_id`, `source_collection`, `source_doc_id`, `correlation`.
- `group.*` (grouped sources): `correlation_value`, `count`, `docs`,
  `complete`, `state_doc_id`.
- `ctx.now`, `node.node_did`, `node.behavior_id`, `session.session_id` and
  `request.request_id`.

### Minimal `pack_config.json` (one event-triggered stage)

```json
{
  "agent_principal": {},
  "agent_behaviors": [
    { "behavior_id": "book-worker", "context_id": "book-worker-context",
      "display_name": "Book worker", "inference_profile_id": "gents:inference-slot:worker" }
  ],
  "contexts": [
    { "context_id": "book-worker-context", "display_name": "Book worker",
      "system_prompt": "./agent_behaviors/worker/system_prompt.md", "tools_id": "book-worker-tools" }
  ],
  "tools": [ { "tools_id": "book-worker-tools", "display_name": "No tools" } ],
  "tasks": [
    { "task_id": "book-worker-task", "behavior_id": "book-worker",
      "display_name": "Work one job", "prompt_template": "./tasks/worker_task/prompt.md" }
  ],
  "event_sources": [
    { "event_source_id": "book-job", "source_collection": "BookJob",
      "event_kind": "created", "correlation_field": "job_id" }
  ],
  "triggers": [
    { "trigger_id": "book-job", "task_id": "book-worker-task", "concurrency": "parallel",
      "source": { "kind": "event", "event_source_id": "book-job" } }
  ]
}
```

`schemas/book_job.graphql`: `type BookJob { job_id: String @index  prompt: String }`
(`@index`, `@index(unique: true)` and `@immutable` are used in `ocr/schemas`).

---

## 4. The CLI you will use

| Command | Purpose |
| --- | --- |
| `gents pack new <name> [--kind K] [--template minimal\|automation\|graph\|plugin-tool\|assets] [--namespace NS] [--language L]` | Scaffold `./<name>` |
| `gents pack init` | Scaffold in the current directory |
| `gents pack add behavior <id> [--slot S]` / `task <id> --behavior B` / `trigger <id> --task T --on Collection` / `graph` / `stage <node> --graph G (--task T\|--plugin P) --input C --output C [--from node.port]` / `plugin <name> [--language L] [--prebuilt F.afb]` / `skill` / `schema <Collection>` | Add a part and record it in `manifest.json` |
| `gents pack remove-part`, `gents pack fmt`, `gents pack diff A B` | Edit, canonicalize, compare |
| `gents pack check [dirs]` | Every install-time validation. Writes nothing |
| `gents pack build [dir] [--out F] [--all]` | Compile plugins (`wasm32-wasip1` via Afterburner), compile graph plans, write `<ns>.<name>-<ver>.pack` |
| `gents pack test [dir] [--scenario]` | check + build + every plugin's `plugins/<name>/tests/*.json` case. `--scenario` also runs `experiment.json` (needs a model) |
| `gents pack verify F.pack`, `pack publish`, `pack fetch SPEC --store`, `pack search/info/outdated/update/yank/owner/login` | Registry (default `https://registry.dev.gents.xyz`, `--registry`, `GENTS_REGISTRY`) |
| `gents pack install <dir\|.pack\|sha256:\|ns/name[@ver]> --home H [--inference-slot slot=profile_id]... [--preview] [--grant-authority] [--overwrite\|--keep] [--bindings F.json]` | Install. Never seeds and never prunes |
| `gents pack remove <ns/name> --home H`, `pack prune` | Undo an install, prune the asset cache |
| `gents pack scenario run <dir> [--home H] [--prompt P] [--job-id J] [--http-port N] [--keep-home]`, `scenario init`, `scenario seed` | Drive `experiment.json` |
| `gents pack graph <dir> [--write-readme]` | Mermaid topology |
| `gents plugin build <dir>` / `install` / `list` / `run <ns/name> --input JSON [--bind-dir D] [--home H]` / `remove` / `publish` | One plugin on its own |
| `gents plugin dirs add <path> [--access read\|read_write]` / `list` / `remove` *(main)* | The operator's allowed-folders list for data-chosen plugin paths |
| `gents plugin bind <plugin> <profile>` / `unbind` *(main)* | Bind or unbind a plugin's `model_slot` after install |
| `gents graph run <pkg> [--entry E] [--input JSON] [--field k=v]... [--watch]` *(main)*; `graph watch/result/cancel/enable/disable` | Run an installed graph |
| `gents config validate --root DIR [--home H --bind-agent-did home]` | Validate a desired-state root (a pack dir works) |
| `gents config apply --root DIR --home H --graphql URL --bind-agent-did home [--force-rebind-concrete-did] [--prune]` | Apply `ROOT/schemas/` then the config, against a running server |
| `gents server --home H --http-port N --p2p-transport none --no-codex-shim [--apply-root DIR] [--apply-prune]` | Serve, and optionally apply a root after ready |
| `gents config backend set --file B.json` / `profile set --file P.json` / `profile list` | Create inference backends and profiles (canonical JSON) |
| `gents task ...`, `gents query`, `gents trace timeline --request-id`, `gents status` | Drive and inspect |

---

## 5. Plugins (Afterburner `.afb`)

### 5.1 What a plugin is and what it can do

A plugin is a complete Afterburner package (`.afb`). Its source compiles to a
WASI preview-1 command module (Rust, Go, C, C++, JS, TS and Ruby) or to an
emscripten-pyodide bundle (Python). The accepted `language` values are
`rust`, `go`/`golang`, `c`, `cpp`/`c++`/`cxx`/`cc`, `js`/`javascript`,
`ts`/`typescript`, `python`/`py` and `ruby`/`rb`
(`pack.rs::SUPPORTED_PLUGIN_LANGUAGES`).

At admission, gents asks Afterburner whether the real artifact can be run with
stdin, fuel, memory, wall clock and every granted manifold axis enforced
(`plugin.rs::unbounded_dispatch_reason`). If it cannot, the plugin is refused
by name. Afterburner's own "Known gaps" list Python (source or compiled) and
Ruby source as dropping at least one bound, so expect those to be refused.
**Write plugins in Rust.** Every shipped plugin is Rust (ocr, data_tables,
image_tools, charts, review_evidence, secscan). CI pins the toolchain with
`rust-toolchain.toml`.

The ABI (`plugin.rs` module doc): canonical JSON arguments arrive on stdin,
one JSON value goes to stdout, and stderr is diagnostics (64 KiB kept). Every
call is a fresh instantiation, with no state between calls. A non-zero exit
fails the call even if stdout parses. Stdout that is not exactly one JSON value
is `BadOutput`. `input_schema` is shown to the model but is **not** validated
by the runner.

When the plugin is called as a model tool, the output may be
`{"response": <json>, "parts": [{"type":"image","data":"<b64>","mimeType":"image/png"}]}`
to return images *(main)*. Only the Claude wire carries image parts. Other
providers get a note in place of each image.

### 5.2 Manifest entry (`PackPlugin`)

| Field | Meaning |
| --- | --- |
| `name` | Unique in the pack. It is the tool name and the stage name |
| `description` | Shown to the model when there is no `instructions` |
| `artifact` | `plugins/<name>.afb`. Must also be listed in `assets` |
| `source` | `plugins/<name>`: a Cargo project (the scaffold is `[workspace]`, a `[[bin]]` at `source/main.rs`) |
| `language` | Required even for a prebuilt artifact |
| `input_schema` | JSON Schema of the arguments. A `bind_dir.input_field` must be a property here |
| `instructions` | `plugins/<name>/TOOL.md`, at most 64 KiB, UTF-8. This is the model's tool description |
| `limits` | `{memory_mib ≤ 4096, wall_clock_secs ≤ 900, max_output_mib ≤ 4}`. Without it a call gets 64 MiB, 5 s and 1 MiB. Fuel is unbounded by default; the wall clock is the backstop. The 4 MiB stdout pipe is a hard ceiling |
| `manifold` | Standing authority asked of the sandbox (§5.4). Absent means sealed |
| `bind_dir` | Per-call directory binding (§5.5) |
| `model_slot` | The name of one of the pack's `inference_slots`. While it is bound, the plugin may ask the host for model calls (§5.6) |

### 5.3 Build and test

```sh
gents pack new fetcher --template plugin-tool --namespace acme   # or: gents pack add plugin fetcher
gents pack test ./fetcher        # check + compile to wasm32-wasip1 + run plugins/fetcher/tests/*.json
gents pack build ./fetcher       # -> ../acme.fetcher-0.1.0.pack
gents pack install ./fetcher --home H [--grant-authority]
gents plugin run acme/fetcher --home H --input '{"url":"..."}'
```

I ran `gents pack test` on a freshly scaffolded `plugin-tool` pack in the
scratchpad. It compiled in about 2 s and its one case passed.

A plugin test case (`gents-cli/src/commands/pack/test.rs::PluginCase`, with
`deny_unknown_fields`) looks like this:

```json
{ "input": { "...": "..." }, "expect": { "...": "..." }, "bind": "fixtures" }
```

- `expect` is compared for exact JSON equality. Leave it out to require
  success only.
- `bind` is a directory relative to the case file. It is bound into the
  plugin's `bind_dir`.
- To test the host round-trips, put the host's answers straight into
  `input`. `ocr/plugins/ocr/tests/remote-ocr-answered.json` passes
  `"model_calls": true, "model_results": {...}, "state": {...}`.
- HTTP cases run through the real host-served network.
- Rust plugins also run their own `cargo test` when `cargo` is on PATH.

### 5.4 Sandbox: filesystem, network, environment and time

The `manifold` JSON is Afterburner's `Manifold`:

```json
{ "fs": "None" | {"ReadOnly": ["/abs"]} | {"ReadWrite": ["/abs"]},
  "net": "None" | {"OutboundHttp": ["host", "*.domain", "1.2.3.4", "http://host:8080"]} | {"OutboundHttp": null},
  "env": "None" | {"AllowList": ["KEY"]} | "Full",
  "crypto": false, "child_process": false, "http_timeout_ms": 30000 }
```

- **Consent:** a plugin that asks for anything is refused at install unless
  `--grant-authority` is passed (`authority.rs::grant_for`). The grant is
  recorded. A later version that asks for more needs consent again. Effective
  authority is the declared grant intersected with the granted one, narrowed on
  every axis. `listen` is always off, and a plugin never holds a socket.
- **Filesystem:** use `bind_dir`, not standing `fs` grants. No shipped pack
  declares `fs`. All of them use `bind_dir`.
- **Network** *(main only)*: the guest never gets `net`. It returns
  `{"http_calls": {"requests": [{"id","method","url","headers","body"|"body_base64"}], "state": ...}}`.
  The host performs the admitted requests and calls the plugin again with the
  same input plus `"http_calls": true`, `"http_results": {id: {"status","headers","body"|"body_base64"} | {"error"}}`
  and the returned `state` (`plugin/http_calls.rs`, `plugin/rounds.rs`).
- **Network admission rules:**
  - Hostname entries reach only public addresses. Only a literal IP entry
    reaches an internal one (Tailscale `100.x`, LAN).
  - Plaintext needs an `http://` prefix. Without `:port` only the scheme's
    default port is allowed.
  - `OutboundHttp(null)` means any public host over HTTPS on port 443.
  - No redirects are followed. A 3xx goes back to the plugin.
  - The host adds no proxy, cookies or credentials.
  - `OutboundFull` (raw TCP) is refused.
- **Network limits:** 16 requests per round, 8 in flight, 256 requests per
  call, 64 rounds, a 1 MiB request body, **a 1 MiB response body**, **16 MiB
  of response bytes per call**, and 30 s per request (10 s connect), all inside
  the call's wall clock.
- **Environment:** `env` is an allow-list grant. Do not use it for secrets
  that a profile should hold.
- **Time:** the wall clock is a real preemption with 10 ms granularity: 5 s by
  default, 900 s maximum. WASI clocks are available. The ocr plugin uses
  `Instant` to stop at `max_seconds` and return a `cursor`. When request time
  runs out, the plugin gets one final round (1/8 of the clock) to finish with
  what it has.
- **DefraDB:** a plugin **cannot** query or write DefraDB itself.
  - As a callback or graph stage, it receives the projected source document on
    stdin. The host turns its JSON result into output documents, copies the
    correlation in, and commits everything in one transaction
    (`callback/plugin.rs`).
  - Output collections must belong to the pack, not to the runtime or a
    protected collection.
  - As a model tool, the result goes to the model.

### 5.5 `bind_dir`: per-call file access

```json
"bind_dir": { "input_field": "path", "original_field": "path_original",
              "access": "read" | "read_write", "write_fields": ["save"],
              "description": "shown to the operator" }
```

- The host replaces `input_field` with the canonical bound path. It is a WASI
  preopen. A single file is exposed alone through a hard-linked private folder.
- `original_field` receives the real path. Use it when you hand the path to a
  later stage.
- `write_fields` *(main)*: a call writes only when it sets one of these fields.
  Otherwise it binds read-only.
- Where the path comes from decides what is checked (`plugin/allowed.rs`):
  - An operator's flag or a test case's `bind` is trusted.
  - A path from data (a model's tool argument or a graph/callback source
    document) must resolve inside the session's working folder (read-only;
    never `/`, `$HOME` or the gents home) or inside a folder the operator
    allowed with `gents plugin dirs add <path> --access read|read_write`.
    Otherwise it needs interactive approval or is refused.
  - Graph and callback stages are headless, so **an allowed folder is
    required**. In a scenario, `experiment.json` `allowed_folders` writes it.
- The same binding applies to every host-call round of one call. A download
  plugin can append each ranged chunk to a file in a `read_write` bound folder.

### 5.6 `model_slot`: a plugin that calls inference

While the slot is bound for the installation (`--inference-slot remote_ocr=<profile>`
at install, or `gents plugin bind`), the input carries `"model_calls": true`.
The plugin may return:

```json
{"model_calls": {"requests": [{"id","prompt","images":[{"mime","data_base64"}],"max_tokens"}], "state": ...}}
```

The host calls the bound profile's backend and resumes the plugin with
`"model_results": {id: {"text"} | {"error"}}` (`plugin/model_calls.rs`).

- **Limits:** 64 requests per round, 512 per call, 1 MiB per answer, 32 MiB of
  answers per call and 120 s per request. The backend's `max_concurrent` is
  shared across every call in the process.
- **Dead endpoint:** after two consecutive all-failed rounds, every further
  request fails fast.
- **Privacy:** the plugin never sees the URL or key.
- **One service per round:** a round may ask for `model_calls` or
  `http_calls`, not both.
- **Example:** this is how `gents/ocr` sends poorly read scans to a remote
  vision model (Chandra or any OpenAI-compatible endpoint) through its
  optional `remote_ocr` slot.

### 5.7 Where a plugin can be called

| Call site | How it is configured | Digest pinning |
| --- | --- | --- |
| Model tool | `Tools.integrations.plugins: [{"plugin": "ns/name", "digest"?}]`. The tool name is `name` | Optional |
| Event callback | `callbacks[].handler = {"kind":"plugin","plugin":"ocr","digest":"","correlation_field":"run_id","max_attempts":N,"outputs":[PortSpec...]}` + `callback_bindings[]` (event source + `input_fields`) | Own plugin: `digest:""` is pinned at load. **Another pack's plugin needs a literal `sha256:<64 hex>`** (`callback/plugin.rs::validate_handler`) |
| Graph plugin node | `graph_capabilities[].target = {"kind":"plugin","plugin":"name"}` | The same rule: "compiling an unpinned one fails" |
| Graph entry prepare | `graph_intents[].entries[].prepare = {host:[git_diff, read_only_workspace], plugin, writes:[...]}` | Own plugin only |
| Scenario prepare | `experiment.json prepare: [{plugin, input, bind_dir, seed_fields: {field: "/json/pointer"}}]` | Own plugin |
| Operator | `gents plugin run ns/name --input JSON [--bind-dir D]` | |

For output documents (`PortSpec`: `name`, `collection`, `schema`
(`"Coll/v1"`), `correlation_field`, `cardinality` `one|many`, `required`):

- With a single output, the plugin returns the object (or an array, for
  `many`).
- With several outputs, it returns an object keyed by output name.
- The host writes the correlation field. The plugin must not.

---

## 6. Chaining stages with triggers and datastore writes

### 6.1 The rule

**A document creation fires an event source. A trigger turns that event into
a Task run, and a callback binding turns it into a plugin run. The stage
writes new documents, and those creations fire the next stage.**

- `event_kind` accepts only `created` (`EventSource::validate`). Updates never
  fire anything.
- The datastore tool surfaces offer only `create` and `query`. There is no
  update tool.
- So every stage hand-off is a newly created document, correlated by a shared
  field (`job_id` or `run_id`).

### 6.2 Worked example: `packs/gents/pipeline`

```
create ExperimentJob ──EventSource exp-stage1 (correlation job_id)──► Trigger exp-stage1 ► Task exp-stage1-task (behavior exp-stage1)
   stage-1 calls write_experiment_finding (DatastoreToolSurface "experiment-writes")
      └─ creates ExperimentFinding{job_id ← filled from correlation, finding_id, content, stage}
create ExperimentFinding ──EventSource exp-stage2──► Trigger exp-stage2 ► Task exp-stage2-task (no tools; reads {{ doc.content }})
```

The pieces, all in `pipeline/pack_config.json`:

- **Two event sources** on `ExperimentJob` and `ExperimentFinding`, both with
  `correlation_field: "job_id"`.
- **Two triggers** with `concurrency: "parallel"`.
- **The surface entry** for `write_experiment_finding`. Its `job_id` field has
  `"fill": "correlation"`, so the runtime fills it. The model cannot set it, and
  a refusal names the surface if it tries.
- **The stage-1 Task** sets `goal_objective_template`. Its Tools document turns
  on `enable_goal_tools`. The prompt tells the model to `update_goal
  status=complete` after the write, so the durable goal is the stage's
  terminal condition.
- **The stage-2 prompt** interpolates `{{ doc.* }}`, so it needs no read tool.

To run it (pipeline README):

```sh
gents init --home H --inference-url http://100.73.235.38:8000/v1 --backend-preset vllm \
  --openai-wire-api chat-completions --model-name GLM-5.3-Flash-NVFP4 --tool-package minimal --max-concurrent 32
gents pack install ./packs/gents/pipeline --home H --inference-slot worker=<profile_id>   # one usable profile auto-binds
gents server --home H --http-port 19191 --p2p-transport none --no-codex-shim
# wait for the log line "event source now observing source collection source_collection=ExperimentJob"
curl -s localhost:19191/api/v0/graphql -H 'content-type: application/json' \
  -d '{"query":"mutation { create_ExperimentJob(input:{job_id:\"j1\", prompt:\"...\"}) { _docID } }"}'
curl -s localhost:19191/api/v0/graphql -H 'content-type: application/json' \
  -d '{"query":"{ AgentRequest { caused_by_trigger_id lifecycle_state } }"}'
```

**Gotcha:** a document created before its event source is observing is seeded
as already seen and **never fires**. Wait for the log line above, or use
`gents pack scenario seed`, which waits for it.

### 6.3 Datastore tool surfaces

```jsonc
{ "surface_id": "s", "entries": [
  // create (the default kind)
  { "tool_name": "write_x", "collection": "X", "description": "...",
    "fields": [ { "name": "run_id", "fill": "correlation" },
                { "name": "assignment_id", "fill": { "source_field": "assignment_id" } },  // copied from the trigger's source doc
                { "name": "body", "required": true } ],
    "output_obligation": { "scope": "trigger", "minimum_writes": 1, "expected_count_field": "expected_total" } },
  // query
  { "kind": "query", "tool_name": "read_x", "collection": "X", "description": "...",
    "fields": ["a", "b"],                                   // projection allowlist
    "filter_fields": [ { "name": "run_id", "fill": "correlation" } ] }
] }
```

- `output_obligation` stops the request from completing until the minimum
  number of writes has happened. web_deep_research uses
  `minimum_writes` 2, 6 and 6 for sources, claims and evidence.
- **Read truncation:** query results cut every string field to **2,000
  bytes**, with a `[truncated: showed N of M bytes]` marker
  (`defra_query/mod.rs::MAX_FIELD_STRING_BYTES`).
  - The tool reports `field_recovery`. The model must page the field with
    `{"field_page": {"doc_id", "field", "offset_bytes", "expected_hash"}}` in
    2,000-byte steps.
  - Each query returns at most 1,000 rows.
  - For long text such as a page's Markdown, deliver it through `{{ doc.field }}`
    in the triggering Task's prompt (limited only by the 1 MiB rendered prompt).
    Do not have the model read it back through a query tool.
- **Write limits:** no string cap was found in `defra_write`. The
  `grok_tui_port` prompts split proof JSON into 2,000-byte strings, which
  suggests a practical ceiling somewhere. Treat large writes as unverified.

### 6.4 Plugin stages chained by callbacks: `gents/ocr`

`ocr` turns a PDF into DefraDB documents without a model:

```
create OcrJob{run_id,path,...}  ─EventSource ocr-plan─►     Callback ocr-plan    (plugin ocr) ─► OcrChunk × N
create OcrChunk                  ─EventSource ocr-extract─► Callback ocr-extract (plugin ocr) ─► OcrDocument + OcrPage × pages + OcrFigure × figs
```

- The plugin sees `run_id` and switches to graph mode (`source/graph.rs`).
- A chunk's extract call reads 1.5 MB of content at most.
- There are at most 1,000 chunks.
- The `path` must lie inside an allowed folder.
- `OcrPage{run_id, chunk, source, page, markdown}` is the natural input for a
  per-page model stage.

### 6.5 Fan-in (grouping)

An `EventSource.group` holds a trigger until a correlated set is complete:

- `expected_count`: a fixed N, or `{source_field}`, which reads the count from
  each member.
- `timeout_secs` and `min_count`.
- `correlation_field` is required.
- A group holds at most 256 documents (`MAX_EVENT_TRIGGER_GROUP_DOCS`).

The task then sees `group.docs`, `group.count` and `group.complete`. A book of
more than 256 pages cannot fan in on pages directly. Group per chunk, or write
a per-chunk summary document and group those.

### 6.6 Concurrency

Each trigger's `concurrency` is one of:

- `parallel` (the default)
- `serial`
- `queued_serial` (every fire persisted, one at a time in source order)
- `latest_only`

Backend throughput is capped separately by the backend document's
`max_concurrent` and `max_queue_depth`. `gents init` defaults to 2, so pass
`--max-concurrent 32` for a workstation running vLLM.

---

## 7. Graphs (worked example: `web_deep_research`)

A graph pack declares typed stage interfaces plus a topology. The compiler
turns them into derived `EventSource` and `Trigger` documents
(`graph-trigger-*`) and a `GraphRevision`. At runtime a graph is still the
same document-trigger machinery. What a graph adds is listed below.

1. **`graph_capabilities`:** one per stage. Each has a `target` (`task` or
   `plugin`) and typed `input_ports`/`output_ports` (`PortSpec`: collection,
   `Coll/v1` schema, correlation field, cardinality).
   `allowed_callers: ["${GENTS_PACK_AGENT_DID}"]`.
2. **`graph_intents`** (`GraphIntent`):
   - `nodes[]` (`node_id`, `capability_id`, `capability_revision`, optional
     `session.continue_node_id`)
   - `edges[]` (`from`/`to` `{node_id, port}`, `concurrency`, `delivery`,
     optional `predicate`). `delivery` is an `EventGroup`, for example
     `{"expected_count": {"source_field": "expected_total"}}`, the
     investigate-to-adjudicate fan-in.
   - `entries[]` (`name`, `collection`, `schema`, `to`, `input_contract`,
     `input_schema` with defaults, `prepare`)
   - `results[]` (`name`, `from`, `cardinality` `{kind: exactly|at_most, count}`,
     `terminal`)
   - `limits` (`max_nodes`, `max_edges`, `max_depth`, `max_fan_out`,
     `max_total_invocations`, `max_runtime_secs`). web_deep_research uses
     8/16/8/8/32/7200.
3. **`graphs: [{"graph_id"}]`** and `manifest.compiler_version: "graph-intent-v4"`.
4. **`graphs/<id>.plan.json`:** the compiled plan, written by `gents pack build`.
   Install verifies it against the digest.

web_deep_research runs plan (1) → investigate (×N, `parallel`) → adjudicate
(grouped on `expected_total`, `serial`) → report (`serial`).

- `research-plan` and `research-investigate` call exactly two named tools of
  the required MCP service `web-research-mcp` (`tools.remote.services`).
- The other two stages have no remote tools.
- Every surface tool is scoped to the run by `fill: "correlation"` on `run_id`.

To run it:

```sh
gents graph run web_deep_research --field question="..."   # main; old binary: --question
gents graph watch <run_id>; gents graph result <run_id>
```

**Triggers or a graph?** Use plain triggers (a `documents` pack) for an
open-ended, data-driven pipeline that runs continuously as documents arrive,
like OCR to structuring. A `documents` pack can also depend on graph packs.

Use a graph when you want any of these:

- A named entry with a validated input schema.
- Bounded invocations and runtime.
- Typed result contracts that `gents graph result` can show.
- Run status and cancel.
- Static topology checks.

A graph cannot depend on another pack, and removing it is refused while a run
is not terminal.

---

## 8. Inference slots and how they bind at install

1. The pack declares the slots (`manifest.inference_slots`). Behaviors
   reference `gents:inference-slot:<slot>`. A plugin's `model_slot` names a
   slot that has `behaviors: []`, usually `optional: true`.
2. The operator owns the backends and profiles. A pack can never author them.
   Make them with:
   - `gents init --inference-url ... --model-name ...` (one backend, one profile
     and one default behavior), or
   - `gents config backend set --file` and `gents config profile set --file`.

   The canonical `InferenceBackend` fields are `backend_id`, `name`,
   `provider_kind` (`"OpenAiCompatible"`), `openai_wire_api`
   (`chat-completions`), `endpoint`, `auth` (`{"kind":"unauthenticated"}`),
   `max_concurrent` and `max_queue_depth`. The `InferenceProfile` fields are
   `profile_id`, `backend_id`, `model_name`, `context_window`,
   `max_output_tokens`, `reasoning_effort`, `sampling_id` and
   `execution_id`.
3. `gents pack install DIR --home H --preview` lists the slots, the
   principal's profiles with whether each is usable, and the effective map,
   without writing anything.
4. The binding rules (`pack/inference.rs::preview_pack_inference_bindings`):
   - Bind explicitly with `--inference-slot slot=profile_id`, repeated for each
     slot, or with `--bindings F.json` (`{agent_did, inference_slots: {...}}`).
   - **Auto-bind happens only when no bindings were given, exactly one
     non-optional slot exists, and exactly one usable profile exists.**
     Otherwise every required slot must be bound, or the install fails before
     writing anything.
   - A plugin slot's profile is resolved to an endpoint at preview, so a bad
     binding fails early.
5. At bind time the marker is replaced by the real profile ID. Installed
   behaviors keep only that reference.
6. For two workstations, create two backends (workstation-1 and
   workstation-2, `max_concurrent` 32 each) and a profile on each. Bind
   different slots to different profiles to split load. One slot cannot
   load-balance across two profiles.
7. `pack scenario run` builds its own home from `experiment.json`'s `init`
   (`inference_url`, `model_name`, `backend_preset`, `openai_wire_api`,
   `max_concurrent`), so the scenario binds itself.

---

## 9. One pack using another (for example `gents/ocr`'s plugin)

- **`dependencies` cannot point at `gents/ocr`**, because ocr is a
  `documents` pack and only `graph` packs are installable as dependencies.
  Install ocr separately into the same home:
  `gents pack install gents/ocr --home H --inference-slot document_reader=<p>`.
  Its plugin then lands in the shared per-home store as `gents/ocr`.
- **As a model tool** in your own pack, add
  `"integrations": {"plugins": [{"plugin": "gents/ocr"}]}`. Leave out the
  digest to float to whatever is installed, or pin it.
- **As a callback or graph node**, you must pin
  `"digest": "sha256:<hex of the installed gents/ocr .afb>"`, read with
  `gents plugin list --home H`. CI notes that compiled plugins embed build
  paths, so the digest is per build machine. Use a registry-published ocr to
  get a stable digest.
- **Consuming ocr's documents (the simplest route).** Your event sources
  watch `OcrPage`, `OcrDocument` or `OcrChunk` creations on the same node,
  correlated by `run_id`.
  - `gents pack check` validates event-source *filters* against only the
    runtime schemas plus your pack's own. A `filter` on `OcrPage` fails the
    check unless your pack also carries that SDL.
  - Without a filter, the check does not query the collection.
  - Whether install requires the foreign collection to exist was not
    verified. Install ocr first.
- **Copying the ocr plugin into your pack** (vendoring) is the fallback when
  you need a self-contained pack. The store refuses a same-named plugin from
  a different pack *in the same namespace*. A different namespace is fine.

---

## 10. Running a pack end to end locally

### Path A: the `experiment.json` scenario (one command, fresh home)

`experiment.json` (`ScenarioManifest` in `commands/pack/scenario.rs`):

```json
{
  "name": "book_stage",
  "init": { "inference_url": "${GENTS_EXP_ENDPOINT:-http://100.73.235.38:8000/v1}",
            "model_name": "GLM-5.3-Flash-NVFP4", "backend_preset": "vllm",
            "openai_wire_api": "chat-completions", "max_concurrent": 32,
            "tool_package": "minimal", "tool_root": "${BOOKS_ROOT}" },
  "allowed_folders": [ { "path": "scans", "access": "read" } ],
  "prepare": [],
  "seed": { "collection": "BookJob", "job_id_field": "job_id", "prompt_field": "prompt",
            "fields": { "path": "${BOOKS_ROOT}/scans/book.pdf" } },
  "default_prompt": "...",
  "expect": { "trigger_ids": ["book-job"], "collection_counts": { "BookPage": 1 } },
  "await_timeout_secs": 600
}
```

- `allowed_folders` paths are relative to `init.tool_root` and must stay
  inside it, so it needs `init.tool_root`. They are created if missing.
- Seed `fields` are strings.
- `expect` can also check `trigger_request_counts`, `source_edges`, `fan_in`,
  `tool_calls`, `stage_tool_sequences`, `result_documents`,
  `signed_provenance` and `projections`.

```sh
gents pack scenario run ./book_stage --keep-home --http-port 19191   # init, apply, wait for observe, seed, await, report
gents pack test ./book_stage --scenario                              # same, from pack test
```

The artifacts land in the resolved pack's `runs/<job_id>/`.

### Path B: a long-lived home

```sh
gents init --home H --inference-url http://100.73.235.38:8000/v1 --backend-preset vllm \
  --openai-wire-api chat-completions --model-name GLM-5.3-Flash-NVFP4 --max-concurrent 32 --tool-package minimal
gents config profile list --home H                     # find the profile_id
gents plugin dirs add /path/to/scans --access read --home H    # main: let headless stages read the PDFs
gents pack install gents/ocr --home H --inference-slot document_reader=<pid>
gents pack install ./book_stage --home H --inference-slot worker=<pid> [--grant-authority]
gents server --home H --http-port 19191 --p2p-transport none --no-codex-shim
# wait for "event source now observing ...", then create the seed document via GraphQL
```

To apply over a running server instead of installing (not tracked as a pack
install), use either of these:

- `gents config apply --root ./book_stage --home H --graphql http://127.0.0.1:19191/api/v0/graphql --bind-agent-did home --force-rebind-concrete-did`
- `gents server ... --apply-root ./book_stage`

Use `--prune` only on a home dedicated to that pack.

To inspect a run:

- Query `AgentRequest { caused_by_trigger_id lifecycle_state }`.
- Read `InferenceCall { prompt_tokens completion_tokens }` for cost. Do not
  use `AgentResponse.token_count`.
- Run `gents trace timeline --request-id <id> --home H`.

### Path C: CI-style without a model

`tests/*.json` cases, run by `scripts/test-pack.sh` and `make test-<pack>`:

- `{"install": {documents, slots, plugins, dependencies}}` and
  `{"install": {graph, ...}}` check what an install creates. A reinstall must
  create nothing, and a remove must delete exactly that set.
- `{"graphs": [...]}` lists the graph ids the pack compiles to.
- `{"defs", "jq": [...]}` holds assertions over
  `{manifest, config, scenario, assets}`.
- `{"runtime": {repository, seed, taken, expect}}` installs and serves the
  pack, creates the seed and waits for the expected documents, with no model.
  This is how ocr's callback chain is tested end to end (`ocr/tests/graph_file.json`).
- `{"cli_flags"}` and `{"eval_case"}` are also available.

---

## 11. Explicit answers

**Can a plugin make outbound HTTP (for downloading)?**

On gents `main`, yes. The installed `f44a0cef` binary cannot.

- **How:** declare `"manifold": {"net": {"OutboundHttp": ["archive.org", "*.example.edu"]}}`,
  install with `--grant-authority`, and speak the `http_calls`/`http_results`
  round protocol. The guest never holds a socket; the host does the fetch.
- **Size limits:** 1 MiB per response body and 16 MiB per call (256 requests,
  64 rounds, 30 s per request, inside a wall clock of at most 900 s). A large
  PDF therefore needs `Range` requests in 1 MiB slices. Each slice is appended
  to a file in a `bind_dir` `read_write` folder, and a `cursor` resumes the
  download across calls past 16 MiB.
- **Redirects:** each one is a new request the plugin must issue.
- **Internal addresses:** hostname entries reach public addresses only.
  Reaching a Tailscale or LAN address needs a literal-IP entry.

**Can a behavior use a browser tool?**

There is no built-in browser tool. A grep for browser, playwright and
web_fetch in `crates/gents/src` and `gents-loop` finds nothing outside the
OAuth login flows. The supported routes are:

1. **An MCP service.** Run a browser or extraction MCP server (Playwright MCP,
   or Firecrawl as `web-research-mcp` does), register it as a
   `ToolServiceRegistry` (hostname or IP, `mcp_port`, `mcp_path`), and grant
   exact tool names with
   `Tools.remote.services: [{"mcp_service_id", "tool_names": [...], "required": true}]`.
   This is how `web_deep_research` fetches the web.
2. **Host bash** with network enabled (see the next answer).

**What tool lets an agent fetch a URL to a file?**

There is no dedicated one. The choices are:

1. **Host bash.** Use
   `host.bash: {"mode": "Unrestricted", "execution_mode": "workspace_write", "network_mode": "enabled"}`
   with `host.root` set to the target folder, and optionally
   `allowed_argv_prefixes: [["curl"], ["wget"]]` to restrict commands.
   `curl` appears in the read-only command list, but read-only validation
   rejects `-o`, `-O`, `--output` and the upload flags
   (`toolset/shared/command.rs`). Writing a file needs a write execution mode.
   This trades sandboxing for simplicity.
2. **A Rust download plugin** with `OutboundHttp` and a `bind_dir`
   (`read_write`, `write_fields: ["save"]`) on gents `main`. Offer it as a
   model tool, or run it as a callback triggered by a `DownloadJob` document.
   This is the most contained option and the one that fits "everything inside
   gents".
3. **An MCP tool** that downloads.

---

## 12. Implications for a PDF to structured-book pack

- **OCR is solved:** create an `OcrJob` (path in an allowed folder) and
  consume `OcrPage` creations. The ocr chain needs no model.
- **Hand the page text through the prompt.** Use `{{ doc.markdown }}` in the
  per-page Task prompt, not a query tool, because of the 2,000-byte read
  truncation.
- **Every stage writes new documents** through surface `create` tools (no
  updates, only `created` events), correlated by `run_id` or `book_id`.
- **Book-level steps** (ToC, chapter assembly) need fan-in. The 256-member
  group cap forces a two-level fan-in for long books: group pages per chunk,
  then chunks per book.
- **Downloading is a separate `plugins` or `documents` pack** with an HTTP
  plugin (gents `main` only).
- **Throughput:** put a backend per workstation with `max_concurrent` 32, and
  set triggers to `parallel`.

## 13. Open questions (not verified from source)

- Is there a practical size limit on a string field written by a surface
  `create` tool? Nothing was found in `defra_write`, but `grok_tui_port`
  prompts split proof JSON into 2,000-byte strings.
- Does install reject an event source whose `source_collection` belongs to
  another pack and is not yet on the node? Install ocr first to be safe.
- Can a pack legally ship `tool_service_registries` (MCP endpoints)? The
  struct accepts it, but every official pack leaves it to the operator.
- Does `GLM-5.3-Flash-NVFP4` accept images? This decides whether it can serve
  ocr's `remote_ocr` slot or figure descriptions.
- When will a gents release containing `http_calls`, `plugin dirs` and
  optional slots ship as the desktop app? Until then, use a gents built from
  `main`.
