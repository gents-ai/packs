//! The URLs this plugin asks the host to fetch: checked before every request
//! (the first and each redirect) so a refusal names the caller's mistake
//! instead of the host's allow-list. The host checks again on its side,
//! including the addresses a host name resolves to; this module only sees the
//! text, so a public name that resolves to an internal address is the host's
//! refusal, reported as the fetch error.
use std::net::{Ipv4Addr, Ipv6Addr};

pub const MAX_URL_CHARS: usize = 2048;

/// A URL split at its parts, as far as resolving a redirect needs.
#[derive(Debug, PartialEq, Eq)]
pub struct Parts<'a> {
    /// `http` or `https`, lowercased.
    pub scheme: String,
    pub authority: &'a str,
    /// From the first `/` up to `?` or `#`; empty when the URL has none.
    pub path: &'a str,
    /// From `?` up to `#`, with the `?`; empty when none.
    pub query: &'a str,
}

pub fn split(url: &str) -> Result<Parts<'_>, String> {
    let (scheme, rest) = url
        .split_once("://")
        .filter(|(s, _)| s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https"))
        .ok_or_else(|| "url must start with http:// or https://".to_owned())?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let tail = tail.split('#').next().unwrap_or_default();
    let (path, query) = match tail.find('?') {
        Some(at) => tail.split_at(at),
        None => (tail, ""),
    };
    Ok(Parts {
        scheme: scheme.to_ascii_lowercase(),
        authority,
        path,
        query,
    })
}

/// Refuses what the plugin will not ask for: another scheme, credentials,
/// a URL longer than 2048 characters, a missing host, `localhost` and IP
/// literals in loopback, private, link-local, shared, reserved or multicast
/// ranges (the ranges the host treats as internal).
pub fn check(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("url is required: the http or https address to fetch".into());
    }
    if url.chars().count() > MAX_URL_CHARS {
        return Err(format!("url is longer than {MAX_URL_CHARS} characters"));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\') {
        return Err("url holds a space, a control character or a backslash; percent-encode it".into());
    }
    let parts = split(url)?;
    if parts.authority.contains('@') {
        return Err("url must not carry credentials (user:password@host); this plugin sends none".into());
    }
    let host = host(parts.authority)?;
    let lower = host.to_ascii_lowercase();
    let lower = lower.trim_end_matches('.');
    if lower == "localhost" || lower.ends_with(".localhost") {
        return Err(format!("url names {host}, this machine; this plugin fetches public addresses only"));
    }
    if let Some(v6) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        let ip: Ipv6Addr = v6
            .parse()
            .map_err(|_| format!("url names {host}, which is not an IPv6 address"))?;
        if !v6_public(&ip) {
            return Err(internal(host));
        }
        return Ok(());
    }
    if looks_numeric(lower) {
        let ip: Ipv4Addr = lower.parse().map_err(|_| {
            format!("url names {host}; write an IPv4 address as four decimal parts")
        })?;
        if !v4_public(&ip) {
            return Err(internal(host));
        }
    }
    Ok(())
}

fn internal(host: &str) -> String {
    format!("url names the internal address {host}; this plugin fetches public addresses only")
}

/// The host part of an authority, without the port; the port must be 1 to 65535.
fn host(authority: &str) -> Result<&str, String> {
    let (host, port) = if authority.starts_with('[') {
        match authority.find(']') {
            Some(end) => {
                let (host, rest) = authority.split_at(end + 1);
                (host, rest.strip_prefix(':'))
            }
            None => return Err("url has an unclosed [ in its host".into()),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if let Some(port) = port {
        if !port.is_empty()
            && !(port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p > 0))
        {
            return Err(format!("url has the port {port:?}; a port is 1 to 65535"));
        }
    }
    if host.is_empty() {
        return Err("url names no host".into());
    }
    Ok(host)
}

/// The WHATWG URL parser reads a host whose last label is a number (decimal
/// or `0x` hex) as an IPv4 address, in shorthand forms like `127.1` too.
fn looks_numeric(host: &str) -> bool {
    let last = host.rsplit('.').next().unwrap_or_default();
    !last.is_empty()
        && (last.bytes().all(|b| b.is_ascii_digit())
            || last
                .strip_prefix("0x")
                .is_some_and(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit())))
}

/// The host's `v4_public` (gents `plugin/http_calls.rs`), kept equal.
pub fn v4_public(ip: &Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..128).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || a >= 224)
}

/// The host's `v6_public`, kept equal.
pub fn v6_public(ip: &Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return v4_public(&v4);
    }
    let segments = ip.segments();
    if segments[0] == 0x2002 {
        let [hi, lo] = [segments[1].to_be_bytes(), segments[2].to_be_bytes()];
        return v4_public(&Ipv4Addr::new(hi[0], hi[1], lo[0], lo[1]));
    }
    (segments[0] & 0xe000) == 0x2000
        && !(segments[0] == 0x2001
            && (segments[1] == 0 || segments[1] == 0x0db8 || (0x10..0x30).contains(&segments[1])))
}

/// A redirect's `Location` against the URL that answered: absolute,
/// scheme-relative (`//host/...`), absolute-path, query-only or relative to
/// the answering path's folder. Dot segments are left for the server.
pub fn resolve(base: &str, location: &str) -> Result<String, String> {
    let location = location.trim();
    if location.is_empty() {
        return Err("the redirect named no location".into());
    }
    if let Some((scheme, _)) = location.split_once("://") {
        if scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
            return Ok(location.to_owned());
        }
    }
    let parts = split(base)?;
    let origin = format!("{}://{}", parts.scheme, parts.authority);
    Ok(if let Some(rest) = location.strip_prefix("//") {
        format!("{}://{rest}", parts.scheme)
    } else if location.starts_with('/') {
        format!("{origin}{location}")
    } else if location.starts_with('?') {
        let path = if parts.path.is_empty() { "/" } else { parts.path };
        format!("{origin}{path}{location}")
    } else if location.starts_with('#') {
        format!("{origin}{}{}{location}", if parts.path.is_empty() { "/" } else { parts.path }, parts.query)
    } else {
        let folder = parts.path.rfind('/').map_or("/", |at| &parts.path[..=at]);
        format!("{origin}{folder}{location}")
    })
}

/// The URL with any `user:password@` replaced by `***@`, so a refused URL's
/// credentials are not stored in the failed row.
pub fn redact(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..end].rfind('@') {
        Some(at) => format!("{scheme}://***{}", &rest[at..]),
        None => url.to_owned(),
    }
}
