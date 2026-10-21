// ValkaWorker against scripted fake nodes: reconnect backoff, per-run state, and a drain
// that survives a node restart.
import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { createServer, ServerError, Status } from "nice-grpc";
import { ValkaWorker } from "../dist/index.js";

// The generated service definition is internal to the package: bundle it for the fake nodes.
const out = await build({
  entryPoints: [fileURLToPath(new URL("../src/generated/valka/v1/worker.ts", import.meta.url))],
  bundle: true,
  format: "esm",
  platform: "node",
  write: false,
  logLevel: "silent",
});
const { WorkerServiceDefinition, WorkerResponse } = await import(
  `data:text/javascript;base64,${Buffer.from(out.outputFiles[0].text).toString("base64")}`
);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function until(what, cond, ms = 5_000) {
  const deadline = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting: ${what}`);
    await sleep(10);
  }
}

/** Silence the worker's console output; returns what it warned. */
function quiet() {
  const saved = { log: console.log, warn: console.warn };
  const warnings = [];
  console.log = () => {};
  console.warn = (msg) => warnings.push(String(msg));
  return { warnings, restore: () => Object.assign(console, saved) };
}

/** A node that records what workers send it (into `log`) and answers from a script. */
async function node(name, log, { onHello = [], leaveOnDrain = false, reject = false } = {}) {
  const server = createServer();
  server.add(WorkerServiceDefinition, {
    async *session(requests) {
      if (reject) throw new ServerError(Status.UNAUTHENTICATED, "bad key");
      for await (const req of requests) {
        log.push([name, req]);
        if (req.hello) {
          for (const r of onHello) yield r;
        } else if (req.shutdown && leaveOnDrain) {
          yield WorkerResponse.fromPartial({ serverShutdown: { reason: "restart" } });
          return;
        } else if (req.taskResult) {
          const { taskId, taskRunId } = req.taskResult;
          yield WorkerResponse.fromPartial({ resultAck: { taskId, taskRunId, status: 1 } });
        }
      }
    },
    async checkpoint() {
      return {};
    },
  });
  const port = await server.listen("127.0.0.1:0");
  return { server, addr: `127.0.0.1:${port}` };
}

test("reconnects back off while a node rejects the session, and a shutdown ends the wait", async () => {
  const out = quiet();
  const rejecting = await node("r", [], { reject: true });
  try {
    const worker = ValkaWorker.builder()
      .serverAddr(rejecting.addr)
      .queues(["q"])
      .handler(async () => ({}))
      .build();
    const run = worker.run();
    const delays = () =>
      out.warnings.map((w) => /Reconnecting in (\d+)ms/.exec(w)).filter(Boolean).map((m) => +m[1]);
    await until("three reconnects", () => delays().length >= 3);
    worker.shutdown();
    await run;
    const [d0, d1, d2] = delays();
    assert.ok(d0 < d1 && d1 < d2 && d2 >= 3 * d0, `the delay grows: ${delays()}`);
  } finally {
    out.restore();
    rejecting.server.forceShutdown();
  }
});

test("the ack of an older run leaves the newer run of the same task alone", () => {
  const worker = ValkaWorker.builder().queues(["q"]).handler(async () => ({})).build();
  worker.running.set("t", "r2");
  worker.unacked.set("r1", { taskId: "t", taskRunId: "r1" });
  worker.onResultAck({ taskId: "t", taskRunId: "r1", status: 2, message: "" }); // STALE
  assert.equal(worker.unacked.size, 0);
  assert.equal(worker.running.get("t"), "r2");
  worker.onResultAck({ taskId: "t", taskRunId: "r2", status: 1, message: "" }); // APPLIED
  assert.equal(worker.running.size, 0);
});

test("a drain survives a node restart: the next node hears it and gets the result", async () => {
  const out = quiet();
  const log = [];
  const assignment = WorkerResponse.fromPartial({
    taskAssignment: { taskId: "t1", taskRunId: "r1", attemptNumber: 1 },
  });
  const a = await node("a", log, { onHello: [assignment], leaveOnDrain: true });
  const b = await node("b", log);
  try {
    let release;
    const gate = new Promise((r) => (release = r));
    let started;
    const startedP = new Promise((r) => (started = r));
    const worker = ValkaWorker.builder()
      .serverAddrs([a.addr, b.addr])
      .queues(["q"])
      .handler(async () => {
        started();
        await gate;
        return { ok: true };
      })
      .build();
    let finished = false;
    const run = worker.run().then(() => (finished = true));
    await startedP;
    worker.shutdown();

    const onB = () => log.filter(([n]) => n === "b").map(([, r]) => r);
    await until("node b hears the hello and the drain", () => onB().length >= 2);
    assert.deepEqual(
      onB()[0].hello.running.map((r) => r.taskRunId),
      ["r1"],
      "the hello lists the running task",
    );
    assert.ok(onB()[1].shutdown, "the drain goes on on the new node");
    await sleep(50);
    assert.equal(finished, false, "the drain waits for the running handler");

    release();
    await run;
    const results = log
      .filter(([, r]) => r.taskResult)
      .map(([n, r]) => `${n}:${r.taskResult.taskRunId}`);
    assert.deepEqual(results, ["b:r1"]);
  } finally {
    out.restore();
    a.server.forceShutdown();
    b.server.forceShutdown();
  }
});
