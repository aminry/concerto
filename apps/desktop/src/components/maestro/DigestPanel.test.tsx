// @vitest-environment jsdom
//
// Component tests for the digest panel (Task 415). Proves: the textual
// Finished/Blocked/Still-working grouping (split out of `Digest.text`, since
// the grouping is NOT a wire field) + the one-line next step render; the R-7
// stale-badge renders when inert; the privacy-blanked summary row renders; the
// persisted chips (D11) render + click.

import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { DigestPanel, splitDigestSections } from "./DigestPanel";
import type { Digest } from "../../api/maestro";

const digest: Digest = {
  text:
    "Finished: merged 2 PRs on bach.\n" +
    "Blocked: web is awaiting review.\n" +
    "Still working: mozart is running tests.\n" +
    "Next: review the web PR.",
  chips: [
    {
      rule_id: "chip-1",
      workarea_id: "wa-web",
      title: "Review web PR",
      priority: 5,
      created_at_ms: 1717459200000,
      action: "open_pr",
    },
  ],
  generated_at_ms: 1717459200000,
  stale: false,
};

describe("splitDigestSections", () => {
  it("splits prose on the canonical group headers", () => {
    const sections = splitDigestSections(digest.text);
    const headers = sections.map((s) => s.header);
    expect(headers).toContain("Finished");
    expect(headers).toContain("Blocked");
    expect(headers).toContain("Still working");
  });

  it("returns a single null-headed section when no header is present", () => {
    const sections = splitDigestSections("just some prose");
    expect(sections).toHaveLength(1);
    expect(sections[0].header).toBeNull();
  });
});

describe("DigestPanel", () => {
  it("renders the grouped digest + the next-step line + persisted chips", () => {
    render(<DigestPanel digest={digest} />);
    expect(screen.getByTestId("digest-group-finished")).toBeTruthy();
    expect(screen.getByTestId("digest-group-blocked")).toBeTruthy();
    expect(screen.getByTestId("digest-group-still-working")).toBeTruthy();
    expect(screen.getByText(/review the web PR/i)).toBeTruthy();
    expect(screen.getByText("Review web PR")).toBeTruthy();
  });

  it("fires onChipClick when a persisted chip is clicked", async () => {
    const onChipClick = vi.fn();
    render(<DigestPanel digest={digest} onChipClick={onChipClick} />);
    await userEvent.click(screen.getByText("Review web PR"));
    expect(onChipClick).toHaveBeenCalledTimes(1);
    expect(onChipClick.mock.calls[0][0].rule_id).toBe("chip-1");
  });

  it("renders the R-7 stale badge when inert", () => {
    render(<DigestPanel digest={digest} inert />);
    expect(screen.getByTestId("stale-badge")).toBeTruthy();
  });

  it("renders the stale badge when digest.stale is set", () => {
    render(<DigestPanel digest={{ ...digest, stale: true }} />);
    expect(screen.getByTestId("stale-badge")).toBeTruthy();
  });

  it("renders the privacy-blanked summary row", () => {
    render(
      <DigestPanel
        digest={digest}
        summaryRows={[
          { workareaId: "wa-x", composerName: "secret", blanked: true },
          {
            workareaId: "wa-y",
            composerName: "bach",
            status: "active",
            branch: "feat/x",
          },
        ]}
      />,
    );
    expect(screen.getByTestId("blanked-row").textContent).toMatch(
      /private workarea, name only/i,
    );
    expect(screen.getByText("feat/x")).toBeTruthy();
  });

  it("renders an empty state when there is no digest", () => {
    render(<DigestPanel digest={null} />);
    expect(screen.getByTestId("digest-empty")).toBeTruthy();
  });
});
