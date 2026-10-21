// Where the dashboard keeps the credentials it sends.
//
// The API key (VALKA_API_KEYS on the server) stays in this browser until you sign out. The
// admin token (VALKA_ADMIN_TOKEN) can change where shards live and stop nodes, so it is
// kept for this tab only. Storage can be unavailable (private windows, blocked site
// data): every access is guarded and the dashboard then simply asks again.

const API_KEY = "valka.apiKey";
const ADMIN_TOKEN = "valka.adminToken";

/** Fired when the server refuses a request for a missing or wrong API key. */
export const UNAUTHENTICATED_EVENT = "valka:unauthenticated";

function read(storage: () => Storage, key: string): string | null {
  try {
    return storage().getItem(key) || null;
  } catch {
    return null;
  }
}

function write(storage: () => Storage, key: string, value: string | null): void {
  try {
    if (value) storage().setItem(key, value);
    else storage().removeItem(key);
  } catch {
    // Storage unavailable: the value lives until the page reloads.
  }
}

let apiKeyFallback: string | null = null;
let adminFallback: string | null = null;

export const credentials = {
  apiKey(): string | null {
    return read(() => localStorage, API_KEY) ?? apiKeyFallback;
  },
  setApiKey(key: string | null): void {
    apiKeyFallback = key;
    write(() => localStorage, API_KEY, key);
  },
  adminToken(): string | null {
    return read(() => sessionStorage, ADMIN_TOKEN) ?? adminFallback;
  },
  setAdminToken(token: string | null): void {
    adminFallback = token;
    write(() => sessionStorage, ADMIN_TOKEN, token);
  },
};
