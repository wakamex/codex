//! Conversation-configurable handling for terminal turn failures.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::FunctionCallError;
use codex_extension_api::JsonToolOutput;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolContributor;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_extension_api::TurnFailureContinuation;
use codex_extension_api::TurnFailureContributor;
use codex_extension_api::TurnFailureInput;
use codex_protocol::ThreadId;
use codex_state::TurnFailureHandler;
use codex_state::TurnFailureHandlerStore;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

pub const GET_TURN_FAILURE_HANDLER_TOOL_NAME: &str = "get_turn_failure_handler";
pub const SET_TURN_FAILURE_HANDLER_TOOL_NAME: &str = "set_turn_failure_handler";
pub const CLEAR_TURN_FAILURE_HANDLER_TOOL_NAME: &str = "clear_turn_failure_handler";

const DEFAULT_MAX_CONTINUATIONS: u32 = 1;
const MAX_CONTINUATIONS: u32 = 8;
const MAX_INSTRUCTIONS_BYTES: usize = 6 * 1024;

#[derive(Clone)]
struct TurnFailureHandlerExtension {
    store: TurnFailureHandlerStore,
}

#[derive(Default)]
struct TurnFailureAttemptCounter(AtomicU32);

pub fn install<C>(
    builder: &mut ExtensionRegistryBuilder<C>,
    state_db: Arc<codex_state::StateRuntime>,
) where
    C: Send + Sync + 'static,
{
    let extension = Arc::new(TurnFailureHandlerExtension {
        store: state_db.turn_failure_handlers().clone(),
    });
    builder.tool_contributor(extension.clone());
    builder.turn_failure_contributor(extension);
}

impl TurnFailureContributor for TurnFailureHandlerExtension {
    fn continuation<'a>(
        &'a self,
        input: TurnFailureInput<'a>,
    ) -> ExtensionFuture<'a, Option<TurnFailureContinuation>> {
        Box::pin(async move {
            let Ok(thread_id) = ThreadId::from_string(input.thread_store.level_id()) else {
                return None;
            };
            let handler = match self.store.get(thread_id).await {
                Ok(handler) => handler,
                Err(err) => {
                    tracing::warn!("failed to read turn failure handler: {err}");
                    return None;
                }
            }?;
            let counter = input
                .turn_store
                .get_or_init::<TurnFailureAttemptCounter>(TurnFailureAttemptCounter::default);
            let attempt = counter.0.fetch_add(1, Ordering::Relaxed) + 1;
            if attempt > handler.max_continuations {
                return None;
            }
            Some(TurnFailureContinuation {
                instructions: handler.instructions,
                attempt,
                max_continuations: handler.max_continuations,
            })
        })
    }
}

impl ToolContributor for TurnFailureHandlerExtension {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn for<'call> ToolExecutor<ToolCall<'call>>>> {
        let Ok(thread_id) = ThreadId::from_string(thread_store.level_id()) else {
            return Vec::new();
        };
        [
            FailureHandlerToolKind::Get,
            FailureHandlerToolKind::Set,
            FailureHandlerToolKind::Clear,
        ]
        .into_iter()
        .map(|kind| {
            Arc::new(FailureHandlerTool {
                kind,
                thread_id,
                store: self.store.clone(),
            }) as Arc<dyn for<'call> ToolExecutor<ToolCall<'call>>>
        })
        .collect()
    }
}

#[derive(Clone, Copy)]
enum FailureHandlerToolKind {
    Get,
    Set,
    Clear,
}

struct FailureHandlerTool {
    kind: FailureHandlerToolKind,
    thread_id: ThreadId,
    store: TurnFailureHandlerStore,
}

#[derive(Deserialize)]
struct SetHandlerArgs {
    instructions: String,
    max_continuations: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HandlerResponse {
    handler: Option<TurnFailureHandlerResponse>,
    cleared: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnFailureHandlerResponse {
    instructions: String,
    max_continuations: u32,
}

impl<'call> ToolExecutor<ToolCall<'call>> for FailureHandlerTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(match self.kind {
            FailureHandlerToolKind::Get => GET_TURN_FAILURE_HANDLER_TOOL_NAME,
            FailureHandlerToolKind::Set => SET_TURN_FAILURE_HANDLER_TOOL_NAME,
            FailureHandlerToolKind::Clear => CLEAR_TURN_FAILURE_HANDLER_TOOL_NAME,
        })
    }

    fn spec(&self) -> ToolSpec {
        match self.kind {
            FailureHandlerToolKind::Get => get_tool_spec(),
            FailureHandlerToolKind::Set => set_tool_spec(),
            FailureHandlerToolKind::Clear => clear_tool_spec(),
        }
    }

