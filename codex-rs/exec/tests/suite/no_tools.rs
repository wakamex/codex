#![cfg(not(target_os = "windows"))]
#![allow(clippy::unwrap_used)]

use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex_exec::test_codex_exec;
use pretty_assertions::assert_eq;
use serde_json::Value;

fn model_visible_tool_count(body: &Value) -> usize {
    let direct_tools = body
        .get("tools")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let additional_tools = body
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("additional_tools"))
        .filter_map(|item| item.get("tools").and_then(Value::as_array))
        .map(Vec::len)
        .sum::<usize>();
    direct_tools + additional_tools
}

fn exec_sse_response(index: usize) -> String {
    responses::sse(vec![
        responses::ev_response_created(&format!("resp-no-tools-{index}")),
        responses::ev_assistant_message(&format!("msg-no-tools-{index}"), "done"),
        responses::ev_completed(&format!("resp-no-tools-{index}")),
    ])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_no_tools_sends_an_empty_tool_list() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let test = test_codex_exec();
    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(&server, exec_sse_response(1)).await;

    test.cmd_with_server(&server)
        .arg("--skip-git-repo-check")
        .arg("--no-tools")
        .arg("--json")
        .arg("answer without tools")
        .assert()
        .success();

    assert_eq!(
        model_visible_tool_count(&response_mock.single_request().body_json()),
        0
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_resume_no_tools_sends_an_empty_tool_list() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let test = test_codex_exec();
    let server = responses::start_mock_server().await;
    let response_mock =
        responses::mount_sse_sequence(&server, vec![exec_sse_response(1), exec_sse_response(2)])
            .await;

    test.cmd_with_server(&server)
        .arg("--skip-git-repo-check")
        .arg("seed session")
        .assert()
        .success();

    test.cmd_with_server(&server)
        .arg("resume")
        .arg("--last")
        .arg("--no-tools")
        .arg("--json")
        .arg("resume without tools")
        .assert()
        .success();

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);
    assert!(model_visible_tool_count(&requests[0].body_json()) > 0);
    assert_eq!(model_visible_tool_count(&requests[1].body_json()), 0);
    Ok(())
}
