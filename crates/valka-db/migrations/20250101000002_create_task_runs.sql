CREATE TABLE task_runs (
    id               TEXT PRIMARY KEY,
    task_id          TEXT NOT NULL REFERENCES tasks(id),
    attempt_number   INT NOT NULL,
    worker_id        TEXT NOT NULL,
    assigned_node_id TEXT NOT NULL,
    status           TEXT NOT NULL DEFAULT 'RUNNING',
    output           JSONB,
    error_message    TEXT,
    lease_expires_at TIMESTAMPTZ NOT NULL,
    started_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at     TIMESTAMPTZ,
    last_heartbeat   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (task_id, attempt_number)
);

CREATE INDEX idx_task_runs_lease ON task_runs (lease_expires_at)
    WHERE status = 'RUNNING';

CREATE INDEX idx_task_runs_task ON task_runs (task_id);
