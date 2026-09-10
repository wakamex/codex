use codex_extension_api::TurnFailureContinuation;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CodexErrorInfo;
use pretty_assertions::assert_eq;

use super::ContextualUserFragment;
use super::TurnFailure;

#[test]
fn tool_summary_distinguishes_recorded_outputs_from_unknown_execution() {
    let history = vec![
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
        &history,
    );

    assert_eq!(
        failure.body(),
        "\nThe preceding model request failed. Continue the same logical turn using the existing thread history, including every prior message and tool item.\nFailure kind: CyberPolicy\nFailure message: policy stopped the request\nHandler attempt: 1 of 2\nCurrent-turn tool activity derived from that history:\n- exec_command [completed-call]: output recorded\n- send_email [outstanding-call]: no output recorded - execution may be incomplete or its result may be unknown\n- tool_search [search-call]: no output recorded - execution may be incomplete or its result may be unknown (reported status: completed)\nUser-configured failure instructions:\nFollow my custom instructions.\n"
    );
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
