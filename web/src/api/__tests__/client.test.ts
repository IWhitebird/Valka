import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchAPI, fetchAPIResult, ApiError, isNotFound, pathSegment } from "../client";
import { tasksApi } from "../tasks";
import { credentials, UNAUTHENTICATED_EVENT } from "@/lib/credentials";
import { partialAnswers } from "@/lib/partial";

function respond(status: number, body: unknown, headers: Record<string, string> = {}) {
  return vi.fn<(path: string, init?: RequestInit) => Promise<Response>>(
    async () => new Response(JSON.stringify(body), { status, headers }),
  );
}

/** Headers of the n-th call to a mocked fetch. */
function sentHeaders(mock: ReturnType<typeof respond>, n = 0): Headers {
  return mock.mock.calls[n]?.[1]?.headers as Headers;
}

afterEach(() => {
  vi.unstubAllGlobals();
  credentials.setApiKey(null);
  credentials.setAdminToken(null);
  partialAnswers.clear();
});

describe("fetchAPI", () => {
  it("sends the stored API key as a bearer token", async () => {
    credentials.setApiKey("k1");
    const fetchMock = respond(200, { ok: true });
    vi.stubGlobal("fetch", fetchMock);
    await fetchAPI("/api/v1/tasks");
    expect(sentHeaders(fetchMock).get("Authorization")).toBe("Bearer k1");
  });

  it("keeps a caller's own Authorization header (admin calls)", async () => {
    credentials.setApiKey("k1");
    const fetchMock = respond(200, {});
    vi.stubGlobal("fetch", fetchMock);
    await fetchAPI("/api/v1/cluster/audit", { headers: { Authorization: "Bearer admin" } });
    expect(sentHeaders(fetchMock).get("Authorization")).toBe("Bearer admin");
  });

  it("announces a refused API key, but not a refused admin token", async () => {
    const seen = vi.fn();
    window.addEventListener(UNAUTHENTICATED_EVENT, seen);
    vi.stubGlobal("fetch", respond(401, { error: "missing or invalid API key", code: "UNAUTHENTICATED" }));
    await expect(fetchAPI("/api/v1/tasks")).rejects.toBeInstanceOf(ApiError);
    expect(seen).toHaveBeenCalledTimes(1);

    vi.stubGlobal("fetch", respond(401, { error: "bad or missing admin token", code: "UNAUTHORIZED" }));
    await expect(fetchAPI("/api/v1/cluster/audit")).rejects.toMatchObject({
      status: 401,
      code: "UNAUTHORIZED",
    });
    expect(seen).toHaveBeenCalledTimes(1);
    window.removeEventListener(UNAUTHENTICATED_EVENT, seen);
  });
});

describe("partial answers", () => {
  it("remembers a partial list until a later answer for it is complete", async () => {
    vi.stubGlobal("fetch", respond(200, [], { "X-Valka-Partial": "true; missing=node-b" }));
    await fetchAPI("/api/v1/workers");
    expect(partialAnswers.snapshot()).toEqual(["node-b"]);

    vi.stubGlobal("fetch", respond(200, [], { "X-Valka-Partial": "false" }));
    await fetchAPI("/api/v1/workers?x=1");
    expect(partialAnswers.snapshot()).toBeNull();
  });

  it("reports a partial mutation to its caller without raising the page banner", async () => {
    vi.stubGlobal(
      "fetch",
      respond(200, { deleted_count: 3 }, { "X-Valka-Partial": "true; missing=node-c" }),
    );
    const result = await fetchAPIResult<{ deleted_count: number }>("/api/v1/tasks", {
      method: "DELETE",
    });
    expect(result).toEqual({ data: { deleted_count: 3 }, missing: ["node-c"] });
    expect(partialAnswers.snapshot()).toBeNull();
    await expect(tasksApi.clearAll()).resolves.toEqual({ deleted_count: 3, missing: ["node-c"] });
  });
});

describe("errors", () => {
  it("calls only a 404 not found", async () => {
    vi.stubGlobal("fetch", respond(404, { error: "Task not found", code: "NOT_FOUND" }));
    const notFound = await fetchAPI("/api/v1/tasks/x").catch((e: unknown) => e);
    expect(isNotFound(notFound)).toBe(true);

    vi.stubGlobal("fetch", respond(503, { error: "owner unavailable", code: "UNAVAILABLE" }));
    const unavailable = await fetchAPI("/api/v1/tasks/x").catch((e: unknown) => e);
    expect(isNotFound(unavailable)).toBe(false);
    expect(unavailable).toMatchObject({ status: 503, message: "owner unavailable" });
  });
});

describe("task paths", () => {
  it("encodes ids so they cannot reach other routes", async () => {
    const fetchMock = respond(200, {});
    vi.stubGlobal("fetch", fetchMock);
    await tasksApi.get("../tasks");
    await tasksApi.delete("..");
    await tasksApi.getRunLogs("t/1", "r 1", { afterId: 40, limit: 10 });
    expect(fetchMock.mock.calls.map((c) => c[0])).toEqual([
      "/api/v1/tasks/..%2Ftasks",
      "/api/v1/tasks/%252E%252E",
      "/api/v1/tasks/t%2F1/runs/r%201/logs?after_id=40&limit=10",
    ]);
    // What a browser would request: no dot segment survives.
    expect(new URL("/api/v1/tasks/%252E%252E", "http://x").pathname).toBe("/api/v1/tasks/%252E%252E");
    expect(pathSegment(".")).toBe("%252E");
    expect(pathSegment("0192f3a4-aaaa-7bbb-8ccc-dddddddddddd")).toBe(
      "0192f3a4-aaaa-7bbb-8ccc-dddddddddddd",
    );
  });
});

describe("credentials", () => {
  it("keeps the API key across reloads and the admin token per tab", () => {
    credentials.setApiKey("k");
    credentials.setAdminToken("a");
    expect(localStorage.getItem("valka.apiKey")).toBe("k");
    expect(sessionStorage.getItem("valka.adminToken")).toBe("a");
    credentials.setApiKey(null);
    expect(credentials.apiKey()).toBeNull();
  });
});
