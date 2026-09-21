use codex_protocol::protocol::CodexErrorInfo;

use crate::ExtensionData;

/// Input supplied when a model request has failed and the host can still continue the turn.
pub struct TurnFailureInput<'a> {
    /// Identifier of the logical turn whose request failed.
    pub turn_id: &'a str,
    /// Structured error classification exposed by the host.
    pub error: CodexErrorInfo,
    /// Human-readable error returned by the failed request.
    pub message: &'a str,
    /// Extension data scoped to the session.
    pub session_store: &'a ExtensionData,
    /// Extension data scoped to the thread.
    pub thread_store: &'a ExtensionData,
    /// Extension data scoped to this logical turn.
    pub turn_store: &'a ExtensionData,
}

/// User-authored instructions selected by an extension for same-turn continuation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnFailureContinuation {
    /// Exact user-authored prose to follow when continuing the turn.
    pub instructions: String,
    /// One-based continuation attempt within this logical turn.
    pub attempt: u32,
    /// Maximum attempts configured for this handler.
    pub max_continuations: u32,
}
