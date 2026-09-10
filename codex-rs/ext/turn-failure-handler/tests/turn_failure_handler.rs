use std::sync::Arc;

use codex_extension_api::ConversationHistory;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::FunctionCallError;
use codex_extension_api::NoopTurnItemEmitter;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolCallSource;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolPayload;
use codex_extension_api::TurnFailureInput;
use codex_protocol::ThreadId;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::TruncationPolicy;
use codex_turn_failure_handler_extension::CLEAR_TURN_FAILURE_HANDLER_TOOL_NAME;
use codex_turn_failure_handler_extension::GET_TURN_FAILURE_HANDLER_TOOL_NAME;
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
    let other_thread_id = ThreadId::from_string("22222222-2222-4222-8222-222222222222")
        .map_err(anyhow::Error::msg)?;
    assert_eq!(
        runtime.turn_failure_handlers().get(other_thread_id).await?,
        None
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

    drop(tools);
    drop(registry);
    runtime.close().await;
    drop(runtime);
    let reopened = codex_state::StateRuntime::init(
        codex_state::SqliteConfig::new_for_testing(tempdir.path().abs()),
        "test-provider".to_string(),
    )
    .await?;
    assert_eq!(
        reopened.turn_failure_handlers().get(thread_id).await?,
        Some(codex_state::TurnFailureHandler {
            instructions: "Email me a concise diagnosis, then delegate any approved follow-up exactly as I described.".to_string(),
            max_continuations: 2,
        })
    );
    reopened.close().await;

    Ok(())
}

#[tokio::test]
async fn get_and_clear_tools_report_persisted_state() -> anyhow::Result<()> {
    let tempdir = TempDir::new()?;
    let runtime = codex_state::StateRuntime::init(
        codex_state::SqliteConfig::new_for_testing(tempdir.path().abs()),
        "test-provider".to_string(),
    )
    .await?;
    let thread_id = ThreadId::from_string("33333333-3333-4333-8333-333333333333")
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
    tool_by_name(&tools, SET_TURN_FAILURE_HANDLER_TOOL_NAME)
        .handle(tool_call(json!({"instructions": "do nothing"})))
        .await?;

    assert_eq!(
        output_json(
            tool_by_name(&tools, GET_TURN_FAILURE_HANDLER_TOOL_NAME)
                .handle(tool_call(json!({})))
                .await?
        )?,
        json!({
            "handler": {"instructions": "do nothing", "maxContinuations": 1},
            "cleared": null
        })
    );
    assert_eq!(
        output_json(
            tool_by_name(&tools, CLEAR_TURN_FAILURE_HANDLER_TOOL_NAME)
                .handle(tool_call(json!({})))
                .await?
        )?,
        json!({"handler": null, "cleared": true})
    );
    assert_eq!(
        output_json(
            tool_by_name(&tools, GET_TURN_FAILURE_HANDLER_TOOL_NAME)
                .handle(tool_call(json!({})))
                .await?
        )?,
        json!({"handler": null, "cleared": null})
    );

    drop(tools);
    drop(registry);
    runtime.close().await;

    Ok(())
}

#[tokio::test]
async fn set_tool_rejects_invalid_bounds() -> anyhow::Result<()> {
    let tempdir = TempDir::new()?;
    let runtime = codex_state::StateRuntime::init(
        codex_state::SqliteConfig::new_for_testing(tempdir.path().abs()),
        "test-provider".to_string(),
    )
    .await?;
    let thread_id = ThreadId::from_string("44444444-4444-4444-8444-444444444444")
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

    for (arguments, expected) in [
        (
            json!({"instructions": "   "}),
            "instructions must not be empty".to_string(),
        ),
        (
            json!({"instructions": "x".repeat(6 * 1024 + 1)}),
            "instructions must be at most 6144 bytes".to_string(),
        ),
        (
            json!({"instructions": "handle", "max_continuations": 0}),
            "max_continuations must be between 1 and 8".to_string(),
        ),
        (
            json!({"instructions": "handle", "max_continuations": 9}),
            "max_continuations must be between 1 and 8".to_string(),
        ),
    ] {
        let error = match set_tool.handle(tool_call(arguments)).await {
            Ok(_) => panic!("invalid handler configuration was accepted"),
            Err(error) => error,
        };
        assert_eq!(error, FunctionCallError::RespondToModel(expected));
    }

    assert_eq!(runtime.turn_failure_handlers().get(thread_id).await?, None);
    drop(tools);
    drop(registry);
    runtime.close().await;

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

fn output_json(
    output: Box<dyn codex_extension_api::ToolOutput>,
) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::from_str(&output.log_output())?)
}