    fn handle<'a>(
        &'a self,
        invocation: ToolCall<'call>,
    ) -> codex_extension_api::ToolExecutorFuture<'a>
    where
        'call: 'a,
    {
        Box::pin(async move {
            match self.kind {
                FailureHandlerToolKind::Get => self.get(invocation).await,
                FailureHandlerToolKind::Set => self.set(invocation).await,
                FailureHandlerToolKind::Clear => self.clear(invocation).await,
            }
        })
    }
}

impl FailureHandlerTool {
    async fn get(
        &self,
        invocation: ToolCall<'_>,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let _: serde_json::Value = parse_args(invocation.function_arguments()?)?;
        let handler = self
            .store
            .get(self.thread_id)
            .await
            .map_err(tool_error)?
            .map(TurnFailureHandlerResponse::from);
        json_output(HandlerResponse {
            handler,
            cleared: None,
        })
    }

    async fn set(
        &self,
        invocation: ToolCall<'_>,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let mut args: SetHandlerArgs = parse_args(invocation.function_arguments()?)?;
        args.instructions = args.instructions.trim().to_string();
        if args.instructions.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "instructions must not be empty".to_string(),
            ));
        }
        if args.instructions.len() > MAX_INSTRUCTIONS_BYTES {
            return Err(FunctionCallError::RespondToModel(format!(
                "instructions must be at most {MAX_INSTRUCTIONS_BYTES} bytes"
            )));
        }
        let max_continuations = args.max_continuations.unwrap_or(DEFAULT_MAX_CONTINUATIONS);
        if !(1..=MAX_CONTINUATIONS).contains(&max_continuations) {
            return Err(FunctionCallError::RespondToModel(format!(
                "max_continuations must be between 1 and {MAX_CONTINUATIONS}"
            )));
        }
        let handler = TurnFailureHandler {
            instructions: args.instructions,
            max_continuations,
        };
        self.store
            .set(self.thread_id, &handler)
            .await
            .map_err(tool_error)?;
        json_output(HandlerResponse {
            handler: Some(handler.into()),
            cleared: None,
        })
    }

    async fn clear(
        &self,
        invocation: ToolCall<'_>,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let _: serde_json::Value = parse_args(invocation.function_arguments()?)?;
        let cleared = self.store.clear(self.thread_id).await.map_err(tool_error)?;
        json_output(HandlerResponse {
            handler: None,
            cleared: Some(cleared),
        })
    }
}

impl From<TurnFailureHandler> for TurnFailureHandlerResponse {
    fn from(handler: TurnFailureHandler) -> Self {
        Self {
            instructions: handler.instructions,
            max_continuations: handler.max_continuations,
        }
    }
}

fn parse_args<T: serde::de::DeserializeOwned>(arguments: &str) -> Result<T, FunctionCallError> {
    serde_json::from_str(arguments).map_err(|err| {
        FunctionCallError::RespondToModel(format!("invalid turn failure handler arguments: {err}"))
    })
}

fn tool_error(err: anyhow::Error) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!("turn failure handler storage failed: {err}"))
}

fn json_output(value: HandlerResponse) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    let value = serde_json::to_value(value)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;
    Ok(Box::new(JsonToolOutput::new(value)))
}

fn get_tool_spec() -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: GET_TURN_FAILURE_HANDLER_TOOL_NAME.to_string(),
        description:
            "Get the user-configured instructions that run after a terminal model turn failure."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(BTreeMap::new(), Some(Vec::new()), Some(false.into())),
        output_schema: None,
    })
}

fn set_tool_spec() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "instructions".to_string(),
            JsonSchema::string(Some(
                "The user's prose instructions to follow after a terminal model turn failure. Preserve the user's wording and intent.".to_string(),
            )),
        ),
        (
            "max_continuations".to_string(),
            JsonSchema::integer(Some(
                "Maximum same-turn handler invocations after repeated failures. Defaults to 1 and may be 1 through 8.".to_string(),
            )),
        ),
    ]);
    ToolSpec::Function(ResponsesApiTool {
        name: SET_TURN_FAILURE_HANDLER_TOOL_NAME.to_string(),
        description: "Persist a failure handler for this thread only when the user explicitly asks to configure or change one. The handler is arbitrary prose, not a structured action list. After a terminal model request failure, Codex appends the failure and current-turn tool status to the existing history and continues the same logical turn with these instructions.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["instructions".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

fn clear_tool_spec() -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: CLEAR_TURN_FAILURE_HANDLER_TOOL_NAME.to_string(),
        description:
            "Clear this thread's persisted turn failure handler only when the user explicitly asks."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(BTreeMap::new(), Some(Vec::new()), Some(false.into())),
        output_schema: None,
    })
}
