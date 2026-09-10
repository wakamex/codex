use std::collections::HashSet;

use codex_extension_api::TurnFailureContinuation;
use codex_protocol::models::ContentItemKind;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CodexErrorInfo;

use super::ContextualUserFragment;

const MAX_ERROR_MESSAGE_BYTES: usize = 1024;
const MAX_TOOL_SUMMARY_BYTES: usize = 2048;
const MAX_TOOL_ENTRIES: usize = 32;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct TurnFailure {
    error: CodexErrorInfo,
    message: String,
    continuation: TurnFailureContinuation,
    tool_summary: String,
}

impl TurnFailure {
    pub(crate) fn from_history(
        error: CodexErrorInfo,
        message: &str,
        continuation: TurnFailureContinuation,
        turn_id: &str,
        history: &[ResponseItem],
    ) -> Self {
        Self {
            error,
            message: truncate_utf8(message, MAX_ERROR_MESSAGE_BYTES),
            continuation,
            tool_summary: summarize_tool_activity(turn_id, history),
        }
    }
}

impl ContextualUserFragment for TurnFailure {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("generic.turn_failure_handler".to_string())
    }

    fn role(&self) -> &'static str {
        "user"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<turn_failure_handler>", "</turn_failure_handler>")
    }

    fn body(&self) -> String {
        format!(
            "\nThe preceding model request failed. Continue the same logical turn using the existing thread history, including every prior message and tool item.\nFailure kind: {:?}\nFailure message: {}\nHandler attempt: {} of {}\nCurrent-turn tool activity derived from that history:\n{}\nUser-configured failure instructions:\n{}\n",
            self.error,
            self.message,
            self.continuation.attempt,
            self.continuation.max_continuations,
            self.tool_summary,
            self.continuation.instructions,
        )
    }
}

fn summarize_tool_activity(turn_id: &str, history: &[ResponseItem]) -> String {
    let tool_call_count = history
        .iter()
        .filter(|item| item.turn_id() == Some(turn_id))
        .filter(|item| is_tool_call(item))
        .count();
    let displayed_entry_limit = if tool_call_count > MAX_TOOL_ENTRIES {
        MAX_TOOL_ENTRIES - 1
    } else {
        MAX_TOOL_ENTRIES
    };
    let completed_function_calls = history
        .iter()
        .filter(|item| item.turn_id() == Some(turn_id))
        .filter_map(|item| match item {
            ResponseItem::FunctionCallOutput {
                call_id: Some(call_id),
                ..
            }
            | ResponseItem::CustomToolCallOutput { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let completed_search_calls = history
        .iter()
        .filter(|item| item.turn_id() == Some(turn_id))
        .filter_map(|item| match item {
            ResponseItem::ToolSearchOutput {
                call_id: Some(call_id),
                ..
            } => Some(call_id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    let mut entries = Vec::new();
    for item in history
        .iter()
        .filter(|item| item.turn_id() == Some(turn_id))
    {
        let entry = match item {
            ResponseItem::FunctionCall {
                name,
                namespace,
                call_id,
                ..
            }
            | ResponseItem::CustomToolCall {
                name,
                namespace,
                call_id,
                ..
            } => {
                let name = namespace
                    .as_deref()
                    .map_or_else(|| name.clone(), |namespace| format!("{namespace}.{name}"));
                let state = if completed_function_calls.contains(call_id.as_str()) {
                    "output recorded"
                } else {
                    "no output recorded - execution may be incomplete or its result may be unknown"
                };
                Some(format!("- {name} [{call_id}]: {state}"))
            }
            ResponseItem::ToolSearchCall {
                call_id, status, ..
            } => {
                let call_id = call_id.as_deref().unwrap_or("unknown call id");
                let state = if completed_search_calls.contains(call_id) {
                    "output recorded".to_string()
                } else {
                    format!(
                        "no output recorded - execution may be incomplete or its result may be unknown (reported status: {})",
                        status.as_deref().unwrap_or("unknown")
                    )
                };
                Some(format!("- tool_search [{call_id}]: {state}"))
            }
            ResponseItem::LocalShellCall {
                call_id, status, ..
            } => {
                let call_id = call_id.as_deref().unwrap_or("unknown call id");
                let state = if completed_function_calls.contains(call_id) {
                    format!("output recorded (reported status: {status:?})")
                } else {
                    format!(
                        "no output recorded - execution may be incomplete or its result may be unknown (reported status: {status:?})"
                    )
                };
                Some(format!("- local_shell [{call_id}]: {state}"))
            }
            ResponseItem::WebSearchCall { status, .. } => Some(format!(
                "- web_search: {}",
                status.as_deref().unwrap_or("status unknown")
            )),
            ResponseItem::ImageGenerationCall { status, .. } => {
                Some(format!("- image_generation: {status}"))
            }
            _ => None,
        };
        if let Some(entry) = entry {
            entries.push(entry);
            if entries.len() == displayed_entry_limit {
                break;
            }
        }
    }

    if entries.is_empty() {
        return "- No tool calls were recorded in this turn before the failure.".to_string();
    }
    if tool_call_count > entries.len() {
        let omitted = tool_call_count.saturating_sub(entries.len());
        let omission = format!("- {omitted} additional tool calls omitted by the summary limit.");
        let prefix_limit = MAX_TOOL_SUMMARY_BYTES.saturating_sub(omission.len() + 1);
        let prefix = truncate_utf8(&entries.join("\n"), prefix_limit);
        return format!("{prefix}\n{omission}");
    }
    truncate_utf8(&entries.join("\n"), MAX_TOOL_SUMMARY_BYTES)
}

fn is_tool_call(item: &ResponseItem) -> bool {
    matches!(
        item,
        ResponseItem::FunctionCall { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
    )
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    const SUFFIX: &str = "...[truncated]";
    if max_bytes <= SUFFIX.len() {
        return SUFFIX[..max_bytes].to_string();
    }
    let mut end = max_bytes.saturating_sub(SUFFIX.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{SUFFIX}", &value[..end])
}

#[cfg(test)]
#[path = "turn_failure_tests.rs"]
mod tests;
