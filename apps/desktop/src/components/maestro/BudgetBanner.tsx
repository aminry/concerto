// The Maestro budget / policy banners (Task 415, design/08 §3.9 R-10 / §3.10).
//
//   - budget-exhausted → YELLOW banner "Maestro budget exhausted; routing still
//     works" (R-10; routing spends zero tokens, so it survives exhaustion);
//   - 80% amber / 100% red thresholds computed from `MaestroState` daily
//     counters vs the budget;
//   - `enterpriseDataPrivacy`-disabled → the policy banner (§3.10).
//
// The banner state is derived from the server-canonical `MaestroState`
// (404/403/412 own the counters; 414 surfaces them) and/or the
// `maestro.events` `budget_exhausted` / `disabled_by_policy` frames. 415 only
// READS the state to render — it computes no budget and gates nothing.

import type { MaestroState } from "../../api/maestro";

/// The banner severity tier. `none` → render nothing.
export type BannerLevel = "none" | "amber" | "red" | "exhausted" | "policy";

/// The 80%/100% thresholds (design/08 §3.9 R-10).
export const BUDGET_AMBER_FRACTION = 0.8;
export const BUDGET_RED_FRACTION = 1.0;

export type BudgetBannerProps = {
  state?: MaestroState | null;
  /// The daily token budget (in == out cap). 414/412 surface this; until then
  /// the banner falls back to event-driven exhaustion only when it's absent.
  budget?: number | null;
  /// Set when a `maestro.events` `budget_exhausted` frame arrived (the
  /// event-driven path, independent of the counters).
  exhaustedByEvent?: boolean;
  /// Set when a `maestro.events` `disabled_by_policy` frame arrived, carrying
  /// the reason (e.g. enterpriseDataPrivacy + external model, §3.10).
  policyDisabledReason?: string | null;
};

/// Compute the banner level from the inputs. Pure — unit-tested. Precedence:
/// policy-disabled (hard stop) > exhausted > red(100%) > amber(80%) > none.
/// The "exhausted" tier is the explicit yellow R-10 banner; the amber/red
/// tiers are the pre-exhaustion warning thresholds.
export function computeBannerLevel(
  state: MaestroState | null | undefined,
  budget: number | null | undefined,
  exhaustedByEvent: boolean | undefined,
  policyDisabledReason: string | null | undefined,
): BannerLevel {
  if (policyDisabledReason) return "policy";
  if (state && state.enabled === false) {
    // Inert by exhaustion/policy; the explicit yellow exhausted banner.
    return "exhausted";
  }
  if (exhaustedByEvent) return "exhausted";
  if (state && budget && budget > 0) {
    const used = Math.max(state.daily_in_today, state.daily_out_today);
    const frac = used / budget;
    if (frac >= BUDGET_RED_FRACTION) return "red";
    if (frac >= BUDGET_AMBER_FRACTION) return "amber";
  }
  return "none";
}

const STYLE: Record<Exclude<BannerLevel, "none">, string> = {
  amber: "border-warn/40 bg-warn/10 text-warn",
  red: "border-err/40 bg-err/10 text-err",
  exhausted: "border-warn/40 bg-warn/15 text-warn",
  policy: "border-err/40 bg-err/10 text-err",
};

export function BudgetBanner({
  state,
  budget,
  exhaustedByEvent,
  policyDisabledReason,
}: BudgetBannerProps): JSX.Element | null {
  const level = computeBannerLevel(
    state,
    budget,
    exhaustedByEvent,
    policyDisabledReason,
  );
  if (level === "none") return null;

  const message =
    level === "policy"
      ? policyDisabledReason ||
        "Concerto chat disabled by enterprise data-privacy policy."
      : level === "exhausted"
        ? "Maestro budget exhausted; routing still works."
        : level === "red"
          ? "Maestro budget reached; the chat may stop responding."
          : "Maestro budget at 80%.";

  return (
    <div
      role="status"
      data-testid="budget-banner"
      data-level={level}
      className={`border-b px-3 py-1.5 text-xs ${STYLE[level]}`}
    >
      {message}
    </div>
  );
}
