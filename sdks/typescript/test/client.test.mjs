// ValkaClient against local HTTP servers: node failover, API keys, safe create retries.
import { test } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { ValkaClient } from "../dist/index.js";

function server(handler) {
  return new Promise((resolve) => {
    const s = http.createServer((req, res) => {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => handler(req, body ? JSON.parse(body) : undefined, res));
    });
    s.listen(0, "127.0.0.1", () => resolve(s));
  });
}
const url = (s) => `http://127.0.0.1:${s.address().port}`;

test("a create moves to the next node with the same key and the API key", async () => {
  const seen = [];
  const down = await server((req, body, res) => {
    seen.push(["down", body.idempotency_key, req.headers.authorization]);
    res.writeHead(503).end("node is down");
  });
  const up = await server((req, body, res) => {
    seen.push(["up", body.idempotency_key, req.headers.authorization]);
    res.writeHead(201, { "content-type": "application/json" });
    res.end(JSON.stringify({ id: "t1", status: "PENDING" }));
  });
  try {
    const client = new ValkaClient({ baseUrl: url(down), baseUrls: [url(up)], apiKey: "k1" });
    const task = await client.createTask({ queue_name: "q", task_name: "t" });
    assert.equal(task.id, "t1");
    assert.deepEqual(seen.map((s) => s[0]), ["down", "up"]);
    assert.equal(seen[0][1], seen[1][1]);
    assert.ok(seen[0][1].startsWith("auto-"));
    assert.ok(seen.every((s) => s[2] === "Bearer k1"));
  } finally {
    down.close();
    up.close();
  }
});

test("a cancel is sent once", async () => {
  let calls = 0;
  const s = await server((_req, _body, res) => {
    calls++;
    res.writeHead(503).end("node is down");
  });
  try {
    const client = new ValkaClient(url(s));
    await assert.rejects(client.cancelTask("t1"));
    assert.equal(calls, 1);
  } finally {
    s.close();
  }
});

test("a client error is not retried", async () => {
  let calls = 0;
  const s = await server((_req, _body, res) => {
    calls++;
    res.writeHead(404).end("Task not found");
  });
  try {
    const client = new ValkaClient(url(s));
    await assert.rejects(client.getTask("t1"), (err) => err.status === 404);
    assert.equal(calls, 1);
  } finally {
    s.close();
  }
});
