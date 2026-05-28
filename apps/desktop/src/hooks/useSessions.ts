// React Query hook for `Sessions.ListSessions(workarea_id)`.
//
// V0.1 caps sessions per workarea at 1 (see `tasks/26 §Scope — out`),
// but the list shape is the same so the query is identical to
// Workareas.ListWorkareas. The tab strip in `WorkareaDetail` reads
// from this hook.

import { useQuery } from "@tanstack/react-query";

import { listSessions } from "../api/sessions";

export function useSessions(workareaId: string | null | undefined) {
  return useQuery({
    queryKey: ["sessions", workareaId] as const,
    queryFn: async () => {
      if (!workareaId) return { sessions: [] };
      return listSessions(workareaId);
    },
    enabled: !!workareaId,
  });
}
