// Shared clone-strategy picker + size→strategy recommendation logic.
//
// Used by both the Settings → "Add Repository" form and the "New Project"
// modal's inline first-repo step, so the size-probe / recommendation
// behaviour (design/02 §3.5 heuristic, surfaced per design/15 §7.1) stays
// identical in both places. Treeless is intentionally never offered in the
// UI (design/02 §12 R-1).

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery, type UseQueryResult } from "@tanstack/react-query";

import { estimateRepoSize, type SizeReport } from "../api/repositories";
import { useDebouncedValue } from "../hooks/useConeEstimate";
import { formatBytes } from "./ConePicker";
import { Segmented } from "./ui/segmented";

// The three strategy choices the picker offers. Treeless is intentionally
// absent (R-1). Each maps to the `(clone_strategy, with_sparse)` pair
// `AddRepository` takes (Task 301).
export type StrategyChoice = "full" | "blobless" | "blobless-sparse";

const STRATEGY_ITEMS: ReadonlyArray<{ id: StrategyChoice; label: string }> = [
  { id: "full", label: "Full" },
  { id: "blobless", label: "Blobless" },
  { id: "blobless-sparse", label: "Blobless + Sparse" },
];

const STRATEGY_BLURB: Record<StrategyChoice, string> = {
  full: "Every blob on disk. Best for small repos and offline work.",
  blobless: "Faster clone; file contents fetched on demand.",
  "blobless-sparse":
    "Blobless plus a sparse cone — only the directories you pick land on disk.",
};

const STRATEGY_LABEL: Record<StrategyChoice, string> = {
  full: "Full",
  blobless: "Blobless",
  "blobless-sparse": "Blobless + Sparse",
};

/// Map the `SizeReport` recommendation (design/02 §3.5 heuristic, computed on
/// the Core) onto a picker choice. `recommended_strategy` is `full` or
/// `blobless` (treeless is never recommended); `recommend_sparse` promotes
/// blobless to the "+ Sparse" tier (>10 GB).
export function choiceFromReport(report: SizeReport): StrategyChoice {
  if (report.recommended_strategy === "blobless") {
    return report.recommend_sparse ? "blobless-sparse" : "blobless";
  }
  return "full";
}

/// Split a picker choice back into the `(cloneStrategy, withSparse)` the
/// `addRepository` binding sends.
export function choiceToWire(choice: StrategyChoice): {
  cloneStrategy: "full" | "blobless";
  withSparse: boolean;
} {
  switch (choice) {
    case "full":
      return { cloneStrategy: "full", withSparse: false };
    case "blobless":
      return { cloneStrategy: "blobless", withSparse: false };
    case "blobless-sparse":
      return { cloneStrategy: "blobless", withSparse: true };
  }
}

export type UseCloneStrategy = {
  /// The user-facing choice (defaults to Full so the form is usable before
  /// any probe). Tracks the recommendation until the user overrides it.
  strategy: StrategyChoice;
  /// Wire shape for `addRepository`, derived from `strategy`.
  wire: { cloneStrategy: "full" | "blobless"; withSparse: boolean };
  /// Props to spread onto `<CloneStrategyPicker {...pickerProps} />`.
  pickerProps: CloneStrategyPickerProps;
  /// Reset back to Full + clear the override latch (after a successful add).
  reset: () => void;
  /// Underlying size probe (exposed for callers that want the raw report).
  sizeQuery: UseQueryResult<SizeReport>;
};

