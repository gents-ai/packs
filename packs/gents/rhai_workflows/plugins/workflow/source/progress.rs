//! The JobProgress frame every output carries (L2 section 4.11, root C6). `last_event_at` and
//! `revision` are the host's to fill.
use crate::host_api::{Host, clip, terminal};
use crate::validate::Meta;
use crate::wire::{
    ActiveChild, JobPin, JobProgress, JobState, PauseReason, Phase, PhaseState, ProgressBudget,
};
use serde::Serialize;
use serde_json::Value;

const TITLE_MAX: usize = 256;
const PROGRESS_MAX: usize = 16 * 1024;

/// What the output is, which fixes `state`, `pause_reason` and `question`.
pub enum Kind {
    Running,
    Completed,
    Failed,
    /// The script's pause kind, aliases included.
    Paused(String),
    /// The question put to the user.
    Awaiting(String),
}

/// `text` within 256 B; a longer text is cut and says by how much, as `what` truncated.
pub fn capped(text: &str, what: &str) -> String {
    if text.len() <= TITLE_MAX {
        return text.into();
    }
    let named = format!(" [{what} truncated from {} bytes]", text.len());
    format!("{}{named}", clip(text, TITLE_MAX - named.len()))
}

/// The closed `pause_reason` enum, after Grok's aliases `blocked` and `backoff`.
pub fn pause_reason(kind: &str) -> Option<PauseReason> {
    Some(match kind {
        "user" => PauseReason::User,
        "back_off" | "backoff" => PauseReason::BackOff,
        "no_progress" => PauseReason::NoProgress,
        "verification" | "blocked" => PauseReason::Verification,
        "infra" => PauseReason::Infra,
        "budget_exceeded" => PauseReason::BudgetExceeded,
        _ => return None,
    })
}

/// The leading items that fit in `room` bytes of JSON.
fn fit<T: Serialize>(items: Vec<T>, room: &mut usize) -> Vec<T> {
    let mut take = |item: &T| {
        let size = serde_json::to_string(item).expect("item serializes").len() + 1;
        let fits = size <= *room;
        *room -= if fits { size } else { 0 };
        fits
    };
    items.into_iter().take_while(|item| take(item)).collect()
}

/// The frame of this output. `adopted` is the step-0 tier line, which beats any `log()`.
pub fn build(meta: &Meta, pin: &JobPin, host: &Host, kind: Kind, adopted: Option<&str>) -> Value {
    let (state, pause_reason, question) = match kind {
        Kind::Running => (JobState::Running, None, None),
        Kind::Completed => (JobState::Completed, None, None),
        Kind::Failed => (JobState::Failed, None, None),
        Kind::Paused(kind) => (JobState::Paused, pause_reason(&kind), None),
        Kind::Awaiting(q) => (JobState::AwaitingUser, None, Some(capped(&q, "question"))),
    };
    let mut titles: Vec<String> = (meta.phases.iter())
        .map(|p| capped(&p.title, "phase"))
        .collect();
    let named = host.phases.iter().filter(|t| !titles.contains(t)).cloned();
    titles.extend(named.collect::<Vec<_>>());
    let current = (host.phase.as_ref()).and_then(|c| titles.iter().position(|t| t == c));
    let phases: Vec<Phase> = (titles.into_iter().enumerate())
        .map(|(i, title)| Phase {
            title,
            state: match current {
                Some(at) if i < at => PhaseState::Done,
                Some(at) if i == at => PhaseState::Active,
                _ => PhaseState::Pending,
            },
        })
        .collect();
    let (live, ended): (Vec<_>, Vec<_>) =
        host.children.iter().partition(|(_, c)| !terminal(&c.state));
    let active: Vec<ActiveChild> = (live.into_iter())
        .map(|(slot, c)| ActiveChild {
            slot: slot.clone(),
            title: capped(&c.title, "title"),
            session_id: c.session_id.clone(),
        })
        .collect();
    let last_event = adopted.or(host.last_event.as_deref());
    let mut progress = JobProgress {
        title: capped(&meta.description, "title"),
        state,
        pause_reason,
        phases: Vec::new(),
        active: Vec::new(),
        spawned: host.spent,
        budget: ProgressBudget {
            total: pin.agent_budget,
            spent: host.spent,
            remaining: pin.agent_budget.saturating_sub(host.spent),
        },
        last_event: capped(last_event.unwrap_or_default(), "event"),
        last_event_at: Value::Null,
        question,
        observed_terminal_children: ended.len() as u64,
        revision: 0,
    };
    // Past 16 KiB the phases go first, then the active list; the fixed fields always stay.
    let mut room = PROGRESS_MAX - serde_json::to_string(&progress).expect("progress").len();
    progress.active = fit(active, &mut room);
    progress.phases = fit(phases, &mut room);
    serde_json::to_value(progress).expect("progress serializes")
}
