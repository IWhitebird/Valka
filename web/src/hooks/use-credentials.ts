import { useSyncExternalStore } from "react";
import { credentials } from "@/lib/credentials";

/** Whether an API key is stored; re-renders when it is set or forgotten. */
export function useHasApiKey(): boolean {
  return useSyncExternalStore(credentials.subscribe, () => credentials.apiKey() !== null);
}

/** Whether an admin token is set for this tab; re-renders when it changes. */
export function useHasAdminToken(): boolean {
  return useSyncExternalStore(credentials.subscribe, () => credentials.adminToken() !== null);
}
