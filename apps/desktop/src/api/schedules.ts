// Typed wrapper around `Schedules.ListSchedules`. The Scheduler tab in
// the Task 46 right rail consumes this; create / pause / delete RPCs
// land alongside the V0.1 Maestro surface in a later task.

import { callRpc } from "./client";

/// Mirrors `concerto.v1.Schedule`. Timestamps land as `[seconds, nanos]`
/// tuples per the shared serde shim.
export type Schedule = {
  id: string;
  workarea_id: string;
  kind: string;
  interval_seconds: number;
  expires_at?: [number, number] | null;
  last_run_at?: [number, number] | null;
  paused: boolean;
  prompt: string;
  agent_kind: string;
  created_at?: [number, number] | null;
};

export type ListSchedulesResponse = {
  schedules: Schedule[];
};

export async function listSchedules(
  workareaId: string,
): Promise<ListSchedulesResponse> {
  return callRpc<{ workarea_id: string }, ListSchedulesResponse>(
    "Schedules.ListSchedules",
    { workarea_id: workareaId },
  );
}
