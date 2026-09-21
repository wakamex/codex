use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use anyhow::Result;
use codex_core::config::Config;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::TurnErrorInput;
use codex_extension_api::TurnFailureContinuation;
use codex_extension_api::TurnFailureContributor;
use codex_extension_api::TurnFailureInput;
use codex_extension_api::TurnLifecycleContributor;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::EventMsg;
use codex_protocol::turn_input::TurnInputRequest;
use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::test_support::PathExt;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use wiremock::ResponseTemplate;

struct ConfiguredFailureHandler {
    max_continuations: u32,
    terminal_errors: Arc<AtomicU32>,
}

#[derive(Default)]
struct HandlerAttempts(AtomicU32);

impl TurnFailureContributor for ConfiguredFailureHandler {
    fn continuation<'a>(
        &'a self,
        input: TurnFailureInput<'a>,
    ) -> ExtensionFuture<'a, Option<TurnFailureContinuation>> {
        Box::pin(async move {
            let attempt = input
                .turn_store
                .get_or_init::<HandlerAttempts>(HandlerAttempts::default)
                .0
                .fetch_add(1, Ordering::Relaxed)
                + 1;
            (attempt <= self.max_continuations).then(|| TurnFailureContinuation {
                instructions: "Inspect the failure and finish the requested work without repeating completed side effects.".to_string(),
                attempt,
                max_continuations: self.max_continuations,
            })
        })
    }
}

