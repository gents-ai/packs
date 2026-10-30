//! Fetches one `SourceRequest`'s URL through the host and answers with one
//! flat `Source` row (the callback has one port, so the output object is the
//! row). The plugin never opens a socket: it hands the host a GET
//! (`http_calls`) and reads the answer (`http_results`) on the next round,
//! following up to five redirects itself because the host follows none.
//! Every outcome is a row: a refused URL, a host without network, an
//! unreachable server, an HTTP error and an unsupported type are rows with
//! `status: "failed"` and the reason in `error`, never a missing row.
//!
//! Source ids: `s:` and the first 16 hex digits of sha256(url) belong to the
//! first successful fetch of that URL in the graph. Every other row (a failure,
//! a second fetch of a stored URL) takes `s:<16 hex>-<8 hex of
//! sha256(request_id)>`, so no row collides with the `(graph_id, source_id)`
//! unique index and a failure never blocks a later successful fetch.
use std::io::{Read, Write};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

mod base64;
mod clock;
mod extract;
mod url;

const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_REDIRECTS: u64 = 5;
const FETCH: &str = "fetch";
const EXISTING: &str = "existing";
const ACCEPT: &str = "text/html, text/plain, application/json, application/pdf;q=0.1";
const USER_AGENT: &str = "gents-claim-graph/0.3";
pub const NO_NETWORK: &str =
    "the host offers this plugin no network; grant OutboundHttp to claim_sources";

/// What the request document asks for. Built even from a bad input, so the
/// failed row still names the graph and URL it was given.
struct Request {
    request_id: String,
    graph_id: String,
    url: String,
    confidential: bool,
    note: Value,
    now: String,
}

/// What came back from the server, as far as it came.
#[derive(Default)]
struct Fetched {
    final_url: Option<String>,
    http_status: Option<u64>,
    content_type: Option<String>,
    content_hash: Option<String>,
    title: Option<String>,
    text: Option<String>,
    truncated: bool,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn canonical_id(url: &str) -> String {
    format!("s:{}", &sha256_hex(url.as_bytes())[..16])
}

pub fn request_scoped_id(url: &str, request_id: &str) -> String {
    format!("{}-{}", canonical_id(url), &sha256_hex(request_id.as_bytes())[..8])
}

fn text_field(input: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim().to_owned())),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

/// The request and the first thing wrong with it, if anything.
fn parse(input: &Value) -> (Request, Option<String>) {
    let empty = Map::new();
    let object = input.as_object();
    let input = object.unwrap_or(&empty);
    let mut problem = object.is_none().then(|| "the input must be a JSON object".to_owned());
    let mut note = |p: String| {
        problem.get_or_insert(p);
    };
    let mut text = |key: &str| {
        text_field(input, key).unwrap_or_else(|e| {
            note(e);
            None
        })
    };
    let request_id = text("request_id").unwrap_or_default();
    let graph_id = text("graph_id").unwrap_or_default();
    let url = text("url").unwrap_or_default();
    let note_text = text("note");
    let fetch_now = text("fetch_now");
    let confidential = match input.get("confidential") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            note("confidential must be true or false".into());
            false
        }
    };
    let now = match fetch_now {
        Some(now) if clock::is_iso_utc(&now) => now,
        Some(_) => {
            note("fetch_now must be a UTC timestamp like 2026-10-30T12:00:00Z".into());
            clock::now_iso()
        }
        None => clock::now_iso(),
    };
    if graph_id.is_empty() {
        note("graph_id is required: the graph the source belongs to".into());
    }
    if let Err(why) = url::check(&url) {
        note(why);
    }
    let url = url::redact(&url);
    let request = Request {
        request_id,
        graph_id,
        url,
        confidential,
        note: note_text.map_or(Value::Null, Value::String),
        now,
    };
    (request, problem)
}

fn opt(value: Option<String>) -> Value {
    value.map_or(Value::Null, Value::String)
}

