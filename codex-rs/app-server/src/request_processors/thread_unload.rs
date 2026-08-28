use super::*;

impl ThreadRequestProcessor {
    pub(crate) async fn thread_unload(
        &self,
        params: ThreadUnloadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_unload_response_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "unload eligibility must be serialized against listener attachment"
    )]
    async fn thread_unload_response_inner(
        &self,
        params: ThreadUnloadParams,
    ) -> Result<ThreadUnloadResponse, JSONRPCErrorError> {
        let thread_id = ThreadId::from_string(&params.thread_id)
            .map_err(|err| invalid_request(format!("invalid thread id: {err}")))?;

        let mut pending_thread_unloads = self.pending_thread_unloads.lock().await;
        if pending_thread_unloads.contains(&thread_id) {
            return Ok(ThreadUnloadResponse {
                status: ThreadUnloadStatus::Unloading,
            });
        }

        let thread = match self.thread_manager.get_thread(thread_id).await {
            Ok(thread) => thread,
            Err(_) => {
                drop(pending_thread_unloads);
                self.finalize_thread_teardown(thread_id).await;
                return Ok(ThreadUnloadResponse {
                    status: ThreadUnloadStatus::NotLoaded,
                });
            }
        };

        if !self
            .thread_state_manager
            .subscribed_connection_ids(thread_id)
            .await
            .is_empty()
        {
            return Ok(ThreadUnloadResponse {
                status: ThreadUnloadStatus::HasSubscribers,
            });
        }

        let thread_id_string = thread_id.to_string();
        let watched_status = self
            .thread_watch_manager
            .loaded_statuses_for_threads([thread_id_string.clone()])
            .await
            .remove(&thread_id_string);
        if matches!(watched_status, Some(ThreadStatus::Active { .. }))
            || matches!(thread.agent_status().await, AgentStatus::Running)
        {
            return Ok(ThreadUnloadResponse {
                status: ThreadUnloadStatus::Active,
            });
        }

        pending_thread_unloads.insert(thread_id);
        drop(pending_thread_unloads);

        let status = match unload_thread(
            Arc::clone(&self.thread_manager),
            Arc::clone(&self.outgoing),
            Arc::clone(&self.pending_thread_unloads),
            self.thread_state_manager.clone(),
            self.thread_watch_manager.clone(),
            thread_id,
            thread,
        )
        .await
        {
            ThreadUnloadResult::Complete => ThreadUnloadStatus::Unloaded,
            ThreadUnloadResult::Replaced => {
                return Err(internal_error(format!(
                    "thread {thread_id} was replaced while unloading"
                )));
            }
            ThreadUnloadResult::SubmitFailed => {
                return Err(internal_error(format!(
                    "failed to submit Shutdown to thread {thread_id}"
                )));
            }
            ThreadUnloadResult::TimedOut => {
                return Err(internal_error(format!(
                    "thread {thread_id} shutdown timed out"
                )));
            }
        };

        Ok(ThreadUnloadResponse { status })
    }
}
