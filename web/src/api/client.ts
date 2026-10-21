import { credentials, UNAUTHENTICATED_EVENT } from "@/lib/credentials";
import { PARTIAL_HEADER, parsePartialHeader, partialAnswers } from "@/lib/partial";

export class ApiError extends Error {
  status: number;
  code: string | undefined;

  constructor(status: number, message: string, code?: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

/** True only for a 404 from the server: anything else is a failure, not a missing resource. */
export function isNotFound(error: unknown): boolean {
  return error instanceof ApiError && error.status === 404;
}

/** A readable message for any thrown value. */
export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * One URL path segment. Encoded, and never `.` or `..`: a URL parser would resolve those
 * (`/api/v1/tasks/..` is `/api/v1/`), so they are sent as a literal id that matches nothing.
 */
export function pathSegment(value: string): string {
  const encoded = encodeURIComponent(value);
  return encoded === "." || encoded === ".." ? encoded.replace(/\./g, "%252E") : encoded;
}

export interface ApiResult<T> {
  data: T;
  /** Nodes that did not answer a cluster-wide request; null when the answer is complete. */
  missing: string[] | null;
}

/**
 * Call the Valka REST API and report whether a cluster-wide answer is partial. The API key,
 * when one is set, goes as a bearer token; a caller that sets its own `Authorization` header
 * (the admin API) keeps it. Partial GET answers also feed the app-wide banner.
 */
export async function fetchAPIResult<T>(
  path: string,
  options: RequestInit = {},
): Promise<ApiResult<T>> {
  const headers = new Headers(options.headers);
  if (!headers.has("Content-Type")) headers.set("Content-Type", "application/json");
  const apiKey = credentials.apiKey();
  if (apiKey && !headers.has("Authorization")) headers.set("Authorization", `Bearer ${apiKey}`);

  const response = await fetch(path, { ...options, headers });

  if (!response.ok) {
    let message = `Request failed with status ${response.status}`;
    let code: string | undefined;
    try {
      const body = await response.json();
      if (body.error) message = body.error;
      else if (body.message) message = body.message;
      code = body.code;
    } catch {
      // Not a JSON error body.
    }
    if (response.status === 401 && code === "UNAUTHENTICATED") {
      window.dispatchEvent(new CustomEvent(UNAUTHENTICATED_EVENT));
    }
    throw new ApiError(response.status, message, code);
  }

  const partial = parsePartialHeader(response.headers.get(PARTIAL_HEADER));
  if (partial && (options.method ?? "GET").toUpperCase() === "GET") {
    partialAnswers.record(path.split("?")[0], partial);
  }
  const missing = partial?.partial ? partial.missing : null;

  if (response.status === 204) {
    return { data: undefined as T, missing };
  }

  return { data: (await response.json()) as T, missing };
}

/** Call the Valka REST API; see `fetchAPIResult`. */
export async function fetchAPI<T>(path: string, options: RequestInit = {}): Promise<T> {
  return (await fetchAPIResult<T>(path, options)).data;
}