/// The Source row, every field in schema order; `request_id` is left to the
/// host, which writes the correlation.
fn row(request: &Request, source_id: String, error: Option<String>, fetched: Fetched) -> Value {
    let ok = error.is_none();
    let text_chars = fetched.text.as_ref().map_or(0, |t| t.chars().count());
    json!({
        "source_id": source_id,
        "graph_id": request.graph_id,
        "url": request.url,
        "final_url": fetched.final_url.unwrap_or_else(|| request.url.clone()),
        "title": opt(fetched.title),
        "fetched_at": request.now,
        "status": if ok { "ok" } else { "failed" },
        "error": opt(error),
        "http_status": fetched.http_status,
        "content_type": opt(fetched.content_type),
        "content_hash": opt(fetched.content_hash),
        "text": opt(fetched.text),
        "text_chars": text_chars,
        "truncated": fetched.truncated,
        "confidential": request.confidential,
        "note": request.note,
    })
}

fn failed(request: &Request, error: impl Into<String>, fetched: Fetched) -> Value {
    let id = request_scoped_id(&request.url, &request.request_id);
    row(request, id, Some(error.into()), fetched)
}

/// Where a fetch stands between rounds.
struct Progress {
    url: String,
    hops: u64,
    /// No stored Source holds the URL's canonical id, so a success may take it.
    canonical_free: bool,
}

fn progress(request: &Request, state: Option<&Value>) -> Progress {
    let state = state.unwrap_or(&Value::Null);
    Progress {
        url: state["url"].as_str().unwrap_or(&request.url).to_owned(),
        hops: state["hops"].as_u64().unwrap_or(0),
        canonical_free: state["canonical_free"].as_bool().unwrap_or(true),
    }
}

fn ask_fetch(request: &Request, at: &Progress) -> Value {
    let mut calls = json!({"requests": [{
        "id": FETCH,
        "method": "GET",
        "url": at.url,
        "headers": {"accept": ACCEPT, "user-agent": USER_AGENT},
    }]});
    // The first request of a plain fetch needs no state: the defaults are the
    // request's URL, no hops and a free canonical id.
    if at.url != request.url || at.hops > 0 || !at.canonical_free {
        calls["state"] = json!({"url": at.url, "hops": at.hops, "canonical_free": at.canonical_free});
    }
    json!({"http_calls": calls})
}

fn ask_existing(request: &Request) -> Value {
    json!({"data_calls": {"requests": [{
        "id": EXISTING,
        "query": {
            "collection": "Source",
            "filter": {
                "graph_id": {"_eq": request.graph_id},
                "source_id": {"_eq": canonical_id(&request.url)},
            },
            "limit": 1,
            "fields": ["source_id", "status", "fetched_at"],
        },
    }]}})
}

pub fn run(input: &Value) -> Value {
    let (request, problem) = parse(input);
    if let Some(problem) = problem {
        return failed(&request, problem, Fetched::default());
    }
    let offered = input.get("http_calls") == Some(&Value::Bool(true));
    if let Some(results) = input.get("http_results") {
        return answer(&request, &progress(&request, input.get("state")), results);
    }
    if !offered {
        return failed(&request, NO_NETWORK, Fetched::default());
    }
    if let Some(results) = input.get("data_results") {
        return checked(&request, results);
    }
    if input.get("data_calls") == Some(&Value::Bool(true)) {
        return ask_existing(&request);
    }
    ask_fetch(&request, &progress(&request, None))
}

/// After the existence query: a URL already stored in this graph is not
/// fetched again; the row points at the stored source. A failed query does
/// not stop the fetch.
fn checked(request: &Request, results: &Value) -> Value {
    let rows = results
        .get(EXISTING)
        .and_then(|r| r.pointer("/result/rows"))
        .and_then(Value::as_array);
    let stored = rows.and_then(|rows| rows.first());
    if let Some(stored) = stored.filter(|s| s["status"].as_str() == Some("ok")) {
        let id = stored["source_id"].as_str().unwrap_or_default();
        let at = stored["fetched_at"].as_str().unwrap_or("earlier");
        return failed(
            request,
            format!("this url is already stored as source {id} (fetched {at}); cite that source_id"),
            Fetched::default(),
        );
    }
    let at = Progress {
        url: request.url.clone(),
        hops: 0,
        canonical_free: stored.is_none(),
    };
    ask_fetch(request, &at)
}

fn header<'a>(response: &'a Value, name: &str) -> Option<&'a str> {
    response["headers"].as_object()?.iter().find_map(|(k, v)| {
        k.eq_ignore_ascii_case(name).then(|| v.as_str()).flatten()
    })
}