/// Owns the URL size-probe, the recommendation latch, and the selected
/// strategy. `url` is the (untrimmed) repository URL the caller's input is
/// bound to; the hook trims + debounces it internally.
export function useCloneStrategy(url: string): UseCloneStrategy {
  // `strategy` defaults to Full so the form is usable before any probe.
  // `userOverrode` flips true once the user touches the selector — after that
  // a fresh recommendation no longer stomps their choice.
  const [strategy, setStrategy] = useState<StrategyChoice>("full");
  const userOverrodeRef = useRef(false);

  // Pre-clone size probe (Task 301). Debounce the URL so each keystroke
  // doesn't hit the remote; only probe a non-empty, trimmed URL. A probe
  // failure (private/offline repo) is NOT fatal — `retry: false` and the
  // form falls back to a manual pick with a note (design/02 §3.5/§7.1).
  const trimmedUrl = url.trim();
  const debouncedUrl = useDebouncedValue(trimmedUrl, 500);
  const sizeQuery = useQuery<SizeReport>({
    queryKey: ["repoSize", debouncedUrl] as const,
    queryFn: () => estimateRepoSize(debouncedUrl),
    enabled: debouncedUrl.length > 0,
    retry: false,
    staleTime: 60_000,
  });

  const recommendedChoice = useMemo(
    () => (sizeQuery.data ? choiceFromReport(sizeQuery.data) : null),
    [sizeQuery.data],
  );

  // Default the selector to the recommendation when one arrives, unless the
  // user has already overridden it.
  useEffect(() => {
    if (recommendedChoice && !userOverrodeRef.current) {
      setStrategy(recommendedChoice);
    }
  }, [recommendedChoice]);

  // Reset the "user overrode" latch whenever the URL changes, so a brand-new
  // repo's recommendation can take effect again.
  useEffect(() => {
    userOverrodeRef.current = false;
  }, [debouncedUrl]);

  // Stable callbacks: a fresh identity each render would, e.g., re-fire a
  // caller's reset-on-open effect every render and wipe its other inputs.
  const onSelect = useCallback((choice: StrategyChoice): void => {
    userOverrodeRef.current = true;
    setStrategy(choice);
  }, []);

  const reset = useCallback((): void => {
    setStrategy("full");
    userOverrodeRef.current = false;
  }, []);

  return {
    strategy,
    wire: choiceToWire(strategy),
    reset,
    sizeQuery,
    pickerProps: {
      strategy,
      onSelect,
      probing: sizeQuery.isFetching,
      report: sizeQuery.data ?? null,
      recommended: recommendedChoice,
      probeFailed: sizeQuery.isError,
    },
  };
}

export type CloneStrategyPickerProps = {
  strategy: StrategyChoice;
  onSelect: (choice: StrategyChoice) => void;
  probing: boolean;
  report: SizeReport | null;
  recommended: StrategyChoice | null;
  probeFailed: boolean;
};

/// The clone-strategy block: a size→strategy recommendation line (design/02
/// §3.5, surfaced per design/15 §7.1) plus a Full / Blobless / Blobless +
/// Sparse selector defaulting to the recommendation. Always visible so the
/// option is discoverable before a URL is entered; before a probe completes
/// it shows a neutral hint. Treeless is never an option (R-1). A probe
/// failure (private/offline) degrades to a manual pick with a note rather
/// than blocking the add.
export function CloneStrategyPicker({
  strategy,
  onSelect,
  probing,
  report,
  recommended,
  probeFailed,
}: CloneStrategyPickerProps): JSX.Element {
  return (
    <div className="space-y-1.5">
      <label className="block text-xs uppercase tracking-wider text-faint">
        Clone strategy
      </label>

      {probing && (
        <p className="text-xs text-faint">Estimating repository size…</p>
      )}

      {!probing && report && recommended && (
        <p className="text-xs text-faint">
          ≈ {formatBytes(report.size_bytes)}{" "}
          <span className="opacity-70">(est.)</span> ·{" "}
          {report.object_count.toLocaleString()} objects → recommended:{" "}
          <span className="font-semibold text-foreground">
            {STRATEGY_LABEL[recommended]}
          </span>
        </p>
      )}

      {!probing && probeFailed && (
        <p className="text-xs text-warn">
          Couldn’t reach the remote to estimate its size (it may be private or
          offline). Pick a strategy manually — defaulting to Full.
        </p>
      )}

      {!probing && !report && !probeFailed && (
        <p className="text-xs text-faint">
          Enter a repository URL for a size-based recommendation, or pick a
          strategy below.
        </p>
      )}

      <Segmented<StrategyChoice>
        items={STRATEGY_ITEMS}
        active={strategy}
        onSelect={onSelect}
      />
      <p className="text-xs text-faint">{STRATEGY_BLURB[strategy]}</p>
    </div>
  );
}
