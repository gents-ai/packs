# Download pack: prior art and design

Status: research, phase 1 (read-only survey). Nothing in shelf, gents or packs
was changed.

Goal: a generic `gents/download` pack (working name) that turns a DOI, an
identifier, a bibliographic description or a URL into a verified file on disk
plus a DefraDB provenance record. The `structured_book` pack reads that record
and hands the file to `gents/ocr`. The pack is plain plumbing that works with
any source, plus resolvers for legitimate sources only: open-access locations,
public-domain scans, author-posted copies, institutional repositories, and URLs
the user supplies. Shadow libraries are out of scope and get no resolver, no
config hook and no documentation.

## TL;DR

1. **Put fetching in a plugin, not in a behavior using the web-research MCP.**
   Gents `main` now gives plugins network access through the host (PR #2300,
   `crates/gents/src/plugin/http_calls.rs`): the plugin returns `http_calls`
   batches and the host runs the requests its `OutboundHttp` allow-list admits.
   Together with a `read_write` `bind_dir` (PR #2301), one plugin can download
   bytes, check them, hash them and write them to disk. The web-research MCP
   cannot return the original bytes of a binary file at all (details below).
2. **Version catch:** host HTTP is in the gents CHANGELOG under *Unreleased*.
   The installed `gents` v0.20.0 (2026-10-05) does not have it. The pack needs
   gents built from `main`, or the next release.
3. **Size catch:** the host caps one response body at 1 MiB and one call at
   16 MiB, 256 requests and 30 s per request. It does not follow redirects and
   sends no cookies. Book PDFs (20 to 300 MB scans) must therefore be fetched
   with HTTP `Range` requests in 1 MiB slices, spread over several calls that
   resume from a cursor. We should also ask upstream for a "stream to a bound
   file" mode that skips the 1 MiB cap.
4. **The LLM is only needed for discovery.** A `source-finder` behavior uses
   the web-research MCP (`web_search`, `web_scrape_url`) only when the input is
   a bare title and author. All of these are deterministic plugin code: DOI and
   identifier resolution, checking and fetching, hashing, dedup and writing
   records.
5. **Record shape:** a `DownloadedSource` per fetched file, with a
   content-addressed path `library/sha256/<aa>/<hex>.<ext>`. The SHA-256 is the
   identity of the bytes and the dedup key (schema below).

## 1. What exists today

### 1.1 web-research-mcp (SearXNG + Firecrawl + Camoufox)

The Amygdala crate `crates/web-research-mcp` is now an empty directory. Commit
`0b9298ad` ("refactor: consume public web research gateway") deleted it and
moved the code into the public repo
`$SRC/github.com/source-inc/web-research-mcp`
(Apache-2.0, v0.1.10 pinned). Amygdala only keeps deployment inventory, under
`infra/services/web-research-mcp/`. The live gateway runs on studio-2, port
9213, at `/mcp`.

Tools it exposes (README, `src/mcp/*`):

| Family | Tools | Default |
| --- | --- | --- |
| Bounded research | `web_collect_evidence` (1-6 queries, at most 12 scrapes, at most 8 evidence records, idempotent by `assignment_id`) | on |
| Search | `web_search` (SearXNG; `categories`, `language`, `time_range`, `include_domains`/`exclude_domains`, at most 20 results) | on |
| Extract | `web_scrape_url` (`mode` static or rendered), `web_map_site` | on |
| Evidence | `web_get_fetch`, `web_find_in_fetch`, `web_verify_quote` | on |
| Crawl | `web_crawl_site` (depth 2 or less, 50 pages or less) | off |
| Browser | `browser_open`, snapshot, screenshot, click, type, scroll, close, `browser_import_cookies` | off |

How it handles the things a download pack cares about:

- **Downloads and PDFs:** no download tool. `web_scrape_url` calls Firecrawl
  `/v1/scrape` with `formats: ["markdown","links"]` and `onlyMainContent`.
  Firecrawl can turn a PDF into Markdown, but the original bytes never come
  back. Camoufox returns only screenshots, as base64 PNG.
- **Size:** `scrape_max_bytes = 2_000_000`, counted on the Markdown. Larger
  pages are refused.
- **Hash:** `content_hash` is the SHA-256 of the *wrapped Markdown envelope*,
  not of the original file. It works for checking quotes, but cannot identify
  a file.
- **Cookies:** `browser_import_cookies` always returns `policy_denied` in v1.
  Browser sessions have a TTL of 300 s, an idle timeout of 60 s, and at most
  4 run at once.
- **Rate limits and policy:** a global in-flight cap (`inflight_max = 8`),
  capped search results and crawl depth/pages, and domain allow/deny lists.
  The default denylist blocks `*.internal`, `*.local`, localhost, metadata
  hosts and private CIDRs. It rejects non-HTTP schemes and credentials in
  URLs. There is no per-host politeness delay.
- **Trust:** content is wrapped in nonce-delimited `untrusted-web-content`
  markers with spoofed tags neutralized. A deployment-level
  `WEB_RESEARCH_MCP_EXPOSED_TOOLS` allowlist narrows what a deployment exposes.
- **Store:** a disk `FetchRecord` (`fetch_id`, `requested_url`, `final_url`,
  `mode`, `requested_at`, `completed_at`, `status`, `bytes`, `content_hash`,
  `source_provider`, `policy_decision`, `truncated`) plus an `AuditRecord` for
  every call and every denial.

Verdict: the MCP is right for **finding** a source (search, reading a landing
page to locate the PDF link, author home pages). It is wrong for
**acquiring** one.

### 1.2 `gents/web_deep_research` pack (v1.2.0)

- `kind: graph` with four behaviors: plan, investigate, adjudicate, report.
  Each behavior is bound to an inference slot (`coordinator`, `researcher`,
  `verifier`).
- The MCP is declared once in `manifest.external_dependencies` (`service_id:
  web-research-mcp` with an `install_command`). Each Tools document references
  it with `remote.services: [{mcp_service_id, required: true, tool_names:
  [...]}]`, and the investigator gets only `web_collect_evidence` and
  `web_find_in_fetch`. **We copy this pattern for `source-finder`.**
- Typed handoffs: `graph_capabilities` with input/output ports
  (`collection`, `schema: X/v1`, `correlation_field: run_id`, cardinality),
  `graph_intents` with edges, `concurrency: parallel|serial`, and joins
  written as `delivery.expected_count.source_field`. Writes go through
  `datastore_tool_surfaces` with `output_obligation`.
- Schema conventions to copy: `run_id: String @index @immutable`, a stable
  unique id with `@index(unique: true) @immutable`, and almost every other
  field as `String`. `WebResearchSource` already models web provenance
  (`url`, `fetched_at`, `fetch_id`, `content_hash`, `extraction_method`,
  `content_integrity_verified`).

### 1.3 `gents/ocr` pack: the downstream consumer, and the trigger pattern

- `kind: documents`. Its `pack_config.json` uses the newer
  `event_sources` → `callback_bindings` → `callbacks` shape, where
  `handler.kind: "plugin"` runs a plugin as a plain graph node with no model:
  `OcrJob` created → `ocr-plan` → `OcrChunk` (many) → `ocr-extract` →
  `OcrDocument`/`OcrPage`/`OcrFigure`. **This is the shape for the
  "afterburner scripts in the trigger pipeline".** Our resolve and fetch nodes
  are built the same way.
- The handoff contract is simple: create `OcrJob {run_id, path}` where `path`
  is a file or folder inside an allowed folder (`gents plugin dirs add`).
  A graph node asks nobody, so the download library folder must be on the
  allowed list.
- `remote.rs` is prior art for the round protocol (`model_calls` requests plus
  echoed `state`, at most two rounds, budget carried in the cursor).
  `http_calls` reuses the same loop (`plugin/rounds.rs`), so the OCR plugin's
  cursor/state code is the template.
- Inline input already exists: `data_base64` takes up to 64 MiB. A path on
  disk is still the better handoff, because a book is too big for a DB field.

### 1.4 Gents plugin runtime (main, Unreleased)

From `crates/gents/src/plugin/http_calls.rs` and the CHANGELOG:

- The guest never holds a socket, and the WASI `net` axis is stripped. A
  manifold declaring `net: {"OutboundHttp": [...]}` gets host-performed
  HTTP. `OutboundHttp(null)` means any public host, HTTPS only, port 443.
  `OutboundFull` is refused at load time.
- Allow-list entries: `host`, `*.domain`, or an IP literal, optionally prefixed
  with `http://` and suffixed with `:port`. Hostname entries reach only public
  addresses, checked on the resolved addresses of each connection. The
  resolver is pinned, so DNS rebinding is closed.
- No redirects are followed. A 3xx comes back with `location`, and the plugin
  re-issues the request, which is admitted again. That is useful because each
  hop can be recorded in provenance.
- The request carries only what the plugin sets. There is no host proxy,
  cookie jar or credentials, and the plugin may set `Range`, `User-Agent` and
  `Accept`.
- Caps: 16 requests per round, 8 in flight, 256 per call; 1 MiB request body;
  **1 MiB response body**; 16 MiB response bytes per call; 30 s per request
  (can be lowered with `http_timeout_ms`).
- Wire format: input gets `"http_calls": true`, then
  `http_results: {id: {status, headers, body|body_base64}|{error}}` and the
  echoed `state`. Output is
  `{"http_calls": {"requests": [{id, method, url, headers, body|body_base64}], "state": ...}}`.
- Filesystem: `bind_dir.access: "read_write"` combined with `write_fields`
  (see the `data_tables` field `output` and the `charts` field `save`) means a
  call writes only when it sets a write field.
- Granting authority beyond the sealed default needs `--grant-authority`
  (on install, and in `gents pack scenario` runs). No first-party pack
  declares `OutboundHttp` yet, so this pack would be the first. The packs
  README still says the ceiling is "sealed today", which is out of date for
  `main`.

### 1.5 am-hist-bench acquire stage (architecture only)

`pipeline/behaviors/acquire-fetcher/` is built around a shadow-library source
(`annas.py`, plus parts of `browser.py` and `runner.py`). **None of that source
logic is reused or described here.** Only its generic plumbing is worth
keeping:

| Concern | What it does | Carry over as |
| --- | --- | --- |
| Queue / state machine | wishlist rows `want → queued → acquiring → have / skip / unavailable / failed`; on interrupt `acquiring → queued` ("safe to rerun"); `--retry-failed` re-queues terminal rows | the `status` field on `DownloadJob` / `DownloadedSource` |
| Concurrency | a topic-level `fcntl` lock; at most one download per IP; a cooldown between works (`ACQUIRE_COOLDOWN_SECONDS=45`, kept in `rate_limit.json`) | per-host politeness: `min_interval_ms` per resolver host in the plugin config, and the graph node's concurrency |
| Crash recovery | `_claim_existing_book` checks the destination folder before any network call, so a valid file that landed before the catalog write is reused | the fetch plugin checks the content-addressed path and the `.part` before requesting anything |
| Hashing | `sha256_file`, streamed in 1 MiB blocks, run after the download and before any catalog write | the same; plus checking the upstream checksum when the resolver supplies one |
| Dedup | (a) the same work with the same SHA means a no-op; (b) the same title family in a topic means `duplicate_of`; (c) identical bytes must keep the old downstream ids | (a) and (c) work as-is; (b) becomes `DownloadedSource.duplicate_of` |
| Format check | `_detect_book_format`: at least 1000 bytes; `%PDF` magic; ZIP with `mimetype == application/epub+zip`, `META-INF/container.xml` and `testzip()` clean; anything else is an error | the same, done by sniffing bytes; the server's Content-Type is only a hint |
| Canonical naming | renamed to `<work_id>.<fmt>`; a mismatched hash at the canonical path is an error, never an overwrite | named by content address, so a mismatch cannot happen |
| Atomic writes | tempfile, then `fsync`, then `os.replace` for every JSONL | write `.part`, hash it, then rename into `sha256/`; DefraDB records written last |
| Catalog identity | `catalog.jsonl` row keyed by `work_id`; a new binary replaces the row; identical bytes keep `shelf_book_id`, `parse_status` and the rest | `structured_book` keys its book identity on `sha256`, so fetching the same bytes again from another resolver never re-parses |
| Handoff | catalog `normalize_status: pending`, which Shelf picks up | `DownloadedSource` created with `status: ok` triggers the `structured_book` node, which writes `OcrJob {path}` |
| Honest failure | `unavailable` (no confident match) is kept apart from `failed` (transport error) | the same two terminal states, plus `refused` (license or policy) |

## 2. Where fetching runs: plugin or MCP behavior

| | Plugin with host `http_calls` | Behavior plus web-research MCP |
| --- | --- | --- |
| Original bytes | yes (`body_base64`, Range slices) | no; Markdown only |
| True SHA-256 of the file | yes | no (it hashes the envelope) |
| Size | 1 MiB per response and 16 MiB per call, so ranged and resumed | 2 MB of Markdown |
| Writes the file | yes (`read_write` bind_dir) | no |
| Deterministic, testable, no model cost | yes; runs as a callback node | no; LLM in the loop |
| Redirect provenance | each hop is visible | `final_url` only |
| Finding a landing page from a title | weak | **strong** (SearXNG with scholarly engines) |
| Runs on released gents 0.20.0 | **no** | yes |

**Decision:** split along that line.

- `download` plugin (Rust, like `ocr`). It has three modes. `resolve` uses a
  host allow-list of resolver APIs. `fetch` uses `OutboundHttp(null)`, because
  OA locations point at arbitrary repository hosts and users supply arbitrary
  URLs; that still means HTTPS only, port 443, public addresses only. `verify`
  is local only. If least privilege matters more than having one artifact,
  ship `download_resolve` and `download_fetch` as two plugins with different
  manifolds.
- `source-finder` behavior (LLM, `GLM-5.3-Flash` slot), run only when the job
  has no identifier and no URL. Its tools are `web_search` and
  `web_scrape_url` from `web-research-mcp`, plus a datastore write of
  `DownloadCandidate` rows. Its output goes through the same plugin checks as
  everything else, so the model never writes a file and never decides a
  license.
- If gents `main` cannot be used yet: there is no good stopgap inside the
  sealed runtime. Options are to wait, or to run an out-of-process fetcher
  that writes into the allowed folder and creates the record. The second is a
  step back from the "no external app" goal.

## 3. Minimal tool set

| Tool / mode | Input | Does | Output |
| --- | --- | --- | --- |
| `search` (behavior via MCP) | title, author, year, hints | SearXNG with `categories` set to science/general; reads landing pages; picks candidates | `DownloadCandidate` rows (`resolver: web_search`) |
| `resolve` (plugin) | `doi` / `arxiv` / `hal` / `zenodo` / `ia` / `htid` / `gutenberg` / `url` | calls the resolver APIs below; ranks OA locations (publishedVersion > acceptedVersion > submitted; PDF > EPUB > HTML; repository > unknown host); records license per location | ordered candidates in a `DownloadPlan` |
| `fetch` (plugin) | plan plus a cursor | HEAD or `Range: bytes=0-0` to learn `Content-Length` and `Accept-Ranges`; follows redirects by hand (at most 8 hops); gets 1 MiB ranges into `.part`; resumes from the cursor; `max_bytes` cap (default 1 GiB) and `min_bytes` of 1000 | `.part` progress or a finished file |
| `check` (in fetch) | `.part` | sniffs magic bytes (`%PDF-`, EPUB ZIP rules, TIFF/JPEG/PNG, TEI/XML, HTML); an HTML reply where a PDF was expected means a landing page, so it extracts `citation_pdf_url` / `<link rel=alternate type=application/pdf>` and tries again once; checks the Content-Length match | `mime`, or a refusal |
| `hash` (in fetch) | file | SHA-256, streamed; compared with the upstream md5/sha1/size where the resolver gives one (IA `files.xml`, Zenodo `checksum`) | `sha256`, `upstream_checksum_ok` |
| `dedup` (in fetch) | sha256 | if `library/sha256/<aa>/<hex>.<ext>` exists, the `.part` is dropped and the record is marked as already present | `deduplicated: true` |
| `record` (callback output) | everything above | writes one `DownloadedSource` row with provenance and license | the DB row, which triggers `structured_book` |

Being polite to sources (built into the plugin, not left to the model):

- Every request sends `User-Agent: gents-download/<ver> (+<project URL>)`.
  **No email address goes in any header or query string.** That rules out the
  Unpaywall API, which requires an `email=` parameter. Use OpenAlex
  `best_oa_location` / `oa_locations` instead; that data comes from Unpaywall
  anyway and needs no email. Crossref and OpenAlex `mailto` "polite pool"
  parameters are also left out. A pack config field for a role address
  (`polite_contact`) is possible, but it defaults to empty and the doc tells
  operators not to use a personal address.
- Per-host minimum spacing, set in config. Defaults: arXiv 3 s (its API
  guidance); Gutenberg, use a mirror or `gutenberg.org/cache/epub/` files,
  never crawl the site; IA 1 s; other hosts 1 s. Honor `Retry-After` on 429
  and 503. Use exponential backoff, at most 3 attempts per URL, then mark the
  attempt `failed` with the HTTP status.
- `robots.txt` is checked for the `search` path (HTML landing pages), not for
  documented APIs.

## 4. Legitimate resolvers

| Resolver | Input | Lookup (host on the allow-list) | Download location | License / rights field |
| --- | --- | --- | --- | --- |
| `openalex` | DOI | `api.openalex.org/works/doi:<doi>` | `best_oa_location.pdf_url`, `oa_locations[]` | `license`, `version`, `is_oa`, `oa_status` |
| `crossref` | DOI | `api.crossref.org/works/<doi>` | `link[]` with `content-type: application/pdf` (often TDM or paywalled; keep only with an OA license) | `license[].URL`, `content-version` |
| `arxiv` | arXiv id | `export.arxiv.org/api/query?id_list=` | `arxiv.org/pdf/<id>` | per-paper license (arXiv non-exclusive or a CC license) |
| `hal` | HAL id / DOI | `api.archives-ouvertes.fr/search/?q=...&fl=fileMain_s,licence_s,...` | `fileMain_s` | `licence_s` |
| `zenodo` | record id / DOI `10.5281/zenodo.*` | `zenodo.org/api/records/<id>` | `files[].links.self` | `metadata.license.id`, `files[].checksum` (md5) |
| `oapen` / `doab` | title, ISBN, handle | `library.oapen.org/rest/search`, `directory.doabooks.org/rest/search` | bitstream URL | `dc.rights` / CC license |
| `internet_archive` | identifier | `archive.org/metadata/<id>` | `archive.org/download/<id>/<file>` (prefer the original PDF/EPUB; `_djvu.txt`/`_hocr` as extras) | `licenseurl`, `rights`, `possible-copyright-status`; **refuse if `access-restricted-item` is true or the item is in a lending collection.** Lending items are not public-domain full view. Use `files[].sha1`/`md5`/`size` to verify. |
| `hathitrust` | htid / OCLC / ISBN | `catalog.hathitrust.org/api/volumes/full/<type>/<id>.json` | full-view items only (`rightsCode` `pd`; `pdus` is public domain only in the US, so record that) | `rightsCode`, `usRightsString`; whole-volume download without a member login is restricted (see open questions) |
| `gutenberg` | ebook id | catalog feed (`gutenberg.org/cache/epub/feeds/`), or a mirror | `.../cache/epub/<id>/pg<id>-images.epub` or `.txt` | public domain in the US plus the Gutenberg license text; record `rights_jurisdiction: US` |
| `perseus` | CTS URN | `scaife.perseus.org` / the `PerseusDL/canonical-*` TEI on GitHub (`raw.githubusercontent.com`) | TEI XML | CC BY-SA (repo `LICENSE`) |
| `lacuscurtius` | URL under `penelope.uchicago.edu/Thayer/` | none (direct) | HTML pages; the OCR pack reads HTML | public-domain source texts per the site; record `license_basis: site_statement` and keep the page URL |
| `url` | any https URL from the user | none | the URL itself | `license: unknown`, `license_basis: user_supplied` (the user is asserting the right) |
| `web_search` | from `source-finder` | SearXNG via MCP | the candidate URL | whatever the page states, else `unknown` with `license_basis: author_posted` or `institutional_repository`, flagged `needs_review` |

When to refuse: a candidate from `openalex`, `crossref` or `web_search` with
no OA signal (`is_oa: false`, a non-OA `oa_status`, or a Crossref TDM-only
link) is recorded as `refused` and never fetched. User-supplied URLs are
fetched as given.

## 5. Pipeline shape (pack_config, OCR-style callbacks)

```
DownloadJob (created)
  └─ download-resolve   [plugin, resolver allow-list]          → DownloadPlan (one, ordered candidates JSON)
       (no identifier → source-finder behavior writes DownloadCandidate rows → download-resolve re-ranks them)
  └─ download-fetch     [plugin, OutboundHttp(null), read_write] → DownloadedSource (one)
                                                                 → DownloadProgress (one, while incomplete)
DownloadedSource (created, status=ok)
  └─ structured_book: create OcrJob {run_id, path}
```

Fetch tries the plan's candidates in order until one passes `check`. Each try
is recorded in `attempts` on the final row, so failed hosts are kept as
provenance. To continue a large file, each `download-fetch` call moves 16 MiB
at most. While the file is incomplete, the call writes a `DownloadProgress
{run_id, cursor, bytes_done, bytes_total}` row, and an event source on
`DownloadProgress` created fires the same callback again. A 300 MB scan takes
about 19 calls. Whether a callback may write to the collection that triggers
it is an open question. The fallback is the OCR plan/chunk pattern:
`download-plan` emits N `DownloadRange` rows, which get fetched in parallel
into `part-<n>` files, with an `expected_count` join that assembles and hashes
them.

## 6. DefraDB record shapes

Field types follow the existing packs: mostly `String`, with `Int`/`Boolean`
where the OCR schema already uses them. Bytes stay on disk and never go in a
DB field.

```graphql
type DownloadJob {
  run_id: String @index(unique: true) @immutable
  # exactly one of these identifies the work; the rest are hints
  url: String
  doi: String @index
  identifier: String          # "arxiv:2401.01234", "ia:romanindiatrade00ward", "htid:mdp.39015...", "gutenberg:1234", "hal:hal-0123", "zenodo:123", "cts:urn:cts:greekLit:tlg0016.tlg001"
  title: String
  authors: String
  year: String
  formats: [String]           # preferred order, default ["pdf","epub"]
  allow_unknown_license: Boolean  # default false except resolver=url
  library_path: String        # allowed read_write folder; default "library"
}

type DownloadedSource {
  run_id: String @index @immutable
  source_id: String @index(unique: true) @immutable  # "<run_id>:sha256:<hex>"
  status: String @index       # ok | duplicate | unavailable | failed | refused
  # what was asked for and how it was resolved
  url: String                 # final URL the bytes came from (after manual redirects)
  requested_url: String
  landing_url: String         # HTML page the PDF link was taken from, if any
  redirects: [String]
  resolver: String @index     # openalex | crossref | arxiv | hal | zenodo | oapen | doab | internet_archive | hathitrust | gutenberg | perseus | lacuscurtius | url | web_search
  resolver_ref: String        # the resolver's own id for the item
  doi: String @index
  title: String
  authors: String
  year: String
  version: String             # publishedVersion | acceptedVersion | submittedVersion | scan | edition
  # rights
  license: String             # SPDX-ish id or URL: "CC-BY-4.0", "public-domain", "unknown"
  license_url: String
  license_basis: String       # resolver_metadata | site_statement | user_supplied | author_posted | institutional_repository
  rights_jurisdiction: String # "US" for pdus/Gutenberg, empty when worldwide
  needs_review: Boolean
  # bytes
  sha256: String @index       # lowercase hex; the identity of the bytes
  bytes: Int
  mime: String                # sniffed: application/pdf | application/epub+zip | text/html | application/tei+xml | image/tiff ...
  declared_mime: String       # server Content-Type, for the record
  upstream_checksum: String   # "md5:<hex>" / "sha1:<hex>" from the resolver
  upstream_checksum_ok: Boolean
  etag: String
  last_modified: String
  retrieved_at: String        # RFC 3339 UTC
  path: String                # relative to the library: "sha256/ab/ab12....pdf"
  deduplicated: Boolean       # bytes were already in the library
  duplicate_of: String        # source_id of the earlier row with the same sha256
  attempts: String            # JSON list of {url, resolver, status, http_status, error}
  error: String
}
```

How `structured_book` consumes this:

- It reads `status == "ok"`, then `path`, `mime` and `sha256`.
- Its book identity is `sha256`. When the same bytes arrive again through
  another resolver or run, the existing book row and parse are reused (the
  am-hist-bench `append_catalog` rule).
- It sends `path` to `OcrJob`.
- It never reads the network.

## 7. Reuse checklist

- From `source-inc/web-research-mcp`: the policy module's URL rules (scheme,
  credentials, private CIDR, internal-suffix denylist) as a pre-check before a
  plugin's `http_calls`, even though the host enforces them too; the shape of
  the `FetchRecord`/`AuditRecord` fields; the `untrusted-web-content` envelope
  for any HTML text shown to the `source-finder` model; and
  `WEB_RESEARCH_MCP_EXPOSED_TOOLS=web_search,web_scrape_url` for the
  deployment that `source-finder` uses.
