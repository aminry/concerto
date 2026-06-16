// Unit tests for the mobile workspaces data seam (Task 513). Proves the
// fixture-backed `mockWorkspacesClient` honours the `WorkspacesClient` contract
// against @concerto/client's REAL generated proto types (PHASE5_PLANNING D11).
import { demoWorkspacesFixture, makeWorkspace } from "./fixtures";
import { mockWorkspacesClient } from "./workspaces-client";

describe("mockWorkspacesClient", () => {
  it("lists active workspaces and filters archived by default", async () => {
    const fixture = demoWorkspacesFixture();
    const archived = makeWorkspace({
      id: "ws-old",
      name: "Old Workspace",
      archivedAt: { seconds: 1n, nanos: 0 },
    });
    const client = mockWorkspacesClient({ ...fixture, workspaces: [...fixture.workspaces, archived] });

    const active = await client.listWorkspaces();
    expect(active.map((w) => w.id)).toEqual(["ws-web", "ws-core"]);

    const all = await client.listWorkspaces({ includeArchived: true });
    expect(all.map((w) => w.id)).toContain("ws-old");
  });

  it("lists workareas scoped to a workspace (no project tier — D14)", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture());
    const webAreas = await client.listWorkareas("ws-web");
    expect(webAreas.map((w) => w.id)).toEqual(["wa-aria"]);
    const coreAreas = await client.listWorkareas("ws-core");
    expect(coreAreas.map((w) => w.id)).toEqual(["wa-bee"]);
  });

  it("returns a workarea's sessions and PR set", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture());
    const sessions = await client.listSessions("wa-aria");
    expect(sessions).toHaveLength(2);
    expect(sessions[0].agentKind).toBe("claude");

    const prs = await client.getWorkareaPrSet("wa-aria");
    expect(prs).toHaveLength(1);
    expect(prs[0].prNumber).toBe(482n);
  });

  it("returns a PR's unified-diff text and '' for an unknown PR (Task 514)", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture());
    const diff = await client.getPrDiff("pr-1");
    expect(diff).toContain("diff --git");
    expect(diff).toContain("+++ b/README.md");
    // A PR with no diff fixture resolves to the empty string (no diff).
    await expect(client.getPrDiff("pr-missing")).resolves.toBe("");
  });

  it("getWorkarea throws for an unknown id", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture());
    await expect(client.getWorkarea("nope")).rejects.toThrow(/not found/);
  });

  it("failWith rejects every method (drives the error state)", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture(), { failWith: "core unreachable" });
    await expect(client.listWorkspaces()).rejects.toThrow("core unreachable");
    await expect(client.listWorkareas("ws-web")).rejects.toThrow("core unreachable");
    await expect(client.listSessions("wa-aria")).rejects.toThrow("core unreachable");
    await expect(client.getWorkareaPrSet("wa-aria")).rejects.toThrow("core unreachable");
    await expect(client.getPrDiff("pr-1")).rejects.toThrow("core unreachable");
  });
});
