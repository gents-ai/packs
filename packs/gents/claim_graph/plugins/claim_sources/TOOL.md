# `claim_sources`

Fetches one web page or text file and stores it as a `Source` document, so
claims can quote it and the gate can check the quote. It runs as the callback
of a `SourceRequest`; you do not call it directly. Create the request, then
find the `Source` whose `request_id` is your request's id.

## Request

`SourceRequest` fields it reads: `request_id`, `graph_id` (required), `url`
(required), `confidential`, `note`. Other fields are ignored.

The URL must be `http://` or `https://`, at most 2048 characters, without
`user:password@`, spaces or backslashes, and must not name `localhost` or an
internal IP literal (loopback, private, link-local, shared, reserved,
multicast; IPv4 written as four decimal parts). The host refuses names that
resolve to internal addresses and, with an unscoped grant, plain `http://`
(see Authority).

## Source

One row per request, always, with these fields: `source_id`, `graph_id`,
`url`, `final_url`, `title`, `fetched_at`, `status` (`ok` or `failed`),
`error`, `http_status`, `content_type`, `content_hash`, `text`,
`text_chars`, `truncated`, `confidential`, `note`; the runtime adds
`request_id`.

- `ok`: the server answered 2xx with `text/html` or `application/xhtml+xml`
  (page text extracted, `title` from `<title>`), or `text/plain` or
  `application/json` (body kept verbatim). `text` holds at most 200000
  characters; `truncated` says it was cut; `text_chars` counts what is kept.
- `failed`: `error` says why in one line and `text` is null: a refused URL,
  no network grant, a host error (unreachable, timeout, body over 1 MiB),
  an HTTP status other than 2xx, more than 5 redirects, no or another
  content type (`unsupported content type <type>`, PDF included), or a URL
  this graph already holds (`this url is already stored as source <id> ...;
  cite that source_id`).

`source_id` is `s:` and the first 16 hex digits of sha256 of the URL as
requested, for the first successful fetch of that URL in the graph. Every
failed row, and a fetch whose canonical id is already taken, has
`s:<16 hex>-<8 hex>` (the suffix is sha256 of `request_id`), so a failure
never blocks a later fetch and no two rows collide. Cite a source by the
`source_id` of an `ok` row only.

`content_hash` is `sha256:<hex>` of the raw response body as received.
`final_url` is the URL that answered after redirects.

HTML text: the content of `script`, `style`, `nav`, `footer`, `noscript`,
`template`, `svg` and `head` is dropped; block elements (paragraphs,
headings, list items, table cells, `br`, ...) end a line; inline tags vanish;
`&nbsp;` is a space and the common named and all numeric entities are
decoded; whitespace runs are one space; empty lines are dropped. Quote a
claim from within one line of `text`, exactly.

## What it relies on from the host

- `http_calls: true` in the input when the network grant is in force; no
  such field means no network and a failed row.
- One GET per round, answered in `http_results.fetch` as `{"status", "headers",
  "body"}` (header names lowercase, repeated headers joined with `, `;
  `body_base64` instead of `body` when the body is not UTF-8) or
  `{"error": "..."}`.
- The host follows no redirects; the plugin follows 301, 302, 303, 307 and 308
  through `location` itself, rechecking each URL, carrying the URL and hop
  count in `state`.
- With a `Source` data grant (`data_calls: true`), round one asks whether the
  URL's canonical id is stored in the graph; without it the plugin fetches
  straight away and a second successful fetch of the same URL fails the
  callback on the unique index.
- `fetch_now` (a `YYYY-MM-DDTHH:MM:SSZ` timestamp) replaces the clock for
  `fetched_at`; the plugin's own test cases use it.

## Testing

`gents pack test` runs `tests/*.json` with the manifest's grants in force,
so a case that reaches a fetch gets a real `http_calls` round against the
live network. Those cases therefore hold only inputs refused before any
fetch (bad scheme, credentials, length, port, localhost and internal
addresses, missing `graph_id`). The canned host rounds (first request,
existence query, HTML, plain text, HTTP error, unsupported type, redirects,
unreachable host, no network) live in `tests/fixtures/rounds/` and are
replayed through `run()` by `cargo test`.

## Authority

Outbound HTTP only (`OutboundHttp`); no files, environment or commands. An
unscoped grant reaches public `https` hosts on port 443; plain `http` or
other ports need an allow-list entry (`http://host` or `host:port`).
Responses are capped at 1 MiB by the host and requests at 30 s.
