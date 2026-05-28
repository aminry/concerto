// Typed wrappers around `Projects.*` RPCs.

import { callRpc } from "./client";

// Mirrors `concerto.v1.Project`. Field names match the proto's
// snake_case wire shape (prost's serde derive does not rename).
export type Project = {
  id: string;
  name: string;
  icon?: string | null;
  // `[seconds, nanos]` tuple per the `option_timestamp` serde shim
  // in `crates/proto/src/lib.rs::serde_compat`.
  created_at?: [number, number] | null;
  archived_at?: [number, number] | null;
};

export type ListProjectsResponse = {
  projects: Project[];
};

export async function listProjects(): Promise<ListProjectsResponse> {
  return callRpc<Record<string, never>, ListProjectsResponse>(
    "Projects.ListProjects",
    {},
  );
}