- From `gents/web_deep_research`: the `external_dependencies` declaration,
  `remote.services` tool scoping, inference-slot binding, the schema
  conventions, and the `delivery.expected_count` join.
- From `gents/ocr`: the Rust plugin skeleton (`input.rs`, cursor/state round
  loop in `remote.rs`), `TOOL.md` for the model-tool face, the
  `event_sources`/`callback_bindings`/`callbacks` pack_config, and the
  test-file layout (`tests/install.json`, `graph_file.json`).
- From `gents/data_tables` and `gents/charts`: `bind_dir` with
  `access: read_write` plus `write_fields`.
- From am-hist-bench (plumbing only): the state machine, crash-recovery claim,
  streamed SHA-256, byte-sniffing format check, canonical/atomic placement,
  and keeping identity when the bytes match.
- From gents runtime: `plugin/http_calls.rs` and `plugin/rounds.rs`, with
  `http_calls_tests.rs` as the spec for the wire format.

## 8. Open questions

1. When will host `http_calls` (#2300) ship in a tagged gents release? Until
   then, do we build gents from `main` for this pipeline?
2. Upstream ask: a `sink` mode on `http_calls` that streams a response into a
   bound `read_write` file (with a hash computed on the way) and skips the
   1 MiB/16 MiB caps. Without it, servers that ignore `Range` cannot deliver
   files over 1 MiB at all.
3. Can a callback's output collection be the collection that triggers it
   (`DownloadProgress` self-loop), or do we need the plan → range → join
   design?
4. HathiTrust: for non-members, full-view public-domain volumes download page
   by page, not as one PDF. Should the resolver assemble pages (slow, rate
   limited), or prefer an IA copy of the same scan when one exists?
5. Should `allow_unknown_license` ever be true for `web_search` candidates
   without a human approving them? The proposal is no: write `needs_review`
   and stop.
6. Should the `polite_contact` config field exist at all, given the rule
   against email in requests? The proposal is to leave it out of v1.
7. Where does the library live: one shared folder per gents home (dedup
   across projects) or one per project? Either way it must be registered with
   `gents plugin dirs add <lib> --access read_write`.
