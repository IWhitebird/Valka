CREATE TABLE dead_letter_queue (
    id            TEXT PRIMARY KEY,
    task_id       TEXT NOT NULL REFERENCES tasks(id),
    queue_name    TEXT NOT NULL,
    task_name     TEXT NOT NULL,
    input         JSONB,
    error_message TEXT,
    attempt_count INT NOT NULL,
    metadata      JSONB NOT NULL DEFAULT '{}',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_dead_letter_queue ON dead_letter_queue (queue_name, created_at);
