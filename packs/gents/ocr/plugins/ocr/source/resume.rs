//! Continuation of a call that stopped early: the position inside one file
//! ([`Resume`]) and the opaque cursor that carries it together with the file
//! it belongs to and the request it was issued for, so a caller repeating the
//! call with the cursor neither repeats nor skips anything.
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const PREFIX: &str = "ocr1.";

/// A 64-bit FNV-1a hash: fixed by definition, so a fingerprint means the same
/// in every call of every build.
pub struct Fnv(u64);

impl Fnv {
    pub fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    pub fn write(&mut self, bytes: &[u8]) -> &mut Self {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        self
    }

    /// A separator so `("ab", "c")` and `("a", "bc")` hash differently.
    pub fn field(&mut self, bytes: &[u8]) -> &mut Self {
        self.write(bytes).write(&[0xff])
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

/// Where inside one file the next call continues.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Resume {
    /// The 1-based page, slide, sheet or section to continue with.
    pub unit: u32,
    /// A position inside the unit: the byte offset of the next element, row or line.
    pub pos: u64,
    /// Bytes of the unit's rendered text already delivered, for a unit larger than one call.
    pub skip: u64,
    /// Line breaks between the delivered part and the rest: 0 (a cut inside a line), 1 or 2.
    pub joint: u8,
    /// The figure counter at the start of the unit.
    pub fig: u32,
    /// Hashes (hex) of the images already listed in the document.
    pub seen: Vec<String>,
    /// State a format carries from one call to the next.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub st: Option<Value>,
}

impl Resume {
    /// Whether this is the start of the file, which is read like a fresh call.
    pub fn is_start(&self) -> bool {
        self.unit == 0 && self.pos == 0 && self.skip == 0
    }

    /// The joint name a continuation reports.
    pub fn joint_name(&self) -> &'static str {
        match self.joint {
            0 => "none",
            1 => "line",
            _ => "blank",
        }
    }
}

/// What a cursor says: the request, the file and the position.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Payload {
    pub v: u8,
    /// Fingerprint of the request options the cursor is valid for.
    pub req: u64,
    /// Index of the file in the request's file list.
    pub file: u32,
    pub name: String,
    pub len: u64,
    /// Fingerprint of the file's content (see `Src::fingerprint`).
    pub fp: u64,
    pub at: Resume,
}

pub fn encode(p: &Payload) -> Result<String, String> {
    let json = serde_json::to_vec(p).map_err(|e| format!("cannot write the cursor: {e}"))?;
    let sum = Fnv::new().write(&json).finish();
    Ok(format!(
        "{PREFIX}{}.{sum:016x}",
        URL_SAFE_NO_PAD.encode(json)
    ))
}

pub fn decode(cursor: &str) -> Result<Payload, String> {
    let bad = || {
        "the cursor is not one this plugin returned; pass the next.cursor value unchanged"
            .to_string()
    };
    let body = cursor.strip_prefix(PREFIX).ok_or_else(bad)?;
    let (data, sum) = body.rsplit_once('.').ok_or_else(bad)?;
    let json = URL_SAFE_NO_PAD.decode(data).map_err(|_| bad())?;
    let want = u64::from_str_radix(sum, 16).map_err(|_| bad())?;
    if Fnv::new().write(&json).finish() != want {
        return Err("the cursor was damaged; pass the next.cursor value unchanged".into());
    }
    let p: Payload = serde_json::from_slice(&json).map_err(|_| bad())?;
    if p.v != 1 {
        return Err(bad());
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> Payload {
        Payload {
            v: 1,
            req: 7,
            file: 2,
            name: "a/b.pdf".into(),
            len: 99,
            fp: 5,
            at: Resume {
                unit: 4,
                pos: 1000,
                skip: 0,
                joint: 2,
                fig: 3,
                seen: vec!["ff".into()],
                st: Some(serde_json::json!({"x": 1})),
            },
        }
    }

    #[test]
    fn a_cursor_round_trips() {
        let c = encode(&payload()).unwrap();
        assert!(c.starts_with("ocr1.") && !c.contains(['+', '/', '=']));
        assert_eq!(decode(&c).unwrap(), payload());
    }

    #[test]
    fn a_damaged_or_foreign_cursor_is_refused() {
        let c = encode(&payload()).unwrap();
        let mut bytes = c.clone().into_bytes();
        bytes[8] = if bytes[8] == b'A' { b'B' } else { b'A' };
        let flipped = String::from_utf8(bytes).unwrap();
        assert!(decode(&flipped).unwrap_err().contains("damaged"));
        for bad in [
            "",
            "ocr1.",
            "xyz",
            "ocr1.!!.00",
            "ocr1.e30.0000000000000000",
        ] {
            assert!(decode(bad).is_err(), "{bad}");
        }
        let foreign = encode(&Payload { v: 2, ..payload() }).unwrap();
        assert!(decode(&foreign).is_err());
    }

    #[test]
    fn fnv_fields_are_separated() {
        let a = Fnv::new().field(b"ab").field(b"c").finish();
        let b = Fnv::new().field(b"a").field(b"bc").finish();
        assert_ne!(a, b);
        assert_eq!(Fnv::new().write(b"").finish(), 0xcbf2_9ce4_8422_2325);
    }
}
