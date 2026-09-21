use std::sync::Arc;

use chrono::Utc;
use codex_protocol::ThreadId;
use sqlx::SqlitePool;

#[derive(Clone, Debug, Eq, PartialEq)]
/// A thread's persisted user-authored turn failure policy.
pub struct TurnFailureHandler {
    /// Prose to supply to the continuation after a failed model request.
    pub instructions: String,
    /// Maximum continuation attempts in one logical turn.
    pub max_continuations: u32,
}

#[derive(Clone)]
/// Persistent access to thread-scoped turn failure handlers.
pub struct TurnFailureHandlerStore {
    pool: Arc<SqlitePool>,
}

impl TurnFailureHandlerStore {
    pub(crate) fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Reads the handler configured for `thread_id`.
    pub async fn get(&self, thread_id: ThreadId) -> anyhow::Result<Option<TurnFailureHandler>> {
        let row = sqlx::query_as::<_, (String, i64)>(
            "SELECT instructions, max_continuations FROM turn_failure_handlers WHERE thread_id = ?",
        )
        .bind(thread_id.to_string())
        .fetch_optional(self.pool.as_ref())
        .await?;

        row.map(|(instructions, max_continuations)| {
            Ok(TurnFailureHandler {
                instructions,
                max_continuations: u32::try_from(max_continuations)?,
            })
        })
        .transpose()
    }

    /// Creates or replaces the handler configured for `thread_id`.
    pub async fn set(
        &self,
        thread_id: ThreadId,
        handler: &TurnFailureHandler,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
INSERT INTO turn_failure_handlers (thread_id, instructions, max_continuations, updated_at)
VALUES (?, ?, ?, ?)
ON CONFLICT(thread_id) DO UPDATE SET
    instructions = excluded.instructions,
    max_continuations = excluded.max_continuations,
    updated_at = excluded.updated_at
            "#,
        )
        .bind(thread_id.to_string())
        .bind(&handler.instructions)
        .bind(i64::from(handler.max_continuations))
        .bind(Utc::now().timestamp())
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Clears the handler for `thread_id` and reports whether one existed.
    pub async fn clear(&self, thread_id: ThreadId) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM turn_failure_handlers WHERE thread_id = ?")
            .bind(thread_id.to_string())
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
