CREATE TABLE workers (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    node_id        TEXT NOT NULL,
    queues         JSONB NOT NULL DEFAULT '[]',
    concurrency    INT NOT NULL DEFAULT 1,
    status         TEXT NOT NULL DEFAULT 'ACTIVE',
    metadata       JSONB NOT NULL DEFAULT '{}',
    last_heartbeat TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    connected_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    disconnected_at TIMESTAMPTZ
);

CREATE INDEX idx_workers_status ON workers (status) WHERE status = 'ACTIVE';
