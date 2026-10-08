//! Typed parsing of the JSON a caller sends, with one plain sentence for a
//! failure. The serde message names a Rust type (`expected u32`), which means
//! nothing to a caller, so a type or range failure is traced back to the
//! option that caused it and reported as what that option must be.
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Reads `v` as a `T`. `who` names what is being read (`step 2 (resize)`, `the request`);
/// `tags` are the keys that pick an enum variant, which are never the culprit of a type failure.
pub fn from_value<T: DeserializeOwned>(
    mut v: Value,
    who: &str,
    tags: &[&str],
) -> Result<T, String> {
    match T::deserialize(&v) {
        Ok(t) => Ok(t),
        Err(e) => Err(describe::<T>(&mut v, who, tags, &e.to_string())),
    }
}

/// The sentence for the failure `msg` of reading `v` as a `T`.
fn describe<T: DeserializeOwned>(v: &mut Value, who: &str, tags: &[&str], msg: &str) -> String {
    // These name options the caller wrote or left out, which is already plain.
    if ["unknown field", "missing field", "unknown variant"]
        .iter()
        .any(|p| msg.starts_with(p))
    {
        return format!("{who} is not valid: {msg}");
    }
    let Some(keys) = v.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()) else {
        return format!("{who} is not valid; send a JSON object");
    };
    // The culprit is the first option whose removal changes the failure: removing any other
    // option leaves the same failure (a required option then fails later, as a missing field).
    for key in keys.into_iter().filter(|k| !tags.contains(&k.as_str())) {
        let Some(held) = v.as_object_mut().and_then(|o| o.remove(&key)) else {
            continue;
        };
        let changed = T::deserialize(&*v)
            .err()
            .is_none_or(|e| e.to_string() != msg);
        if let Some(o) = v.as_object_mut() {
            o.insert(key.clone(), held.clone());
        }
        if changed {
            return format!("{who} is not valid: {}", about(&key, &held, msg));
        }
    }
    format!("{who} is not valid; check the type of each option")
}

/// What option `key`, holding `held`, must be, from the serde failure `msg`.
fn about(key: &str, held: &Value, msg: &str) -> String {
    if msg.starts_with("invalid length") {
        return format!("{key} has the wrong number of entries");
    }
    let want = match msg.rsplit_once(", expected ").map(|(_, e)| e) {
        Some("u8") => "a whole number from 0 to 255".to_owned(),
        Some("u16") => "a whole number from 0 to 65535".to_owned(),
        Some("u32") => "a whole number from 0 to 4294967295".to_owned(),
        Some("u64" | "usize") => "a whole number, 0 or more".to_owned(),
        Some("i8" | "i16" | "i32" | "i64" | "isize") => "a whole number".to_owned(),
        Some("f32" | "f64") => "a number".to_owned(),
        Some("a boolean") => "true or false".to_owned(),
        Some("a string") => "text".to_owned(),
        Some("a sequence") => "a list".to_owned(),
        _ => return format!("{key} has a value of the wrong type"),
    };
    // A list or object that holds the bad value, not itself the wrong kind.
    if (held.is_array() || held.is_object()) && !want.starts_with("a list") {
        format!("{key} holds a value that must be {want}")
    } else {
        format!("{key} must be {want}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Probe {
        width: u32,
        #[serde(default)]
        small: Option<u8>,
        #[serde(default)]
        scale: Option<f64>,
        #[serde(default)]
        flag: Option<bool>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        list: Option<Vec<u32>>,
        #[serde(default)]
        at: Option<[i64; 2]>,
    }

    fn err(v: Value) -> String {
        from_value::<Probe>(v, "step 1 (probe)", &[]).unwrap_err()
    }

    #[test]
    fn a_good_value_parses() {
        let p: Probe =
            from_value(json!({"width": 5, "small": 255, "at": [1, -2]}), "x", &[]).unwrap();
        assert_eq!((p.width, p.small, p.at), (5, Some(255), Some([1, -2])));
    }

    #[test]
    fn bad_numbers_say_what_the_option_must_be_without_a_type_name() {
        for (v, want) in [
            (
                json!({"width": -5}),
                "width must be a whole number from 0 to 4294967295",
            ),
            (
                json!({"width": 1000.0}),
                "width must be a whole number from 0 to 4294967295",
            ),
            (
                json!({"width": 5000000000u64}),
                "width must be a whole number from 0 to 4294967295",
            ),
            (
                json!({"width": "wide"}),
                "width must be a whole number from 0 to 4294967295",
            ),
            (
                json!({"width": null}),
                "width must be a whole number from 0 to 4294967295",
            ),
            (
                json!({"width": 1, "small": 256}),
                "small must be a whole number from 0 to 255",
            ),
            (
                json!({"width": 1, "scale": "big"}),
                "scale must be a number",
            ),
            (json!({"width": 1, "flag": 1}), "flag must be true or false"),
            (json!({"width": 1, "name": 5}), "name must be text"),
            (json!({"width": 1, "list": 5}), "list must be a list"),
            (
                json!({"width": 1, "list": [1, -1]}),
                "list holds a value that must be a whole number from 0 to 4294967295",
            ),
            (
                json!({"width": 1, "at": [1, 2, 3]}),
                "at has the wrong number of entries",
            ),
        ] {
            let e = err(v.clone());
            assert_eq!(e, format!("step 1 (probe) is not valid: {want}"), "{v}");
            assert!(
                !e.contains("expected") && !e.contains("u32") && !e.contains('`'),
                "{e}"
            );
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "op", rename_all = "snake_case")]
    #[allow(dead_code)]
    enum Tagged {
        Size { width: u32 },
    }

    #[test]
    fn the_tag_key_of_an_enum_is_never_blamed() {
        let e = from_value::<Tagged>(json!({"op": "size", "width": -1}), "step 1", &["op"])
            .unwrap_err();
        assert_eq!(
            e,
            "step 1 is not valid: width must be a whole number from 0 to 4294967295"
        );
        assert!(from_value::<Tagged>(json!({"op": "size", "width": 1}), "step 1", &["op"]).is_ok());
    }

    #[test]
    fn the_first_bad_option_is_named_even_when_more_than_one_is_wrong() {
        // Options are tried in name order, so `small` comes before `width`.
        let e = err(json!({"width": "x", "small": -1}));
        assert!(e.contains("small must be"), "{e}");
    }

    #[test]
    fn names_the_caller_wrote_or_left_out_are_kept() {
        assert!(err(json!({"widht": 5})).contains("unknown field `widht`"));
        assert_eq!(
            err(json!({})),
            "step 1 (probe) is not valid: missing field `width`"
        );
    }

    #[test]
    fn something_that_is_not_an_object_is_one_sentence() {
        for v in [json!("text"), json!(5), json!(null)] {
            assert_eq!(
                err(v.clone()),
                "step 1 (probe) is not valid; send a JSON object",
                "{v}"
            );
        }
    }

    #[test]
    fn the_value_is_left_as_it_was_found() {
        let v = json!({"width": "x", "small": 1});
        let mut copy = v.clone();
        let _ = describe::<Probe>(
            &mut copy,
            "x",
            &[],
            "invalid type: string \"x\", expected u32",
        );
        assert_eq!(copy, v);
    }
}
