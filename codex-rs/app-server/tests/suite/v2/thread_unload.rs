use std::time::Duration;

use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_repeating_assistant;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadClosedNotification;
use codex_app_server_protocol::ThreadHistoryMode;
use codex_app_server_protocol::ThreadLoadedListParams;
use codex_app_server_protocol::ThreadLoadedListResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadStatus;
use codex_app_server_protocol::ThreadStatusChangedNotification;
use codex_app_server_protocol::ThreadUnloadParams;
use codex_app_server_protocol::ThreadUnloadResponse;
use codex_app_server_protocol::ThreadUnloadStatus;
use codex_app_server_protocol::ThreadUnsubscribeParams;
use codex_app_server_protocol::ThreadUnsubscribeResponse;
use codex_app_server_protocol::ThreadUnsubscribeStatus;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput;
use core_test_support::responses;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::start_streaming_sse_server;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn thread_unload_releases_writer_and_preserves_rollout() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .with_sandbox_mode("danger-full-access")
        .write(codex_home.path())?;

    let mut primary = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;
    let ThreadStartResponse { thread, .. } = primary
        .start_thread(ThreadStartParams {
            model: Some("mock-model".to_string()),
            history_mode: Some(ThreadHistoryMode::Legacy),
            ..Default::default()
        })
        .await?;
    let thread_id = thread.id;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread_id.clone(),
            input: vec![UserInput::Text {
                text: "preserve this history".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        }),
    )
    .await??;
    wait_for_thread_status(&mut primary, &thread_id, ThreadStatus::Idle).await?;

    let subscribed: ThreadUnloadResponse = primary
        .request(|request_id| ClientRequest::ThreadUnload {
            request_id,
            params: ThreadUnloadParams {
                thread_id: thread_id.clone(),
            },
        })
        .await?;
    assert_eq!(subscribed.status, ThreadUnloadStatus::HasSubscribers);

    let secondary_sqlite_home = TempDir::new()?;
    let secondary_sqlite_home_path = secondary_sqlite_home.path().to_string_lossy();
    let mut secondary = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .with_env_overrides(&[(
            "CODEX_SQLITE_HOME",
            Some(secondary_sqlite_home_path.as_ref()),
        )])
        .build_initialized()
        .await?;
    let blocked_resume_id = secondary
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread_id.clone(),
            ..Default::default()
        })
        .await?;
    let blocked_resume = timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_error_message(RequestId::Integer(blocked_resume_id)),
    )
    .await??;
    assert_eq!(blocked_resume.error.code, -32600);
    assert_eq!(
        blocked_resume.error.message,
        format!("thread {thread_id} already has an active writer")
    );

    let unsubscribe: ThreadUnsubscribeResponse = primary
        .request(|request_id| ClientRequest::ThreadUnsubscribe {
            request_id,
            params: ThreadUnsubscribeParams {
                thread_id: thread_id.clone(),
            },
        })
        .await?;
    assert_eq!(unsubscribe.status, ThreadUnsubscribeStatus::Unsubscribed);

    let unload: ThreadUnloadResponse = primary
        .request(|request_id| ClientRequest::ThreadUnload {
            request_id,
            params: ThreadUnloadParams {
                thread_id: thread_id.clone(),
            },
        })
        .await?;
    assert_eq!(unload.status, ThreadUnloadStatus::Unloaded);
    wait_for_thread_status(&mut primary, &thread_id, ThreadStatus::NotLoaded).await?;
    let closed = timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_notification_message("thread/closed"),
    )
    .await??;
    let closed: ThreadClosedNotification = serde_json::from_value(
        closed
            .params
            .ok_or_else(|| anyhow::anyhow!("thread/closed notification had no params"))?,
    )?;
    assert_eq!(closed.thread_id, thread_id);

    let ThreadLoadedListResponse { data, next_cursor } = primary
        .request(|request_id| ClientRequest::ThreadLoadedList {
            request_id,
            params: ThreadLoadedListParams::default(),
        })
        .await?;
    assert_eq!((data, next_cursor), (Vec::<String>::new(), None));

    let repeated: ThreadUnloadResponse = primary
        .request(|request_id| ClientRequest::ThreadUnload {
            request_id,
            params: ThreadUnloadParams {
                thread_id: thread_id.clone(),
            },
        })
        .await?;
    assert_eq!(repeated.status, ThreadUnloadStatus::NotLoaded);

    let resumed: ThreadResumeResponse = secondary
        .request(|request_id| ClientRequest::ThreadResume {
            request_id,
            params: ThreadResumeParams {
                thread_id: thread_id.clone(),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(resumed.thread.id, thread_id);
    assert_eq!(resumed.thread.turns.len(), 1);
    assert_eq!(resumed.thread.turns[0].status, TurnStatus::Completed);

    Ok(())
}

#[tokio::test]
async fn thread_unload_leaves_active_thread_running() -> Result<()> {
    let (finish_tx, finish_rx) = oneshot::channel();
    let (server, mut completions) = start_streaming_sse_server(vec![vec![
        StreamingSseChunk {
            gate: None,
            body: responses::sse(vec![responses::ev_response_created("resp-1")]),
        },
        StreamingSseChunk {
            gate: Some(finish_rx),
            body: responses::sse(vec![
                responses::ev_assistant_message("msg-1", "Done"),
                responses::ev_completed("resp-1"),
            ]),
        },
    ]])
    .await;
    let response_completed = completions.remove(0);
    let codex_home = TempDir::new()?;
    MockResponsesConfig::new(server.uri())
        .with_sandbox_mode("danger-full-access")
        .write(codex_home.path())?;

    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;
    let ThreadStartResponse { thread, .. } = mcp
        .start_thread(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_id = thread.id;
    let _: TurnStartResponse = mcp
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread_id.clone(),
                input: vec![UserInput::Text {
                    text: "wait for completion".to_string(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            },
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        server.wait_for_request_count(/*count*/ 1),
    )
    .await?;
    wait_for_thread_status(
        &mut mcp,
        &thread_id,
        ThreadStatus::Active {
            active_flags: Vec::new(),
        },
    )
    .await?;

    let _: ThreadUnsubscribeResponse = mcp
        .request(|request_id| ClientRequest::ThreadUnsubscribe {
            request_id,
            params: ThreadUnsubscribeParams {
                thread_id: thread_id.clone(),
            },
        })
        .await?;
    let unload: ThreadUnloadResponse = mcp
        .request(|request_id| ClientRequest::ThreadUnload {
            request_id,
            params: ThreadUnloadParams {
                thread_id: thread_id.clone(),
            },
        })
        .await?;
    assert_eq!(unload.status, ThreadUnloadStatus::Active);
    assert!(
        timeout(
            Duration::from_millis(250),
            mcp.read_stream_until_notification_message("thread/closed"),
        )
        .await
        .is_err()
    );

    finish_tx
        .send(())
        .map_err(|_| anyhow::anyhow!("failed to release response stream"))?;
    timeout(DEFAULT_READ_TIMEOUT, response_completed).await??;
    server.shutdown().await;

    Ok(())
}

async fn wait_for_thread_status(
    mcp: &mut TestAppServer,
    thread_id: &str,
    expected_status: ThreadStatus,
) -> Result<()> {
    loop {
        let notification = timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_notification_message("thread/status/changed"),
        )
        .await??;
        let status: ThreadStatusChangedNotification =
            serde_json::from_value(notification.params.ok_or_else(|| {
                anyhow::anyhow!("thread/status/changed notification had no params")
            })?)?;
        if status.thread_id == thread_id && status.status == expected_status {
            return Ok(());
        }
    }
}
