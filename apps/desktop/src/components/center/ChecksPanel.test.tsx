// @vitest-environment jsdom
//
// Component tests for the Level-2 Checks panel (Task 324, design/15 §3.4).
// Proves: GetChecks runs render with the right colour band; a live
// `checks.<wa>.<repo>` opaque frame invalidates + re-fetches.
//
// Tier-2 double: mocked `invoke` + a captured `onConcertoEvent` callback per
// subject (the event-bus stub). No Core, no real check-run sync.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

const eventCallbacks = new Map<string, Array<(p: unknown) => void>>();
function fireEvent(subject: string, payload: unknown): void {
  for (const cb of eventCallbacks.get(subject) ?? []) cb(payload);
}
vi.mock("../../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../api/client")>();
  return {
    ...actual,
    onConcertoEvent: vi.fn(async (subject: string, cb: (p: unknown) => void) => {
      const list = eventCallbacks.get(subject) ?? [];
      list.push(cb);
      eventCallbacks.set(subject, list);
      return () => {
        const cur = eventCallbacks.get(subject);
        if (cur) eventCallbacks.set(subject, cur.filter((c) => c !== cb));
      };
    }),
    subscribe: vi.fn(async () => "sub-1"),
    unsubscribe: vi.fn(async () => {}),
  };
});

import { ChecksPanel } from "./ChecksPanel";
import { renderWithClient } from "../test-utils";
import type { PullRequest } from "../../api/vcs";

const pr: PullRequest = {
  id: "pr-1",
  workarea_id: "wa-1",
  repository_id: "repo-a",
  provider: "github",
  pr_number: 7,
  base_ref: "main",
  head_ref: "concerto/api",
  state: "open",
  title: "T",
  body: "",
  url: "https://gh/pr/7",
  head_sha: "sha-1",
  created_at: 0,
  updated_at: 0,
  merge_order: 0,
};

function bytes(obj: unknown): number[] {
  return Array.from(new TextEncoder().encode(JSON.stringify(obj)));
}

let checksResponse: { checks: unknown[] };

beforeEach(() => {
  invoke.mockReset();
  eventCallbacks.clear();
  checksResponse = {
    checks: [{ name: "build", status: "completed", conclusion: "success", details_url: "" }],
  };
  invoke.mockImplementation((cmd: string, args: { method?: string }) => {
    if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
    if (args.method === "Vcs.GetChecks") return Promise.resolve(checksResponse);
    return Promise.resolve(undefined);
  });
});

describe("ChecksPanel", () => {
  it("renders check runs with the matching colour band", async () => {
    renderWithClient(<ChecksPanel workareaId="wa-1" repositoryId="repo-a" pr={pr} />);
    const row = await screen.findByTestId("check-row");
    expect(row).toHaveAttribute("data-band", "green");
    expect(screen.getByText("build")).toBeInTheDocument();
  });

  it("shows an empty state when the repo has no PR", () => {
    renderWithClient(<ChecksPanel workareaId="wa-1" repositoryId="repo-a" pr={null} />);
    expect(screen.getByText(/No pull request for this repo yet/)).toBeInTheDocument();
  });

  it("live-invalidates on a checks.<wa>.<repo> frame and re-fetches", async () => {
    renderWithClient(<ChecksPanel workareaId="wa-1" repositoryId="repo-a" pr={pr} />);
    const row = await screen.findByTestId("check-row");
    expect(row).toHaveAttribute("data-band", "green");

    // The webhook flips the build red; the next fetch returns the failure.
    checksResponse = {
      checks: [{ name: "build", status: "completed", conclusion: "failure", details_url: "" }],
    };
    fireEvent("checks.wa-1.repo-a", {
      checks_opaque: bytes({
        kind: "check_run_updated",
        workarea_id: "wa-1",
        repository_id: "repo-a",
        entity: { sha: "sha-1", runs: [] },
      }),
    });

    await waitFor(() =>
      expect(screen.getByTestId("check-row")).toHaveAttribute("data-band", "red"),
    );
  });
});
