// @vitest-environment jsdom
//
// Component + unit tests for the budget/policy banners (Task 415). Proves: the
// yellow budget-exhausted banner (R-10, routing still works); the 80% amber /
// 100% red thresholds computed from `MaestroState` counters vs the budget; the
// enterpriseDataPrivacy-disabled policy banner; and no banner below threshold.

import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";

import { BudgetBanner, computeBannerLevel } from "./BudgetBanner";
import type { MaestroState } from "../../api/maestro";

const enabled = (used: number): MaestroState => ({
  enabled: true,
  daily_in_today: used,
  daily_out_today: 0,
  last_digest_at_ms: null,
});

describe("computeBannerLevel", () => {
  it("returns none below 80%", () => {
    expect(computeBannerLevel(enabled(700), 1000, false, null)).toBe("none");
  });

  it("returns amber at 80%", () => {
    expect(computeBannerLevel(enabled(800), 1000, false, null)).toBe("amber");
  });

  it("returns red at 100%", () => {
    expect(computeBannerLevel(enabled(1000), 1000, false, null)).toBe("red");
  });

  it("returns exhausted when the state is disabled (inert)", () => {
    expect(
      computeBannerLevel(
        { ...enabled(0), enabled: false },
        1000,
        false,
        null,
      ),
    ).toBe("exhausted");
  });

  it("returns exhausted from a budget_exhausted event", () => {
    expect(computeBannerLevel(null, null, true, null)).toBe("exhausted");
  });

  it("policy-disabled takes precedence over everything", () => {
    expect(
      computeBannerLevel(enabled(1000), 1000, true, "enterprise"),
    ).toBe("policy");
  });
});

describe("BudgetBanner", () => {
  it("renders the yellow exhausted banner with the routing-still-works copy", () => {
    render(<BudgetBanner exhaustedByEvent />);
    const banner = screen.getByTestId("budget-banner");
    expect(banner.getAttribute("data-level")).toBe("exhausted");
    expect(banner.textContent).toMatch(/routing still works/i);
  });

  it("renders the amber threshold banner", () => {
    render(<BudgetBanner state={enabled(850)} budget={1000} />);
    expect(screen.getByTestId("budget-banner").getAttribute("data-level")).toBe(
      "amber",
    );
  });

  it("renders the policy-disabled banner with the reason", () => {
    render(<BudgetBanner policyDisabledReason="enterprise data privacy on" />);
    const banner = screen.getByTestId("budget-banner");
    expect(banner.getAttribute("data-level")).toBe("policy");
    expect(banner.textContent).toMatch(/enterprise data privacy on/i);
  });

  it("renders nothing below threshold", () => {
    const { container } = render(
      <BudgetBanner state={enabled(100)} budget={1000} />,
    );
    expect(container.firstChild).toBeNull();
  });
});
