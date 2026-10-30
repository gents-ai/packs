use super::*;

const NOW: &str = "2026-10-30T12:00:00Z";
const URL: &str = "https://example.org/fcc/flipper";

fn input(extra: Value) -> Value {
    let mut base = json!({"request_id": "sr-1", "graph_id": "g", "url": URL,
        "confidential": false, "note": "", "fetch_now": NOW, "submitted_at": "x"});
    for (k, v) in extra.as_object().unwrap() {
        base[k] = v.clone();
    }
    base
}

fn response(status: u64, content_type: Option<&str>, body: &str) -> Value {
    let mut headers = Map::new();
    if let Some(ct) = content_type {
        headers.insert("content-type".into(), json!(ct));
    }
    json!({"fetch": {"status": status, "headers": headers, "body": body}})
}

fn answered(results: Value) -> Value {
    run(&input(json!({"http_calls": true, "http_results": results})))
}

#[test]
fn ids_are_the_url_hash_and_a_request_scoped_variant() {
    let id = canonical_id(URL);
    assert_eq!(id.len(), 18);
    assert!(id.starts_with("s:") && id[2..].bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(id, format!("s:{}", &sha256_hex(URL.as_bytes())[..16]));
    let scoped = request_scoped_id(URL, "sr-1");
    assert!(scoped.starts_with(&format!("{id}-")) && scoped.len() == 27);
    assert_ne!(scoped, request_scoped_id(URL, "sr-2"));
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn the_first_round_asks_for_one_get() {
    let out = run(&input(json!({"http_calls": true})));
    assert_eq!(out, json!({"http_calls": {"requests": [{"id": "fetch", "method": "GET", "url": URL,
        "headers": {"accept": ACCEPT, "user-agent": USER_AGENT}}]}}));
}

#[test]
fn with_data_calls_it_first_asks_whether_the_url_is_stored() {
    let out = run(&input(json!({"http_calls": true, "data_calls": true})));
    let q = &out["data_calls"]["requests"][0]["query"];
    assert_eq!(q["collection"], "Source");
    assert_eq!(q["filter"], json!({"graph_id": {"_eq": "g"}, "source_id": {"_eq": canonical_id(URL)}}));
    let none = run(&input(json!({"http_calls": true, "data_calls": true,
        "data_results": {"existing": {"result": {"rows": []}}}})));
    assert_eq!(none, run(&input(json!({"http_calls": true}))));
    let stored = run(&input(json!({"http_calls": true, "data_calls": true,
        "data_results": {"existing": {"result": {"rows": [{"source_id": canonical_id(URL), "status": "ok", "fetched_at": "2026-10-01T00:00:00Z"}]}}}})));
    assert_eq!(stored["status"], "failed");
    assert_eq!(stored["source_id"], request_scoped_id(URL, "sr-1"));
    assert_eq!(stored["error"], format!("this url is already stored as source {} (fetched 2026-10-01T00:00:00Z); cite that source_id", canonical_id(URL)));
    let errored = run(&input(json!({"http_calls": true, "data_calls": true,
        "data_results": {"existing": {"error": "boom"}}})));
    assert_eq!(errored, run(&input(json!({"http_calls": true}))));
    let taken = run(&input(json!({"http_calls": true, "data_calls": true,
        "data_results": {"existing": {"result": {"rows": [{"source_id": canonical_id(URL), "status": "failed"}]}}}})));
    assert_eq!(taken["http_calls"]["state"], json!({"url": URL, "hops": 0, "canonical_free": false}));
    let ok = run(&input(json!({"http_calls": true, "state": taken["http_calls"]["state"].clone(),
        "http_results": response(200, Some("text/plain"), "hello")})));
    assert_eq!((ok["status"].as_str(), ok["source_id"].as_str()), (Some("ok"), Some(request_scoped_id(URL, "sr-1").as_str())));
}

#[test]
fn an_html_page_becomes_a_source_row() {
    let page = "<!doctype html><html><head><title>FCC &amp; Flipper</title><style>p{}</style></head>\
        <body><nav>Home | About</nav><h1>Flipper Zero</h1><p>The device holds FCC ID <b>2A2V6-FZ</b>.</p>\
        <script>var x = '<p>no</p>';</script><p>Granted&nbsp;2020.</p><footer>(c) site</footer></body></html>";
    let out = answered(response(200, Some("text/html; charset=utf-8"), page));
    let expected_hash = format!("sha256:{}", sha256_hex(page.as_bytes()));
    assert_eq!(out, json!({
        "source_id": canonical_id(URL), "graph_id": "g", "url": URL, "final_url": URL,
        "title": "FCC & Flipper", "fetched_at": NOW, "status": "ok", "error": null,
        "http_status": 200, "content_type": "text/html; charset=utf-8", "content_hash": expected_hash,
        "text": "Flipper Zero\nThe device holds FCC ID 2A2V6-FZ.\nGranted 2020.", "text_chars": 60,
        "truncated": false, "confidential": false, "note": ""}));
    assert!(out.get("request_id").is_none());
    assert!(out.as_object().unwrap().values().all(|v| !v.is_object() && !v.is_array()));
}

#[test]
fn plain_text_and_json_are_kept_verbatim() {
    let body = "line one\n\n  line two  \n";
    let out = answered(response(200, Some("text/plain"), body));
    assert_eq!((out["text"].as_str(), out["title"].clone()), (Some(body), Value::Null));
    let out = answered(response(200, Some("application/json"), "{\"a\": 1}"));
    assert_eq!(out["text"], "{\"a\": 1}");
}

#[test]
fn failures_are_failed_rows() {
    let scoped = request_scoped_id(URL, "sr-1");
    let out = answered(response(404, Some("text/html"), "<p>gone</p>"));
    assert_eq!((out["status"].as_str(), out["source_id"].as_str()), (Some("failed"), Some(scoped.as_str())));
    assert_eq!(out["error"], "the server answered HTTP 404; the page is not there, check the address");
    assert_eq!(out["http_status"], 404);
    assert!(out["text"].is_null());
    let out = answered(response(200, Some("application/pdf"), "%PDF"));
    assert_eq!(out["error"], "unsupported content type application/pdf");
    let out = answered(response(200, None, "x"));
    assert_eq!(out["error"], "the server sent no content type");
    let out = answered(json!({"fetch": {"error": "example.org could not be reached"}}));
    assert_eq!(out["error"], format!("the fetch of {URL} failed: example.org could not be reached"));
    assert!(out["http_status"].is_null() && out["content_hash"].is_null());
    let out = answered(json!({}));
    assert_eq!(out["error"], "the host returned no answer for the fetch");
    let out = run(&input(json!({})));
    assert_eq!(out["error"], NO_NETWORK);
    assert_eq!(out["source_id"], scoped);
}

#[test]
fn bad_inputs_are_failed_rows_naming_the_problem() {
    let cases = [
        (json!({"url": "ftp://example.org/x"}), "url must start with http:// or https://"),
        (json!({"url": "https://user:pw@example.org/"}), "url must not carry credentials (user:password@host); this plugin sends none"),
        (json!({"url": "http://127.0.0.1:9/"}), "url names the internal address 127.0.0.1; this plugin fetches public addresses only"),
        (json!({"url": "http://10.1.2.3/"}), "url names the internal address 10.1.2.3; this plugin fetches public addresses only"),
        (json!({"url": "http://[::1]/"}), "url names the internal address [::1]; this plugin fetches public addresses only"),
        (json!({"url": "http://[fd00::1]:8080/"}), "url names the internal address [fd00::1]; this plugin fetches public addresses only"),
        (json!({"url": "http://127.1/"}), "url names 127.1; write an IPv4 address as four decimal parts"),
        (json!({"url": "http://localhost:8080/"}), "url names localhost, this machine; this plugin fetches public addresses only"),
        (json!({"url": "https:///path"}), "url names no host"),
        (json!({"url": "https://example.org:99999/"}), "url has the port \"99999\"; a port is 1 to 65535"),
        (json!({"url": "https://exa mple.org/"}), "url holds a space, a control character or a backslash; percent-encode it"),
        (json!({"url": format!("https://example.org/{}", "a".repeat(2048))}), "url is longer than 2048 characters"),
        (json!({"url": null}), "url is required: the http or https address to fetch"),
        (json!({"url": 5}), "url must be a string"),
        (json!({"graph_id": ""}), "graph_id is required: the graph the source belongs to"),
        (json!({"confidential": "yes"}), "confidential must be true or false"),
        (json!({"fetch_now": "today"}), "fetch_now must be a UTC timestamp like 2026-10-30T12:00:00Z"),
    ];
    for (extra, error) in cases {
        let out = run(&input(json!({"http_calls": true}).as_object().unwrap().clone().into_iter().chain(extra.as_object().unwrap().clone()).collect::<Map<_, _>>().into()));
        assert_eq!(out["status"], "failed", "{extra}");
        assert_eq!(out["error"], error, "{extra}");
    }
    assert!(url::check("https://8.8.8.8/").is_ok());
    assert!(url::check("https://[2606:4700::1111]/").is_ok());
    assert!(url::check("https://cafe.de/").is_ok());
    assert!(url::check("HTTPS://Example.org:443/a?b#c").is_ok());
    let out = run(&json!([1]));
    assert_eq!(out["error"], "the input must be a JSON object");
}

#[test]
fn redirects_are_followed_up_to_five_times() {
    let mut headers = Map::new();
    headers.insert("location".into(), json!("/new?x=1"));
    let first = answered(json!({"fetch": {"status": 301, "headers": headers.clone(), "body": ""}}));
    let next = "https://example.org/new?x=1";
    assert_eq!(first["http_calls"]["requests"][0]["url"], next);
    assert_eq!(first["http_calls"]["state"], json!({"url": next, "hops": 1, "canonical_free": true}));
    let done = run(&input(json!({"http_calls": true, "state": first["http_calls"]["state"].clone(),
        "http_results": response(200, Some("text/plain"), "moved")})));
    assert_eq!((done["final_url"].as_str(), done["url"].as_str(), done["source_id"].as_str()),
        (Some(next), Some(URL), Some(canonical_id(URL).as_str())));
    let too_many = run(&input(json!({"http_calls": true, "state": {"url": next, "hops": 5},
        "http_results": {"fetch": {"status": 302, "headers": headers, "body": ""}}})));
    assert_eq!(too_many["error"], "the server redirected more than 5 times");
    let mut inward = Map::new();
    inward.insert("Location".into(), json!("http://192.168.1.1/admin"));
    let refused = answered(json!({"fetch": {"status": 302, "headers": inward, "body": ""}}));
    assert_eq!(refused["error"], format!("the redirect from {URL} to http://192.168.1.1/admin is refused: url names the internal address 192.168.1.1; this plugin fetches public addresses only"));
    let bare = answered(json!({"fetch": {"status": 302, "headers": {}, "body": ""}}));
    assert_eq!(bare["error"], "the server answered HTTP 302 without a location");
}

#[test]
fn redirect_locations_resolve_against_the_answering_url() {
    let base = "https://a.org/dir/page.html?q=1";
    for (loc, want) in [
        ("https://b.org/x", "https://b.org/x"),
        ("//c.org/y", "https://c.org/y"),
        ("/root", "https://a.org/root"),
        ("?p=2", "https://a.org/dir/page.html?p=2"),
        ("other.html", "https://a.org/dir/other.html"),
    ] {
        assert_eq!(url::resolve(base, loc).unwrap(), want, "{loc}");
    }
    assert_eq!(url::resolve("https://a.org", "x").unwrap(), "https://a.org/x");
}

#[test]
fn base64_bodies_are_hashed_raw_and_decoded_by_charset() {
    // "caf\xe9" in Latin-1, not UTF-8, so the host sends it as body_base64.
    let out = answered(json!({"fetch": {"status": 200,
        "headers": {"content-type": "text/plain; charset=ISO-8859-1"}, "body_base64": "Y2Fm6Q=="}}));
    assert_eq!(out["text"], "caf\u{e9}");
    assert_eq!(out["content_hash"], format!("sha256:{}", sha256_hex(b"caf\xe9")));
    let out = answered(json!({"fetch": {"status": 200, "headers": {"content-type": "text/plain"}, "body_base64": "!!"}}));
    assert_eq!(out["error"], "the host's body_base64 is not base64");
}

#[test]
fn text_is_capped_at_200000_characters() {
    let body = "\u{e9}".repeat(extract::MAX_TEXT_CHARS + 10);
    let out = answered(response(200, Some("text/plain"), &body));
    assert_eq!((out["text_chars"].as_u64(), out["truncated"].as_bool()), (Some(200_000), Some(true)));
    assert_eq!(out["content_hash"], format!("sha256:{}", sha256_hex(body.as_bytes())));
    let (kept, cut) = extract::cap("abc".into());
    assert_eq!((kept.as_str(), cut), ("abc", false));
    let exact = "x".repeat(extract::MAX_TEXT_CHARS);
    assert!(!extract::cap(exact).1);
}

#[test]
fn html_extraction_handles_the_awkward_parts() {
    let cases = [
        ("<p>a <!-- hidden --> b</p>", "a b"),
        ("<p>x &lt; y &#38; z &#x41; &bogus; &amp</p>", "x < y & z A &bogus; &amp"),
        ("<p>one<br>two<br/>three</p>", "one\ntwo\nthree"),
        ("<p><span>in</span><span>line</span></p>", "inline"),
        ("<a href=\"x>y\" title='>'>link</a> after", "link after"),
        ("<ul><li>a</li><li>b</li></ul>", "a\nb"),
        ("<table><tr><td>1</td><td>2</td></tr></table>", "1\n2"),
        ("3 < 4 and 5 > 2", "3 < 4 and 5 > 2"),
        ("<script>never closed <p>x</p>", ""),
        ("<nav>unclosed nav <p>kept</p>", "unclosed nav\nkept"),
        ("<header>Masthead</header><main>Body</main>", "Masthead\nBody"),
        ("<svg><title>icon</title></svg><p>t</p>", "t"),
        ("<p>caf&eacute; &mdash; &ldquo;q&rdquo; &rsquo;s</p>", "caf\u{e9} \u{2014} \u{201c}q\u{201d} \u{2019}s"),
        ("<P>Upper</P><SCRIPT>x</SCRIPT>", "Upper"),
        ("<![CDATA[ raw ]]><p>after</p>", "after"),
        ("<p>tab\tand\n  newline</p>", "tab and newline"),
    ];
    for (html, want) in cases {
        assert_eq!(extract::html_text(html), want, "{html}");
    }
    assert_eq!(extract::title("<html><head><title>never closed"), None);
    assert_eq!(extract::title("<head><title>\n A &amp; B \n</title></head>").as_deref(), Some("A & B"));
    assert_eq!(extract::title("<titlex>no</titlex>"), None);
    assert_eq!(extract::title("<title></title>"), None);
}

#[test]
fn credentials_are_not_stored() {
    let out = run(&input(json!({"http_calls": true, "url": "https://admin:secret@example.org/a@b"})));
    assert_eq!(out["url"], "https://***@example.org/a@b");
    assert!(!out.to_string().contains("secret"));
    assert_eq!(url::redact("https://example.org/a@b"), "https://example.org/a@b");
    assert_eq!(url::redact("ftp://u@h"), "ftp://***@h");
}

/// Replays every case file in `dir` through `run()`.
fn replay(dir: &str) -> usize {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let case: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(&run(&case["input"]), &case["expect"], "{}", path.display());
        checked += 1;
    }
    checked
}

/// The host's plugin cases: refused before any fetch, so the same offline
/// and under a real network grant.
#[test]
fn every_plugin_case_matches_this_build() {
    assert_eq!(replay("tests"), 9);
}

/// Canned host rounds (http and data results). `gents pack test` serves real
/// http_calls under the manifest's grant, so these live here, not in `tests/`.
#[test]
fn every_canned_round_matches_this_build() {
    assert_eq!(replay("tests/fixtures/rounds"), 11);
}
