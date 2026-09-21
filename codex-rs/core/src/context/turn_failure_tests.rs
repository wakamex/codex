use codex_extension_api::TurnFailureContinuation;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_protocol::models::LocalShellAction;
use codex_protocol::models::LocalShellExecAction;
use codex_protocol::models::LocalShellStatus;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CodexErrorInfo;
use pretty_assertions::assert_eq;

use super::ContextualUserFragment;
use super::MAX_TOOL_ENTRIES;
use super::MAX_TOOL_SUMMARY_BYTES;
use super::TurnFailure;
use super::summarize_tool_activity;
use super::truncate_utf8;

#[test]
fn tool_summary_distinguishes_recorded_outputs_from_unknown_execution() {
    let history = [
        function_call("completed-call", "exec_command"),
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some("completed-call".to_string()),
            name: Some("exec_command".to_string()),
            namespace: None,
            output: FunctionCallOutputPayload::from_text("done".to_string()),
            internal_chat_message_metadata_passthrough: Some(metadata()),
        },
        function_call("outstanding-call", "send_email"),
        ResponseItem::ToolSearchCall {
            id: None,
            call_id: Some("search-call".to_string()),
            status: Some("completed".to_string()),
            execution: "client".to_string(),
            arguments: serde_json::json!({"query": "example"}),
            internal_chat_message_metadata_passthrough: Some(metadata()),
        },
        function_call_for_other_turn(),
    ];
    let failure = TurnFailure::from_history(
        CodexErrorInfo::CyberPolicy,
        "policy stopped the request",
        TurnFailureContinuation {
            instructions: "Follow my custom instructions.".to_string(),
            attempt: 1,
            max_continuations: 2,
        },
        "turn-1",
        history.iter(),
    );

    assert_eq!(
        failure.body(),
        "\nThe preceding model request failed. Continue the same logical turn using the existing thread history, including every prior message and tool item.\nFailure kind: CyberPolicy\nFailure message: policy stopped the request\nHandler attempt: 1 of 2\nCurrent-turn tool activity derived from that history:\n- exec_command [completed-call]: output recorded\n- send_email [outstanding-call]: no output recorded - execution may be incomplete or its result may be unknown\n- tool_search [search-call]: no output recorded - execution may be incomplete or its result may be unknown (reported status: completed)\nUser-configured failure instructions:\nFollow my custom instructions.\n"
    );
}

#[test]
fn tool_summary_covers_supported_response_tool_items() {
    let history = [
        ResponseItem::CustomToolCall {
            id: None,
            status: Some("completed".to_string()),
            call_id: "custom-call".to_string(),
            name: "apply_patch".to_string(),
            namespace: Some("workspace".to_string()),
            input: "patch".to_string(),
            internal_chat_message_metadata_passthrough: Some(metadata()),
        },
        ResponseItem::CustomToolCallOutput {
            id: None,
            call_id: "custom-call".to_string(),
            name: Some("apply_patch".to_string()),
            output: FunctionCallOutputPayload::from_text("done".to_string()),
            internal_chat_message_metadata_passthrough: Some(metadata()),
        },
        ResponseItem::ToolSearchCall {
            id: None,
            call_id: Some("search-call".to_string()),
            status: Some("completed".to_string()),
            execution: "client".to_string(),
            arguments: serde_json::json!({}),
            internal_chat_message_metadata_passthrough: Some(metadata()),
        },
        ResponseItem::ToolSearchOutput {
            id: None,
            call_id: Some("search-call".to_string()),
            status: "completed".to_string(),
            execution: "client".to_string(),
            tools: Vec::new(),
            internal_chat_message_metadata_passthrough: Some(metadata()),
        },
        ResponseItem::LocalShellCall {
            id: None,
            call_id: Some("shell-call".to_string()),
            status: LocalShellStatus::Completed,
            action: LocalShellAction::Exec(LocalShellExecAction {
                command: vec!["echo".to_string()],
                timeout_ms: None,
                working_directory: None,
                env: None,
                user: None,
            }),
            internal_chat_message_metadata_passthrough: Some(metadata()),
        },
        ResponseItem::WebSearchCall {
            id: None,
            status: Some("completed".to_string()),
            action: None,
            internal_chat_message_metadata_passthrough: Some(metadata()),
        },
        ResponseItem::ImageGenerationCall {
            id: None,
            status: "completed".to_string(),
            revised_prompt: None,
            result: "image".to_string(),
            internal_chat_message_metadata_passthrough: Some(metadata()),
        },
    ];

    assert_eq!(
        summarize_tool_activity("turn-1", history.iter()),
        "- workspace.apply_patch [custom-call]: output recorded\n- tool_search [search-call]: output recorded\n- local_shell [shell-call]: no output recorded - execution may be incomplete or its result may be unknown (reported status: Completed)\n- web_search: completed\n- image_generation: completed"
    );
}

#[test]
fn tool_summary_and_utf8_truncation_obey_hard_limits() {
    let short_history = (0..40)
        .map(|index| function_call(&format!("call-{index}"), "tool"))
        .collect::<Vec<_>>();
    let short_summary = summarize_tool_activity("turn-1", short_history.iter());
    assert!(short_summary.lines().count() <= MAX_TOOL_ENTRIES);
    assert!(short_summary.len() <= MAX_TOOL_SUMMARY_BYTES);
    assert!(short_summary.ends_with("- 9 additional tool calls omitted by the summary limit."));

    let long_history = (0..40)
        .map(|index| function_call(&format!("call-{index}"), &"x".repeat(100)))
        .collect::<Vec<_>>();
    let summary = summarize_tool_activity("turn-1", long_history.iter());

    assert!(summary.len() <= MAX_TOOL_SUMMARY_BYTES);
    assert!(summary.lines().count() <= MAX_TOOL_ENTRIES);
    let truncated = truncate_utf8(&"é".repeat(20), 17);
    assert!(truncated.len() <= 17);
    assert!(truncated.is_char_boundary(truncated.len()));
    assert!(truncated.ends_with("...[truncated]"));
    assert_eq!(truncate_utf8("long value", 5), "...[t");
}

fn function_call(call_id: &str, name: &str) -> ResponseItem {
    ResponseItem::FunctionCall {
        id: None,
        name: name.to_string(),
        namespace: None,
        arguments: "{}".to_string(),
        encrypted_function_args: None,
        call_id: call_id.to_string(),
        internal_chat_message_metadata_passthrough: Some(metadata()),
    }
}

fn function_call_for_other_turn() -> ResponseItem {
    let mut item = function_call("other-call", "other_tool");
    if let ResponseItem::FunctionCall {
        internal_chat_message_metadata_passthrough,
        ..
    } = &mut item
    {
        *internal_chat_message_metadata_passthrough =
            Some(InternalChatMessageMetadataPassthrough {
                turn_id: Some("turn-2".to_string()),
                ..Default::default()
            });
    }
    item
}

fn metadata() -> InternalChatMessageMetadataPassthrough {
    InternalChatMessageMetadataPassthrough {
        turn_id: Some("turn-1".to_string()),
        ..Default::default()
    }
}
