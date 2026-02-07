CREATE TABLE task_logs (
    id            BIGSERIAL PRIMARY KEY,
    task_run_id   TEXT NOT NULL,
    timestamp_ms  BIGINT NOT NULL,
    level         TEXT NOT NULL,
    message       TEXT NOT NULL,
    metadata      JSONB,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_task_logs_run ON task_logs (task_run_id, timestamp_ms);
