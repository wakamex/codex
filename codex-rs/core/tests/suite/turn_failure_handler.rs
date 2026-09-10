use std::sync::Arc;

use anyhow::Result;
use codex_core::config::Config;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::TurnFailureContinuation;
use codex_extension_api::TurnFailureContributor;
use codex_extension_api::TurnFailureInput;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use serde_json::Value;
use serde_json::json;
use wiremock::ResponseTemplate;

struct ConfiguredFailureHandler;

impl TurnFailureContributor for ConfiguredFailureHandler {
    fn continuation<'a>(
        &'a self,
        input: TurnFailureInput<'a>,
    ) -> ExtensionFuture<'a, Option<TurnFailureContinuation>> {
        Box::pin(async move {
            let attempt = input
                .turn_store
                .insert_if(HandlerUsed, |current| current.is_none());
            attempt.then(|| TurnFailureContinuation {
                instructions: "Inspect the failure and finish the requested work without repeating completed side effects."
                    .to_string(),
                attempt: 1,
                max_continuations: 1,
            })
        })
    }
}

struct HandlerUsed;

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

    let mut extensions = ExtensionRegistryBuilder::<Config>::new();
    extensions.turn_failure_contributor(Arc::new(ConfiguredFailureHandler));
    let mut builder = test_codex().with_extensions(Arc::new(extensions.build()));
    let test = builder.build(&server).await?;

    test.submit_turn("perform the task once").await?;

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

    Ok(())
}