impl TurnLifecycleContributor for ConfiguredFailureHandler {
    fn on_turn_error<'a>(&'a self, _input: TurnErrorInput<'a>) -> ExtensionFuture<'a, ()> {
        self.terminal_errors.fetch_add(1, Ordering::Relaxed);
        Box::pin(std::future::ready(()))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_failure_handler_continues_same_turn_with_history_and_tool_summary() -> Result<()>
{
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let first_response = responses::sse_response(responses::sse(vec![
        responses::ev_response_created("resp-1"),
        responses::ev_function_call(
            "plan-call",
            "update_plan",
            &json!({
                "plan": [{"step": "inspect", "status": "completed"}]
            })
            .to_string(),
        ),
        responses::ev_completed("resp-1"),
    ]));
    let failure_response = ResponseTemplate::new(400).set_body_json(json!({
        "error": {
            "message": "blocked by cyber policy",
            "type": "invalid_request",
            "param": null,
            "code": "cyber_policy"
        }
    }));
    let final_response = responses::sse_response(responses::sse(vec![
        responses::ev_response_created("resp-3"),
        responses::ev_assistant_message("msg-3", "handled"),
        responses::ev_completed("resp-3"),
    ]));
    let requests = responses::mount_response_sequence(
        &server,
        vec![first_response, failure_response, final_response],
    )
    .await;

    let terminal_errors = Arc::new(AtomicU32::new(0));
    let handler = Arc::new(ConfiguredFailureHandler {
        max_continuations: 1,
        terminal_errors: Arc::clone(&terminal_errors),
    });
    let mut extensions = ExtensionRegistryBuilder::<Config>::new();
    extensions.turn_failure_contributor(handler.clone());
    extensions.turn_lifecycle_contributor(handler);
    let mut builder = test_codex().with_extensions(Arc::new(extensions.build()));
    let test = builder.build(&server).await?;

    test.codex
        .start_or_steer_turn(text_turn("perform the task once"))
        .await?;

    let mut started_turn_ids = Vec::new();
    let mut error_count = 0;
    let completed = loop {
        match wait_for_event(&test.codex, |_| true).await {
            EventMsg::TurnStarted(event) => started_turn_ids.push(event.turn_id),
            EventMsg::Error(_) => error_count += 1,
            EventMsg::TurnComplete(event) => break event,
            _ => {}
        }
    };

    let requests = requests.requests();
    assert_eq!(requests.len(), 3);
    let continuation_input = requests[2].input();
    let continuation_text = serde_json::to_string(&continuation_input)?;
    assert!(continuation_text.contains("perform the task once"));
    assert!(continuation_text.contains("plan-call"));
    assert!(continuation_text.contains("output recorded"));
    assert!(continuation_text.contains("blocked by cyber policy"));
    assert!(continuation_text.contains("without repeating completed side effects"));
    assert!(continuation_text.contains("turn_failure_handler"));
    assert!(continuation_input.iter().any(|item| {
        matches!(item, Value::Object(object) if object.get("type").and_then(Value::as_str) == Some("function_call_output"))
    }));
    assert_eq!(started_turn_ids, vec![completed.turn_id.clone()]);
    assert_eq!(completed.error, None);
    assert_eq!(completed.last_agent_message.as_deref(), Some("handled"));
    assert_eq!(error_count, 0);
    assert_eq!(terminal_errors.load(Ordering::Relaxed), 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_cyber_policy_failure_exhausts_handler_and_emits_one_terminal_error() -> Result<()>
{
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let requests = responses::mount_response_sequence(
        &server,
        vec![cyber_policy_response(), cyber_policy_response()],
    )
    .await;
    let terminal_errors = Arc::new(AtomicU32::new(0));
    let handler = Arc::new(ConfiguredFailureHandler {
        max_continuations: 1,
        terminal_errors: Arc::clone(&terminal_errors),
    });
    let mut extensions = ExtensionRegistryBuilder::<Config>::new();
    extensions.turn_failure_contributor(handler.clone());
    extensions.turn_lifecycle_contributor(handler);
    let mut builder = test_codex().with_extensions(Arc::new(extensions.build()));
    let test = builder.build(&server).await?;

    test.codex
        .start_or_steer_turn(text_turn("fail twice"))
        .await?;

    let mut started_turn_ids = Vec::new();
    let mut errors = Vec::new();
    let completed = loop {
        match wait_for_event(&test.codex, |_| true).await {
            EventMsg::TurnStarted(event) => started_turn_ids.push(event.turn_id),
            EventMsg::Error(error) => errors.push(error),
            EventMsg::TurnComplete(event) => break event,
            _ => {}
        }
    };

    assert_eq!(requests.requests().len(), 2);
    assert_eq!(started_turn_ids, vec![completed.turn_id.clone()]);
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].codex_error_info,
        Some(CodexErrorInfo::CyberPolicy)
    );
    assert_eq!(completed.error, Some(errors.remove(0)));
    assert_eq!(terminal_errors.load(Ordering::Relaxed), 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_backed_handler_recovers_from_sse_cyber_policy_failure() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let failure = responses::sse_response(responses::sse_failed(
        "resp-1",
        "cyber_policy",
        "blocked by streamed cyber policy",
    ));
    let success = responses::sse_response(responses::sse(vec![
        responses::ev_response_created("resp-2"),
        responses::ev_assistant_message("msg-2", "recovered"),
        responses::ev_completed("resp-2"),
    ]));
    let requests = responses::mount_response_sequence(&server, vec![failure, success]).await;
    let state_home = TempDir::new()?;
    let runtime = codex_state::StateRuntime::init(
        codex_state::SqliteConfig::new_for_testing(state_home.path().abs()),
        "test-provider".to_string(),
    )
    .await?;
    let mut extensions = ExtensionRegistryBuilder::<Config>::new();
    codex_turn_failure_handler_extension::install(&mut extensions, Arc::clone(&runtime));
    let mut builder = test_codex().with_extensions(Arc::new(extensions.build()));
    let test = builder.build(&server).await?;
    runtime
        .turn_failure_handlers()
        .set(
            test.session_configured.thread_id,
            &codex_state::TurnFailureHandler {
                instructions: "Use the persisted recovery instructions.".to_string(),
                max_continuations: 1,
            },
        )
        .await?;

    test.submit_text_turn("recover streamed failure").await?;

    let requests = requests.requests();
    assert_eq!(requests.len(), 2);
    let continuation = serde_json::to_string(&requests[1].input())?;
    assert!(continuation.contains("blocked by streamed cyber policy"));
    assert!(continuation.contains("Use the persisted recovery instructions."));

    drop(test);
    runtime.close().await;

    Ok(())
}

fn cyber_policy_response() -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({
        "error": {
            "message": "blocked by cyber policy",
            "type": "invalid_request",
            "param": null,
            "code": "cyber_policy"
        }
    }))
}

fn text_turn(text: &str) -> TurnInputRequest {
    TurnInputRequest::user_input(vec![UserInput::Text {
        text: text.to_string(),
        text_elements: Vec::new(),
    }])
}
