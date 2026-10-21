import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchAPI, ApiError } from "../client";
import { credentials, UNAUTHENTICATED_EVENT } from "@/lib/credentials";

function respond(status: number, body: unknown) {
  return vi.fn<(path: string, init?: RequestInit) => Promise<Response>>(
    async () => new Response(JSON.stringify(body), { status }),
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
