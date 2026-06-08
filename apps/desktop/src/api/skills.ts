// Typed wrapper around `Skills.ListSkills`. The Skills tab in the Task
// 46 right rail consumes this; the toggle + refresh RPCs land alongside
// the V0.1 settings surface in a later task.

import { callRpc } from "./client";

/// Mirrors `concerto.v1.Skill`.
export type Skill = {
  id: string;
  scope: string;
  workspace_id: string;
  name: string;
  slash_command: string;
  description: string;
  tools: string[];
  source_path: string;
  enabled: boolean;
};

export type ListSkillsResponse = {
  skills: Skill[];
};

export async function listSkills(input?: {
  scope?: string;
  workspaceId?: string;
  enabledOnly?: boolean;
}): Promise<ListSkillsResponse> {
  return callRpc<
    { scope?: string; workspace_id?: string; enabled_only?: boolean },
    ListSkillsResponse
  >("Skills.ListSkills", {
    scope: input?.scope,
    workspace_id: input?.workspaceId,
    enabled_only: input?.enabledOnly,
  });
}
