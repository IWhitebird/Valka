CREATE TABLE task_signals (
    id               TEXT PRIMARY KEY,
    task_id          TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    signal_name      TEXT NOT NULL,
    payload          JSONB,
    status           TEXT NOT NULL DEFAULT 'PENDING',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    delivered_at     TIMESTAMPTZ,
    acknowledged_at  TIMESTAMPTZ
);

CREATE INDEX idx_task_signals_pending ON task_signals (task_id, created_at)
    WHERE status = 'PENDING';
CREATE INDEX idx_task_signals_task ON task_signals (task_id, created_at);
