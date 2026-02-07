CREATE TABLE tasks (
    id               TEXT PRIMARY KEY,
    queue_name       TEXT NOT NULL,
    task_name        TEXT NOT NULL,
    partition_id     INT NOT NULL,
    status           TEXT NOT NULL DEFAULT 'PENDING',
    input            JSONB,
    priority         INT NOT NULL DEFAULT 0,
    max_retries      INT NOT NULL DEFAULT 3,
    attempt_count    INT NOT NULL DEFAULT 0,
    timeout_seconds  INT NOT NULL DEFAULT 300,
    idempotency_key  TEXT,
    metadata         JSONB NOT NULL DEFAULT '{}',
    scheduled_at     TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Primary dequeue index (hot path)
CREATE INDEX idx_tasks_dequeue ON tasks (queue_name, partition_id, created_at)
    WHERE status = 'PENDING';

-- Scheduler: expired leases
CREATE INDEX idx_tasks_running ON tasks (updated_at) WHERE status = 'RUNNING';

-- Retry engine
CREATE INDEX idx_tasks_retry ON tasks (scheduled_at) WHERE status = 'RETRY';

-- Idempotency
CREATE UNIQUE INDEX idx_tasks_idempotency ON tasks (idempotency_key)
    WHERE idempotency_key IS NOT NULL;
