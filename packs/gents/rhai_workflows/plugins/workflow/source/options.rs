//! Agent options to a spawn target and fields (L2 section 4.8). Every refusal is a catchable
//! `<code>: <what>. Next: <one call>` error.
use crate::errors::{
    WORKFLOW_CAPABILITY_UNAVAILABLE, WORKFLOW_INVALID_INPUT, WORKFLOW_NOT_AVAILABLE,
};
use crate::host_api::clip;
use crate::sandbox::infrastructure_error;
use rhai::{EvalAltResult, ImmutableString, Map};
use std::path::{Component, Path};

const MODES: [&str; 4] = ["read-only", "read-write", "execute", "all"];
const NEXT: &str = "workflow help agent";
const TITLE_MAX: usize = 256;

/// What `agent()` takes from its options map.
#[derive(Debug, PartialEq)]
pub struct Mapped {
    pub target: String,
    /// `label`, within 256 B with any truncation named.
    pub title: Option<String>,
    /// `phase`, for the progress grouping.
    pub phase: Option<String>,
    /// A relative subpath of the caller's cwd.
    pub cwd: Option<String>,
    /// An event for the run's log: `agent_type` overrode `capability_mode`.
    pub note: Option<String>,
}

type Refusal = Box<EvalAltResult>;

fn invalid(what: &str) -> Refusal {
    infrastructure_error(WORKFLOW_INVALID_INPUT, what, NEXT)
}

fn text(opts: &Map, key: &str) -> Result<Option<String>, Refusal> {
    match opts.get(key) {
        None => Ok(None),
        Some(v) => match v.clone().try_cast::<ImmutableString>() {
            Some(s) => Ok(Some(s.to_string())),
            None => Err(invalid(&format!("{key} must be a string"))),
        },
    }
}

/// Everything but an absent key or `false` counts as asking for the option.
fn asked(opts: &Map, key: &str) -> bool {
    opts.get(key)
        .is_some_and(|v| !matches!(v.as_bool(), Ok(false)) && !v.is_unit())
}

fn title(label: &str) -> String {
    if label.len() <= TITLE_MAX {
        return label.into();
    }
    let named = format!(" [label truncated from {} bytes]", label.len());
    format!("{}{named}", clip(label, TITLE_MAX - named.len()))
}

fn relative(cwd: &str) -> Result<String, Refusal> {
    let path = Path::new(cwd);
    let inside = !cwd.is_empty()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    match inside {
        true => Ok(cwd.into()),
        false => Err(invalid(&format!(
            "cwd must be a relative subpath of the caller's cwd, without `..` (got `{cwd}`)"
        ))),
    }
}

/// Maps `opts` to a spawn. `targets` is `ceiling.targets`; an empty list, as when the input
/// carries none, allows no target.
pub fn map(opts: &Map, targets: &[String]) -> Result<Mapped, Refusal> {
    if asked(opts, "isolation_worktree") {
        return Err(infrastructure_error(
            WORKFLOW_NOT_AVAILABLE,
            "isolation_worktree is not supported",
            "use capability_mode read-write in the caller's tree, or split the work",
        ));
    }
    if asked(opts, "fork_context") {
        return Err(invalid("fork_context is for built-in workflows only"));
    }
    let allowed = targets.join(", ");
    let mode = text(opts, "capability_mode")?.unwrap_or_else(|| "read-only".into());
    if !MODES.contains(&mode.as_str()) {
        let what = format!(
            "capability_mode `{mode}` is unknown; use one of {}",
            MODES.join(", ")
        );
        return Err(invalid(&what));
    }
    let (mut target, mut note) = (format!("wf-{mode}"), None);
    if let Some(agent_type) = text(opts, "agent_type")? {
        note = Some(format!(
            "agent_type {agent_type} overrides capability_mode {mode}"
        ));
        target = agent_type;
    }
    let (model, effort) = (text(opts, "model")?, text(opts, "effort")?);
    if model.is_some() || effort.is_some() {
        let suffix = [model, effort]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("-");
        target = format!("{target}@{suffix}");
        if !targets.contains(&target) {
            return Err(invalid(&format!(
                "no target {target} here; allowed: {allowed}"
            )));
        }
    }
    if !targets.contains(&target) {
        return Err(infrastructure_error(
            WORKFLOW_CAPABILITY_UNAVAILABLE,
            &format!("target {target} is outside the ceiling; allowed: {allowed}"),
            NEXT,
        ));
    }
    Ok(Mapped {
        target,
        title: text(opts, "label")?.map(|l| title(&l)),
        phase: text(opts, "phase")?,
        cwd: text(opts, "cwd")?.map(|c| relative(&c)).transpose()?,
        note,
    })
}
