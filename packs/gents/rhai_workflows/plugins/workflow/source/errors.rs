//! The engine's error vocabulary: root C9's ten Grok codes plus the two gents adds.

pub const WORKFLOW_INVALID_INPUT: &str = "workflow_invalid_input";
pub const WORKFLOW_NOT_AVAILABLE: &str = "workflow_not_available";
pub const WORKFLOW_LAUNCH_FAILED: &str = "workflow_launch_failed";
pub const WORKFLOW_RESOLVE_FAILED: &str = "workflow_resolve_failed";
pub const WORKFLOW_RESUME_UNKNOWN_RUN: &str = "workflow_resume_unknown_run";
pub const WORKFLOW_CONTROL_UNKNOWN_RUN: &str = "workflow_control_unknown_run";
pub const WORKFLOW_CONTROL_NOT_APPLICABLE: &str = "workflow_control_not_applicable";
pub const WORKFLOW_VALIDATION_FAILED: &str = "workflow_validation_failed";
pub const WORKFLOWS_DISABLED: &str = "workflows_disabled";
pub const WORKFLOW_DEPTH_EXCEEDED: &str = "workflow_depth_exceeded";
pub const WORKFLOW_POLICY_DENIED: &str = "workflow_policy_denied";
pub const WORKFLOW_CAPABILITY_UNAVAILABLE: &str = "workflow_capability_unavailable";

/// Every engine code, for tests and the `help errors` topic.
pub const CODES: [&str; 12] = [
    WORKFLOW_INVALID_INPUT,
    WORKFLOW_NOT_AVAILABLE,
    WORKFLOW_LAUNCH_FAILED,
    WORKFLOW_RESOLVE_FAILED,
    WORKFLOW_RESUME_UNKNOWN_RUN,
    WORKFLOW_CONTROL_UNKNOWN_RUN,
    WORKFLOW_CONTROL_NOT_APPLICABLE,
    WORKFLOW_VALIDATION_FAILED,
    WORKFLOWS_DISABLED,
    WORKFLOW_DEPTH_EXCEEDED,
    WORKFLOW_POLICY_DENIED,
    WORKFLOW_CAPABILITY_UNAVAILABLE,
];

/// `<code>: <what>. Next: <one call>` (L2 section 4.17).
pub fn message(code: &str, what: &str, next: &str) -> String {
    format!("{code}: {what}. Next: {next}")
}
