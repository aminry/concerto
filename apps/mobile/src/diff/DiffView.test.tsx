// DiffView tests (Task 514): renders parsed rows, collapses/expands a hunk on
// tap (JS-only), and smoke-renders the 1000-line perf fixture without error.
// RN-TL v13.3.3. The on-device 60fps / <1.5s budget is Tier-3 (NOT asserted).
import { fireEvent, render, screen } from "@testing-library/react-native";

import { DiffView, PERF_BUDGET } from "./DiffView";
import { SAMPLE_DIFF, makeLargeDiff } from "./diff-fixtures";

describe("DiffView", () => {
  it("renders file headers, hunk headers and body lines from diff text", () => {
    render(<DiffView diffText={SAMPLE_DIFF} />);

    expect(screen.getByTestId("diff-view")).toBeOnTheScreen();
    // Both file headers render.
    expect(screen.getByText("src/app.ts")).toBeOnTheScreen();
    expect(screen.getByText("README.md")).toBeOnTheScreen();
    // A removed and an added body line render their stripped content.
    expect(screen.getByText("const PORT = 3000;")).toBeOnTheScreen();
    expect(screen.getByText("# Concerto")).toBeOnTheScreen();
  });

  it("shows a summary of files / additions / removals", () => {
    render(<DiffView diffText={SAMPLE_DIFF} />);
    expect(screen.getByTestId("diff-view-summary")).toBeOnTheScreen();
    expect(screen.getByText("2 files")).toBeOnTheScreen();
  });

  it("collapses a hunk's body lines when its header is tapped, then re-expands", () => {
    render(<DiffView diffText={SAMPLE_DIFF} />);

    // The README hunk's added line is visible initially.
    expect(screen.getByText("# Concerto")).toBeOnTheScreen();

    // Find the README hunk toggle (the last hunk) and collapse it.
    const toggles = screen.getAllByTestId(/^diff-hunk-toggle-/);
    const lastToggle = toggles[toggles.length - 1];
    fireEvent.press(lastToggle);

    // Its body line is now removed from the visible list.
    expect(screen.queryByText("# Concerto")).toBeNull();
    // The hunk header itself stays visible (so it can be re-expanded).
    expect(lastToggle).toBeOnTheScreen();

    // Tapping again restores the body line.
    fireEvent.press(lastToggle);
    expect(screen.getByText("# Concerto")).toBeOnTheScreen();
  });

  it("honors initiallyCollapsed (body hidden, headers shown)", () => {
    render(<DiffView diffText={SAMPLE_DIFF} initiallyCollapsed />);
    expect(screen.queryByText("# Concerto")).toBeNull();
    // Hunk headers + file headers remain.
    expect(screen.getByText("README.md")).toBeOnTheScreen();
    expect(screen.getAllByTestId(/^diff-hunk-toggle-/).length).toBeGreaterThan(0);
  });

  it("renders the empty state for an empty diff", () => {
    render(<DiffView diffText="" />);
    expect(screen.getByTestId("diff-view-empty")).toBeOnTheScreen();
  });

  it("smoke-renders a 1000-line diff without throwing", () => {
    // Virtualization means only the first window mounts; we assert the list
    // exists + a known early line is present. The 60fps / <1.5s budget is Tier-3.
    expect(() => render(<DiffView diffText={makeLargeDiff(1000)} />)).not.toThrow();
    expect(screen.getByTestId("diff-view-list")).toBeOnTheScreen();
    expect(screen.getByText("new value at line 0")).toBeOnTheScreen();
  });

  it("documents the spike-103 perf budget as a Tier-3 line", () => {
    expect(PERF_BUDGET.diffLines).toBe(1000);
    expect(PERF_BUDGET.firstPaintMs).toBe(1500);
    expect(PERF_BUDGET.targetFps).toBe(60);
    expect(PERF_BUDGET.fallback).toMatch(/native-diff/);
  });
});
