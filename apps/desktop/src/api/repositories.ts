// Typed wrappers around `Repositories.*` RPCs.
//
// Mirrors `concerto.v1.Repository`. The streaming `Clone` surface
// lives on `clone_repository` (see `client.ts::cloneRepository`) and
// is driven via Tauri's typed event bus, not `callRpc`.

import { callRpc } from "./client";

export type Repository = {
  id: string;
  project_id: string;
  name: string;
  url: string;
  local_path: string;
  clone_strategy: string;
  default_branch: string;
  last_fetch_at?: [number, number] | null;
};

export type ListRepositoriesResponse = {
  repositories: Repository[];
};

export async function listRepositories(
  projectId: string,
): Promise<ListRepositoriesResponse> {
  return callRpc<{ project_id: string }, ListRepositoriesResponse>(
    "Repositories.ListByProject",
    { project_id: projectId },
  );
}

export async function addRepository(input: {
  projectId: string;
  name: string;
  url: string;
  defaultBranch?: string;
}): Promise<Repository> {
  return callRpc<
    {
      project_id: string;
      name: string;
      url: string;
      default_branch: string;
    },
    Repository
  >("Repositories.AddRepository", {
    project_id: input.projectId,
    name: input.name,
    url: input.url,
    default_branch: input.defaultBranch ?? "",
  });
}
