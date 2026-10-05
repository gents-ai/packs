//! Paging. A call that stops early returns an opaque cursor; sending it back
//! with the same request continues exactly where the call stopped, so the
//! pieces joined together are the one-shot result. A cursor records which
//! request it belongs to and, when it stops inside one image, which file that
//! was, so a changed request or file is refused with a sentence.
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::sha256_hex;

const PREFIX: &str = "imgt1.";

/// Where a call stopped.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// The request this cursor continues.
    pub req: String,
    /// Index of the source to continue in.
    pub item: usize,
    /// Index of the first tile still to produce inside that source.
    pub tile: usize,
    /// Size of the source file when `tile` is not 0.
    pub len: u64,
    /// SHA-256 of the source file when `tile` is not 0.
    pub sha: String,
}

/// Identifies a request: a digest of everything that decides what a call returns.
pub fn fingerprint(request: &Value) -> String {
    sha256_hex(request.to_string().as_bytes())
}

/// The cursor as the opaque text handed to the caller.
pub fn encode(c: &Cursor) -> String {
    let json = serde_json::to_vec(c).unwrap_or_default();
    format!(
        "{PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    )
}

/// Reads a cursor and checks it belongs to the request `req` over `items` sources.
pub fn decode(text: &str, req: &str, items: usize) -> Result<Cursor, String> {
    let bad = || {
        "the cursor is not valid; use the next.cursor of a previous result unchanged".to_string()
    };
    let body = text.strip_prefix(PREFIX).ok_or_else(bad)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| bad())?;
    let c: Cursor = serde_json::from_slice(&bytes).map_err(|_| bad())?;
    if c.req != req {
        return Err(
            "the cursor belongs to a different request; repeat the request it came from".into(),
        );
    }
    if c.item >= items {
        return Err(
            "the files changed since the cursor was returned; start again without a cursor".into(),
        );
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cur(req: &str) -> Cursor {
        Cursor {
            req: req.into(),
            item: 2,
            tile: 17,
            len: 99,
            sha: "ab".into(),
        }
    }

    #[test]
    fn a_cursor_round_trips_and_is_url_safe_text() {
        let req = fingerprint(&json!({"a": 1}));
        let text = encode(&cur(&req));
        assert!(text.starts_with("imgt1."));
        assert!(
            text.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
        );
        assert_eq!(decode(&text, &req, 3).unwrap(), cur(&req));
    }

    #[test]
    fn the_fingerprint_follows_the_request() {
        let a = fingerprint(&json!({"ops": [{"op": "view"}], "files": ["a"]}));
        assert_eq!(
            a,
            fingerprint(&json!({"files": ["a"], "ops": [{"op": "view"}]})),
            "key order does not matter"
        );
        assert_ne!(
            a,
            fingerprint(&json!({"ops": [{"op": "view", "max_side": 5}], "files": ["a"]}))
        );
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn a_foreign_garbled_or_out_of_range_cursor_is_refused() {
        let req = fingerprint(&json!(1));
        let other = fingerprint(&json!(2));
        let text = encode(&cur(&req));
        assert!(
            decode(&text, &other, 3)
                .err()
                .unwrap()
                .contains("different request")
        );
        assert!(
            decode(&text, &req, 2)
                .err()
                .unwrap()
                .contains("files changed")
        );
        for junk in [
            "",
            "imgt1.",
            "imgt1.!!!",
            "ocr1.abc",
            "imgt1.e30",
            &text[..text.len() - 3],
        ] {
            assert!(
                decode(junk, &req, 3).err().unwrap().contains("not valid"),
                "{junk:?}"
            );
        }
    }
}
