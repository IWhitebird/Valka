import { fetchAPI } from "./client";
import type { Worker } from "./types";

export const workersApi = {
  list(): Promise<Worker[]> {
    return fetchAPI<Worker[]>("/api/v1/workers");
  },
};
