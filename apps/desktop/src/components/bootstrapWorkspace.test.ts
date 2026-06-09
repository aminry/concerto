import { describe, expect, it, vi, beforeEach } from "vitest";

vi.mock("../api/workareas", () => ({
  createWorkarea: vi.fn(),
}));
vi.mock("../api/sessions", () => ({
  createSession: vi.fn(),
}));

import { createWorkarea } from "../api/workareas";
import { createSession } from "../api/sessions";
import { bootstrapWorkspace, DEFAULT_FIRST_AGENT } from "./bootstrapWorkspace";

describe("bootstrapWorkspace", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("creates a workarea then a claude session and returns both ids", async () => {
    (createWorkarea as ReturnType<typeof vi.fn>).mockResolvedValue({ id: "wa1" });
    (createSession as ReturnType<typeof vi.fn>).mockResolvedValue({ id: "s1" });

    const result = await bootstrapWorkspace("ws1");

    expect(createWorkarea).toHaveBeenCalledWith("ws1");
    expect(createSession).toHaveBeenCalledWith({
      workareaId: "wa1",
      agentKind: DEFAULT_FIRST_AGENT,
    });
    expect(result).toEqual({ workareaId: "wa1", sessionId: "s1" });
  });

  it("defaults the first agent to claude", () => {
    expect(DEFAULT_FIRST_AGENT).toBe("claude");
  });
});
