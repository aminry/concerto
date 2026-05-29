// React Query hook for `Schedules.ListSchedules(workarea_id)`.
//
// Used by the right-rail Scheduler tab from Task 46.

import { useQuery } from "@tanstack/react-query";

import { listSchedules } from "../api/schedules";

export function useSchedules(workareaId: string | null | undefined) {
  return useQuery({
    queryKey: ["schedules", workareaId] as const,
    queryFn: async () => {
      if (!workareaId) return { schedules: [] };
      return listSchedules(workareaId);
    },
    enabled: !!workareaId,
  });
}
