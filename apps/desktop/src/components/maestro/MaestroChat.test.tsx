// @vitest-environment jsdom
//
// Tests for the live budget/state feed (Task 417). Proves: `deriveBudget`
// pairs the larger daily counter with its own cap so `<BudgetBanner>`'s
// amber(80%)/red(100%) thresholds light from a real 9-field `MaestroState`
// (counts vs caps), without fabricating any value. The pure threshold math is
// covered in `BudgetBanner.test.tsx`; here we pin the cap-derivation seam the
// chat feeds it.

import { describe, expect, it } from "vitest";

import { deriveBudget } from "./MaestroChat";
import { computeBannerLevel } from "./BudgetBanner";
import type { MaestroState } from "../../api/maestro";

function state(partial: Partial<MaestroState>): MaestroState {
  return {
    enabled: true,
    daily_in_today: 0,
    daily_out_today: 0,
    in_cap: 200000,
    out_cap: 50000,
    last_digest_at_ms: 0,
    inert: false,
    inert_reason: "",
    maestro_session_id: "",
    ...partial,
  };
}

describe("deriveBudget", () => {
  it("returns null without state", () => {
    expect(deriveBudget(null)).toBeNull();
    expect(deriveBudget(undefined)).toBeNull();
  });

  it("pairs the larger counter with its own cap (input-bound ⇒ in_cap)", () => {
    expect(
      deriveBudget(state({ daily_in_today: 100000, daily_out_today: 1000 })),
    ).toBe(200000);
  });

  it("pairs the larger counter with its own cap (output-bound ⇒ out_cap)", () => {
    expect(
      deriveBudget(state({ daily_in_today: 1000, daily_out_today: 40000 })),
    ).toBe(50000);
  });
});

describe("budget meter from a live MaestroState", () => {
  it("lights amber at 80% of the output cap (output-bound)", () => {
    const s = state({ daily_in_today: 1000, daily_out_today: 40000 }); // 40k/50k
    expect(computeBannerLevel(s, deriveBudget(s), false, null)).toBe("amber");
  });

  it("lights red at 100% of the input cap (input-bound)", () => {
    const s = state({ daily_in_today: 200000, daily_out_today: 1000 }); // 200k/200k
    expect(computeBannerLevel(s, deriveBudget(s), false, null)).toBe("red");
  });

  it("stays none below 80%", () => {
    const s = state({ daily_in_today: 50000, daily_out_today: 1000 }); // 50k/200k
    expect(computeBannerLevel(s, deriveBudget(s), false, null)).toBe("none");
  });

  it("reports exhausted when the live state is inert (enabled=false)", () => {
    const s = state({ enabled: false, inert: true, inert_reason: "budget_exhausted" });
    expect(computeBannerLevel(s, deriveBudget(s), false, null)).toBe("exhausted");
  });
});
