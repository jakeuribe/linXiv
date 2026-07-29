import { QueryClient } from "@tanstack/react-query";
import { UPDATE_CHECK_QUERY_KEY } from "./updateSchedule";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      retry: 1,
    },
  },
});

// The launch check's result must survive stretches with no observer, which the
// default 5-minute gcTime would collect.
queryClient.setQueryDefaults([UPDATE_CHECK_QUERY_KEY], { gcTime: Infinity });
