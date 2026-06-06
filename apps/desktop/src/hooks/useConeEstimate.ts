// React Query hook over `Repositories.EstimateConeSize` (Task 305) for
// the sparse-cone picker (Task 322).
//
// Server-canonical telemetry lives in React Query, keyed by
// `["coneEstimate", repositoryId, cone_paths]` (per design/15 §3.3 — never
// duplicated into Zustand). The hook does NOT debounce itself; callers
// pass an already-debounced `conePaths` so each keystroke doesn't fire an
// RPC (see `useDebouncedValue`). A Core rejection of a bad cone path comes
// back as a `CoreClientError` `{kind,message}` — `query.error` carries it,
// read via `errorMessage`/`formatError`.

import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";

import { estimateConeSize, type ConeStats } from "../api/cones";

export function coneEstimateQueryKey(
  repositoryId: string | null | undefined,
  conePaths: string[],
) {
  return ["coneEstimate", repositoryId, conePaths] as const;
}

export function useConeEstimate(
  repositoryId: string | null | undefined,
  conePaths: string[],
  enabled = true,
) {
  return useQuery<ConeStats>({
    queryKey: coneEstimateQueryKey(repositoryId, conePaths),
    queryFn: async () => {
      if (!repositoryId) return { file_count: 0, disk_size_bytes: 0 };
      return estimateConeSize(repositoryId, conePaths);
    },
    enabled: enabled && !!repositoryId,
    retry: false, // a bad cone path is a clean reject, not a transient error
  });
}

/// Debounce a value, re-emitting only after `delayMs` of stability. Used
/// by the cone picker so the `EstimateConeSize` RPC fires on a pause, not
/// per keystroke (~300 ms per the Task 322 implementation note).
export function useDebouncedValue<T>(value: T, delayMs = 300): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const handle = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(handle);
  }, [value, delayMs]);
  return debounced;
}
