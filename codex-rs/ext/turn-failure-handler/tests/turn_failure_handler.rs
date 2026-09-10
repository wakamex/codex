use std::sync::Arc;

use codex_extension_api::ConversationHistory;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::NoopTurnItemEmitter;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolCallSource;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolPayload;
use codex_extension_api::TurnFailureInput;
use codex_protocol::ThreadId;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::TruncationPolicy;
use codex_turn_failure_handler_extension::SET_TURN_FAILURE_HANDLER_TOOL_NAME;
use codex_turn_failure_handler_extension::install;
use codex_utils_absolute_path::test_support::PathExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

#[tokio::test]
async fn configured_prose_is_persistent_and_bounded_per_turn() -> anyhow::Result<()> {
    let tempdir = TempDir::new()?;
    let runtime = codex_state::StateRuntime::init(
        codex_state::SqliteConfig::new_for_testing(tempdir.path().abs()),
        "test-provider".to_string(),
    )
    .await?;
    let thread_id = ThreadId::from_string("11111111-1111-4111-8111-111111111111")
        .map_err(anyhow::Error::msg)?;
    let mut builder = ExtensionRegistryBuilder::<()>::new();
    install(&mut builder, runtime.clone());
    let registry = builder.build();
    let session_store = ExtensionData::new(thread_id.to_string());
    let thread_store = ExtensionData::new(thread_id.to_string());
    let tools = registry
        .tool_contributors()
        .iter()
        .flat_map(|contributor| contributor.tools(&session_store, &thread_store))
        .collect::<Vec<_>>();
    let set_tool = tool_by_name(&tools, SET_TURN_FAILURE_HANDLER_TOOL_NAME);
    let invocation = tool_call(json!({
        "instructions": "Email me a concise diagnosis, then delegate any approved follow-up exactly as I described.",
        "max_continuations": 2
    }));
    set_tool.handle(invocation).await?;

    assert_eq!(
        runtime.turn_failure_handlers().get(thread_id).await?,
        Some(codex_state::TurnFailureHandler {
            instructions: "Email me a concise diagnosis, then delegate any approved follow-up exactly as I described.".to_string(),
            max_continuations: 2,
        })
    );

    let turn_store = ExtensionData::new("turn-1");
    let first = continuation(&registry, &session_store, &thread_store, &turn_store).await;
    let second = continuation(&registry, &session_store, &thread_store, &turn_store).await;
    let exhausted = continuation(&registry, &session_store, &thread_store, &turn_store).await;
    assert_eq!(first.as_ref().map(|value| value.attempt), Some(1));
    assert_eq!(second.as_ref().map(|value| value.attempt), Some(2));
    assert_eq!(exhausted, None);

    let next_turn_store = ExtensionData::new("turn-2");
    assert_eq!(
        continuation(&registry, &session_store, &thread_store, &next_turn_store)
            .await
            .map(|value| value.attempt),
        Some(1)
    );

    Ok(())
}

async fn continuation(
    registry: &codex_extension_api::ExtensionRegistry<()>,
    session_store: &ExtensionData,
    thread_store: &ExtensionData,
    turn_store: &ExtensionData,
) -> Option<codex_extension_api::TurnFailureContinuation> {
    registry
        .turn_failure_continuation(TurnFailureInput {
            turn_id: turn_store.level_id(),
            error: CodexErrorInfo::CyberPolicy,
            message: "blocked",
            session_store,
            thread_store,
            turn_store,
        })
        .await
}

fn tool_by_name<'a>(
    tools: &'a [Arc<dyn for<'call> ToolExecutor<ToolCall<'call>>>],
    name: &str,
) -> &'a Arc<dyn for<'call> ToolExecutor<ToolCall<'call>>> {
    tools
        .iter()
        .find(|tool| tool.tool_name().name == name)
        .expect("requested tool should exist")
}

fn tool_call(arguments: serde_json::Value) -> ToolCall<'static> {
    ToolCall {
        turn_id: "turn-setup".to_string(),
        call_id: "set-handler".to_string(),
        tool_name: codex_extension_api::ToolName::plain(SET_TURN_FAILURE_HANDLER_TOOL_NAME),
        model: "gpt-test".to_string(),
        codex_turn_metadata: None,
        truncation_policy: TruncationPolicy::Bytes(1024),
        source: ToolCallSource::Direct,
        conversation_history: ConversationHistory::default(),
        turn_item_emitter: Arc::new(NoopTurnItemEmitter),
        environments: Vec::new(),
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
    }
}
