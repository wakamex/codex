use super::ResponsesStreamRequest;
use super::ResponsesStreamRetryState;
use super::capacity_retry_delay;
use super::format_capacity_delay;
use super::handle_response_stream_error;
use super::log_retry;
use crate::config::CapacityRetries;
use crate::realtime_history::RealtimeHistoryState;
use crate::session::step_context::StepContext;
use crate::session::tests::make_session_and_context;
use codex_http_client::RetryAfter;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing_test::internal::MockWriter;

#[tokio::test]
async fn sampling_retry_logs_stream_error_context() {
    let (_session, turn_context) = make_session_and_context().await;
    let buffer: &'static std::sync::Mutex<Vec<u8>> =
        Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(MockWriter::new(buffer))
        .finish();
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);

    log_retry(
        ResponsesStreamRequest::Sampling,
        &turn_context,
        &CodexErr::Stream("websocket closed by server before response.completed".to_string()),
        /*retries*/ 2,
        /*max_retries*/ 5,
        Duration::from_secs(1),
    );

    let logs = String::from_utf8(
        buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    )
    .expect("retry log should be valid utf-8");
    assert!(logs.contains("stream disconnected - retrying sampling request"));
    assert!(logs.contains(&format!("turn_id={}", turn_context.sub_id)));
    assert!(logs.contains("retries=2"));
    assert!(logs.contains("max_retries=5"));
    assert!(logs.contains(
        "sampling_error=stream disconnected before completion: websocket closed by server before response.completed"
    ));
}

/// Time spent reporting a retry must count toward the server's original deadline.
#[tokio::test]
#[expect(
    clippy::await_holding_invalid_type,
    reason = "test holds the event-delivery lock to delay notification while virtual time advances"
)]
async fn stream_retry_preserves_deadline_across_delayed_notification() {
    let (mut session, turn_context) = make_session_and_context().await;
    let step_context = StepContext::for_test(Arc::new(turn_context));
    session.realtime_history = Some(Mutex::new(RealtimeHistoryState::default()));
    let mut client_session = session.services.model_client.new_session();
    // The second retry emits a notification even when release builds hide the first.
    let mut retry_state = ResponsesStreamRetryState {
        retries: 1,
        ..Default::default()
    };

    tokio::time::pause();
    let advice = RetryAfter::from_delay(Duration::from_secs(10)).expect("retry deadline");
    tokio::time::advance(Duration::from_secs(4)).await;

    // Hold the event-delivery lock so reporting the error consumes part of the deadline.
    let history_guard = session.realtime_history.as_ref().unwrap().lock().await;
    let retry = handle_response_stream_error(
        &mut retry_state,
        /*max_retries*/ 2,
        CodexErr::InternalServerError.with_retry_after(advice),
        &mut client_session,
        &session,
        &step_context,
        ResponsesStreamRequest::Sampling,
    );
    tokio::pin!(retry);
    assert!(futures::poll!(&mut retry).is_pending());
    tokio::time::advance(Duration::from_secs(4)).await;
    drop(history_guard);

    assert!(futures::poll!(&mut retry).is_pending());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(futures::poll!(&mut retry).is_pending());
    retry.await.expect("retry should be allowed");
    // Tokio rounds timer deadlines up to the next millisecond.
    let resumed_at = Instant::now();
    assert!(
        (advice.deadline()..=advice.deadline() + Duration::from_millis(1)).contains(&resumed_at),
        "retry resumed at {resumed_at:?}, expected {advice:?}"
    );
}

#[test]
fn capacity_retry_delay_follows_schedule_with_positive_jitter() {
    let seconds = |attempt, jitter| capacity_retry_delay(attempt, jitter).as_secs();
    assert_eq!(
        [
            seconds(1, 0.0),
            seconds(1, 1.0),
            seconds(2, 0.0),
            seconds(3, 0.5),
            seconds(9, 0.0),
            seconds(1, 7.0),
        ],
        [30, 60, 120, 450, 300, 60]
    );
}

#[test]
fn capacity_delay_uses_minutes_past_one_minute() {
    assert_eq!(
        [
            format_capacity_delay(Duration::from_secs(45)),
            format_capacity_delay(Duration::from_secs(150)),
        ],
        ["45s".to_string(), "2m 30s".to_string()]
    );
}

async fn capacity_retry_fixture(
    capacity_retries: CapacityRetries,
) -> (
    crate::session::session::Session,
    Arc<StepContext>,
    crate::client::ModelClientSession,
) {
    let (session, mut turn_context) = make_session_and_context().await;
    let mut config = (*turn_context.config).clone();
    config.capacity_retries = capacity_retries;
    turn_context.config = Arc::new(config);
    let step_context = StepContext::for_test(Arc::new(turn_context));
    let client_session = session.services.model_client.new_session();
    (session, step_context, client_session)
}

/// Capacity errors without advice wait at least the base schedule on their own budget,
/// then fall through to the stream retries, which end the turn here because none remain.
#[tokio::test(start_paused = true)]
async fn capacity_retries_wait_on_their_own_budget_until_the_limit() {
    let (session, step_context, mut client_session) =
        capacity_retry_fixture(CapacityRetries::Limited(2)).await;
    let mut retry_state = ResponsesStreamRetryState::default();

    for minimum in [Duration::from_secs(30), Duration::from_secs(120)] {
        let started = Instant::now();
        handle_response_stream_error(
            &mut retry_state,
            /*max_retries*/ 0,
            CodexErr::ServerOverloaded,
            &mut client_session,
            &session,
            &step_context,
            ResponsesStreamRequest::Sampling,
        )
        .await
        .expect("capacity retry should be allowed");
        let waited = started.elapsed();
        assert!(
            (minimum..=minimum * 2).contains(&waited),
            "waited {waited:?}, expected {minimum:?} plus up to 100% jitter"
        );
    }

    let err = handle_response_stream_error(
        &mut retry_state,
        /*max_retries*/ 0,
        CodexErr::ServerOverloaded,
        &mut client_session,
        &session,
        &step_context,
        ResponsesStreamRequest::Sampling,
    )
    .await
    .expect_err("with no stream retries left, the turn should end");
    assert!(matches!(err.details(), CodexErrorDetails::ServerOverloaded));
    assert_eq!((retry_state.capacity_retries, retry_state.retries), (2, 0));
}

#[tokio::test(start_paused = true)]
async fn unlimited_capacity_retries_keep_waiting() {
    let (session, step_context, mut client_session) = capacity_retry_fixture(
        CapacityRetries::Unlimited(codex_config::config_toml::UnlimitedCapacityRetries::Unlimited),
    )
    .await;
    let mut retry_state = ResponsesStreamRetryState::default();

    for _ in 0..10 {
        handle_response_stream_error(
            &mut retry_state,
            /*max_retries*/ 0,
            CodexErr::new(CodexErrorDetails::PolicyCapacityUnavailable),
            &mut client_session,
            &session,
            &step_context,
            ResponsesStreamRequest::RemoteCompactionV2,
        )
        .await
        .expect("unlimited capacity retries should continue");
    }
    assert_eq!(retry_state.capacity_retries, 10);
}
