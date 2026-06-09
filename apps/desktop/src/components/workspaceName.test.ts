import { describe, expect, it } from "vitest";
import { deriveWorkspaceName } from "./workspaceName";

describe("deriveWorkspaceName", () => {
  it("returns empty for no repos", () => {
    expect(deriveWorkspaceName([])).toBe("");
  });
  it("returns the single repo name", () => {
    expect(deriveWorkspaceName(["payments"])).toBe("payments");
  });
  it("joins two repos with a plus", () => {
    expect(deriveWorkspaceName(["payments", "billing"])).toBe(
      "payments + billing",
    );
  });
  it("summarizes 3+ repos with an N-more suffix", () => {
    expect(deriveWorkspaceName(["payments", "billing", "ledger"])).toBe(
      "payments + billing + 1 more",
    );
    expect(
      deriveWorkspaceName(["a", "b", "c", "d", "e"]),
    ).toBe("a + b + 3 more");
  });
});
