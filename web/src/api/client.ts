import { credentials, UNAUTHENTICATED_EVENT } from "@/lib/credentials";

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

/**
 * Call the Valka REST API. The API key, when one is set, goes as a bearer token; a caller
 * that sets its own `Authorization` header (the admin API) keeps it.
 */
export async function fetchAPI<T>(path: string, options: RequestInit = {}): Promise<T> {
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

  if (response.status === 204) {
    return undefined as T;
  }

  return response.json();
}