fn charset(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|param| {
        let (key, value) = param.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"').to_ascii_lowercase())
    })
}

/// Body bytes as text: UTF-8 when they are, Latin-1 when the charset says so,
/// otherwise UTF-8 with replacement characters.
fn body_text(bytes: Vec<u8>, content_type: &str) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            let bytes = error.into_bytes();
            let latin = charset(content_type).is_some_and(|c| {
                matches!(c.as_str(), "iso-8859-1" | "latin1" | "latin-1" | "windows-1252" | "cp1252" | "us-ascii")
            });
            if latin {
                bytes.iter().map(|b| char::from(*b)).collect()
            } else {
                String::from_utf8_lossy(&bytes).into_owned()
            }
        }
    }
}

fn hint(status: u64) -> &'static str {
    match status {
        401 | 403 => "; the page needs a login or refuses this client, and this plugin sends no credentials",
        404 | 410 => "; the page is not there, check the address",
        429 | 503 => "; the server is busy or rate limiting, try again later",
        500..=599 => "; the server failed, try again later",
        _ => "",
    }
}

fn answer(request: &Request, at: &Progress, results: &Value) -> Value {
    let reached = Fetched {
        final_url: Some(at.url.clone()),
        ..Fetched::default()
    };
    let Some(response) = results.get(FETCH) else {
        return failed(request, "the host returned no answer for the fetch", reached);
    };
    if let Some(error) = response.get("error") {
        let error = error.as_str().unwrap_or("unknown error");
        return failed(request, format!("the fetch of {} failed: {error}", at.url), reached);
    }
    let Some(status) = response.get("status").and_then(Value::as_u64) else {
        return failed(request, "the host's answer carries no HTTP status", reached);
    };
    let bytes = match (response.get("body"), response.get("body_base64")) {
        (Some(Value::String(body)), _) => body.as_bytes().to_vec(),
        (_, Some(Value::String(encoded))) => match base64::decode(encoded) {
            Some(bytes) => bytes,
            None => return failed(request, "the host's body_base64 is not base64", reached),
        },
        _ => Vec::new(),
    };
    let content_type = header(response, "content-type").map(str::to_owned);
    let fetched = Fetched {
        final_url: Some(at.url.clone()),
        http_status: Some(status),
        content_type: content_type.clone(),
        content_hash: Some(format!("sha256:{}", sha256_hex(&bytes))),
        ..Fetched::default()
    };
    if matches!(status, 301 | 302 | 303 | 307 | 308) {
        let Some(location) = header(response, "location") else {
            return failed(request, format!("the server answered HTTP {status} without a location"), fetched);
        };
        if at.hops >= MAX_REDIRECTS {
            return failed(request, format!("the server redirected more than {MAX_REDIRECTS} times"), fetched);
        }
        let next = match url::resolve(&at.url, location).and_then(|next| url::check(&next).map(|()| next)) {
            Ok(next) => next,
            Err(why) => {
                return failed(request, format!("the redirect from {} to {location} is refused: {why}", at.url), fetched);
            }
        };
        let next = Progress {
            url: next,
            hops: at.hops + 1,
            canonical_free: at.canonical_free,
        };
        return ask_fetch(request, &next);
    }
    if !(200..300).contains(&status) {
        return failed(request, format!("the server answered HTTP {status}{}", hint(status)), fetched);
    }
    let Some(content_type) = content_type else {
        return failed(request, "the server sent no content type", fetched);
    };
    let media = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let (title, text) = match media.as_str() {
        "text/html" | "application/xhtml+xml" => {
            let page = body_text(bytes, &content_type);
            (extract::title(&page), extract::html_text(&page))
        }
        "text/plain" | "application/json" => (None, body_text(bytes, &content_type)),
        _ => return failed(request, format!("unsupported content type {media}"), fetched),
    };
    let (text, truncated) = extract::cap(text);
    let id = if at.canonical_free {
        canonical_id(&request.url)
    } else {
        request_scoped_id(&request.url, &request.request_id)
    };
    let fetched = Fetched {
        title,
        text: Some(text),
        truncated,
        ..fetched
    };
    row(request, id, None, fetched)
}

fn main() -> std::io::Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let input = if bytes.len() > MAX_INPUT_BYTES {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
    };
    let output = run(&input);
    std::io::stdout().write_all(output.to_string().as_bytes())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
