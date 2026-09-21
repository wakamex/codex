CREATE TABLE turn_failure_handlers (
    thread_id TEXT PRIMARY KEY NOT NULL,
    instructions TEXT NOT NULL,
    max_continuations INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
