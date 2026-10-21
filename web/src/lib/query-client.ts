import { QueryClient } from "@tanstack/react-query";
import { ApiError } from "@/api/client";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 5_000,
      refetchInterval: 10_000,
      refetchOnWindowFocus: true,
      // A refused or missing resource will not change on a retry.
      retry: (failures, error) =>
        failures < 2 &&
        !(error instanceof ApiError && [400, 401, 403, 404].includes(error.status)),
      retryDelay: (attemptIndex) => Math.min(1000 * 2 ** attemptIndex, 10000),
    },
    mutations: {
      retry: false,
    },
  },
});
