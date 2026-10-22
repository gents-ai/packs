//! The call sequencer, slot ids and divergence check of a replay (L2 section 4.6).
use crate::errors::WORKFLOW_VALIDATION_FAILED;
use crate::fail;
use crate::wire::Child;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The host functions a script calls, by whether they take a call index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFn {
    Agent,
    Parallel,
    Budget,
    AwaitUser,
    WriteScratchFile,
    ReadScratchFile,
    Phase,
    Log,
    Fingerprint,
    JsonEncode,
    RenderTemplate,
}

/// Hands out call indexes; a replay starts a fresh one at 0.
#[derive(Debug, Default)]
pub struct Sequencer {
    next: u64,
}

impl Sequencer {
    /// The next index for a journaled call; the pure functions take none.
    pub fn call(&mut self, f: HostFn) -> Option<u64> {
        use HostFn::*;
        if matches!(f, Phase | Log | Fingerprint | JsonEncode | RenderTemplate) {
            return None;
        }
        self.next += 1;
        Some(self.next - 1)
    }
}

/// What a slot's hash covers. The target is part of it, so editing a target re-keys the slot.
#[derive(Debug, Clone, Serialize)]
pub struct SlotKey {
    pub target: Value,
    pub prompt: String,
    pub title: String,
    pub cwd: Option<String>,
    pub schema: Option<Value>,
}

/// Compact JSON with every object's keys sorted, whatever order `Value` keeps them in.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        Value::Object(o) => {
            let sorted: BTreeMap<_, _> = o.iter().collect();
            let fields: Vec<_> = sorted
                .into_iter()
                .map(|(k, v)| format!("{}:{}", Value::from(k.as_str()), canonical(v)))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        other => other.to_string(),
    }
}

/// `<c>[.<i>]:<h>`, `h` the first 16 hex digits of the key's sha256. At most 58 bytes.
pub fn slot_id(c: u64, item: Option<u64>, key: &SlotKey) -> String {
    let digest = Sha256::digest(canonical(&serde_json::to_value(key).unwrap()).as_bytes());
    let h: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    match item {
        Some(i) => format!("{c}.{i}:{h}"),
        None => format!("{c}:{h}"),
    }
}

/// The latest child of `slot`, or none yet. A child under the same `<c>[.<i>]` prefix with
/// another hash, or under the same call in the other shape, means the replay diverged: the step
/// fails and spawns nothing.
pub fn resolve<'a>(
    children: &'a BTreeMap<String, Child>,
    prefix: &str,
    slot: &str,
    name: &str,
) -> Result<Option<&'a Child>, Value> {
    let call = |p: &str| p.split('.').next().unwrap_or(p).to_string();
    let c = call(prefix);
    // The same call switching between agent (`<c>`) and parallel (`<c>.<i>`) is a divergence too.
    let moved = |head: &str| call(head) == c && head.contains('.') != prefix.contains('.');
    let diverged = children.keys().any(|k| {
        let head = k.split_once(':').map(|(p, _)| p);
        k != slot && head.is_some_and(|p| p == prefix || moved(p))
    });
    if diverged {
        return Err(fail(
            WORKFLOW_VALIDATION_FAILED,
            &format!("replay diverged at call {c}. Next: workflow run {name}"),
        ));
    }
    Ok(children.get(slot))
}

/// How many leading items of a panel of `n` are admitted: `t` terminal items plus `live_room`
/// (`ceiling.max_live` minus the chain's other live children), never past the panel.
pub fn window(n: usize, t: usize, live_room: u64) -> usize {
    n.min(t.saturating_add(usize::try_from(live_room).unwrap_or(usize::MAX)))
}
