// The digest panel (Task 415, design/08 §3.6). Rendered ABOVE the chat
// composer, it shows the LLM-grouped Finished / Blocked / Still-working prose
// + the one-line proposed next step + the persisted next-step chips (D11).
//
// ── What is and isn't on the frozen wire ─────────────────────────────────────
// The Finished/Blocked/Still-working grouping is TEXTUAL — it lives inside
// `Digest.text` (the LLM-grouped prose), NOT as wire sub-messages. There is no
// `DigestGroup` on 401.5's frozen wire. So this panel STYLES the prose
// (section-splitting on the known headers when present) rather than mapping
// structured wire groups. The digest body is `text` + `chips` only.
//
// Any richer per-workarea hard-fact rows (status dot, branch, commits/PR/CI,
// the privacy-blanked `[private workarea, name only]` case) are sourced from
// the Desktop's EXISTING workarea state (React Query over
// `Workareas.ListWorkareas`), NOT from the `Digest` message — kept as an
// optional enhancement here (the `summaryRows` prop), since 415 derives no
// facts and applies no privacy gate client-side (404/409/413 own that).
//
// R-7: when the Maestro is inert (`stale`/`MaestroState.enabled=false`) we show
// the LAST GOOD digest DIMMED with a "stale" badge.

import type { Digest, MaestroChip } from "../../api/maestro";
import { workareaStatusToDot } from "../../lib/workareaStatus";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { StatusDot } from "../ui/status-dot";

/// The canonical group headers the LLM digest uses (design/08 §3.6). Used to
/// section-split `Digest.text` for styling; absent headers just render as
/// prose.
export const DIGEST_GROUP_HEADERS = [
  "Finished",
  "Blocked",
  "Still working",
] as const;

/// An optional per-workarea hard-fact row (design/08 §3.3). Sourced from the
/// Desktop's existing workarea state, NOT from the `Digest` wire message.
/// `blanked` marks the `exclude_from_maestro` privacy case → `[private
/// workarea, name only]` (413 owns the actual gate; this only renders it).
export type SummaryRow = {
  workareaId: string;
  composerName: string;
  status?: string;
  branch?: string;
  blanked?: boolean;
};

/// Split the digest prose into (header, body) sections on the known group
/// headers. Lines that don't start a known header attach to the current
/// section; a leading preamble (before any header) becomes the `null`-headed
/// section. Pure — unit-tested.
export function splitDigestSections(
  text: string,
): { header: string | null; body: string }[] {
  if (!text.trim()) return [];
  const headerRe = new RegExp(
    `^\\s*(${DIGEST_GROUP_HEADERS.join("|")})\\b[:.\\-—]?\\s*`,
    "i",
  );
  const sections: { header: string | null; body: string }[] = [];
  let current: { header: string | null; body: string[] } = {
    header: null,
    body: [],
  };
  for (const rawLine of text.split("\n")) {
    const m = headerRe.exec(rawLine);
    if (m) {
      if (current.header !== null || current.body.length > 0) {
        sections.push({ header: current.header, body: current.body.join("\n") });
      }
      const canonical = DIGEST_GROUP_HEADERS.find(
        (h) => h.toLowerCase() === m[1].toLowerCase(),
      );
      current = {
        header: canonical ?? m[1],
        body: [rawLine.slice(m[0].length)],
      };
    } else {
      current.body.push(rawLine);
    }
  }
  if (current.header !== null || current.body.length > 0) {
    sections.push({ header: current.header, body: current.body.join("\n") });
  }
  return sections;
}

export type DigestPanelProps = {
  digest: Digest | null;
  /// Whether the Maestro is inert (R-7). When true the panel dims the body and
  /// shows the stale badge even if `digest.stale` isn't set.
  inert?: boolean;
  collapsed?: boolean;
  onToggleCollapsed?: () => void;
  /// Manual `/digest` refresh affordance.
  onRefresh?: () => void;
  refreshing?: boolean;
  /// Optional hard-fact rows from the Desktop's existing workarea state.
  summaryRows?: SummaryRow[];
  /// Chip click handler (D11 next-step chips). Display-only here; the action is
  /// the Maestro's (407/409). Defaults to a no-op so a chip without a handler
  /// still renders + is keyboard-focusable.
  onChipClick?: (chip: MaestroChip) => void;
};

export function DigestPanel({
  digest,
  inert = false,
  collapsed = false,
  onToggleCollapsed,
  onRefresh,
  refreshing = false,
  summaryRows = [],
  onChipClick,
}: DigestPanelProps): JSX.Element {
  const stale = inert || !!digest?.stale;
  const sections = digest ? splitDigestSections(digest.text) : [];

  return (
    <section
      className="border-b border-border bg-surface-2/40"
      data-testid="digest-panel"
      aria-label="Maestro digest"
    >
      <header className="flex items-center justify-between px-3 py-1.5">
        <button
          type="button"
          onClick={onToggleCollapsed}
          className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-muted hover:text-foreground"
          aria-expanded={!collapsed}
        >
          <span>Digest</span>
          {stale && (
            <Badge
              variant="neutral"
              className="text-faint"
              data-testid="stale-badge"
            >
              stale
            </Badge>
          )}
        </button>
        {onRefresh && (
          <Button
            variant="ghost"
            size="sm"
            onClick={onRefresh}
            disabled={refreshing}
            title="Refresh digest (/digest)"
          >
            {refreshing ? "Refreshing…" : "/digest"}
          </Button>
        )}
      </header>

      {!collapsed && (
        <div className="px-3 pb-2">
          {!digest ? (
            <p
              className="py-2 text-sm text-faint"
              data-testid="digest-empty"
            >
              No digest yet. The Concerto chat summarizes your workareas here on
              your return.
            </p>
          ) : (
            <div className={stale ? "opacity-60" : undefined}>
              {sections.length === 0 ? (
                <p className="whitespace-pre-wrap text-sm text-foreground">
                  {digest.text}
                </p>
              ) : (
                <div className="flex flex-col gap-1.5">
                  {sections.map((sec, i) => (
                    <div key={`${sec.header ?? "intro"}-${i}`}>
                      {sec.header && (
                        <span
                          className="mr-1 text-xs font-semibold uppercase text-accent"
                          data-testid={`digest-group-${sec.header
                            .toLowerCase()
                            .replace(/\s+/g, "-")}`}
                        >
                          {sec.header}
                        </span>
                      )}
                      <span className="whitespace-pre-wrap text-sm text-foreground">
                        {sec.body.trim()}
                      </span>
                    </div>
                  ))}
                </div>
              )}

              {summaryRows.length > 0 && (
                <ul
                  className="mt-2 flex flex-col gap-1"
                  data-testid="summary-rows"
                >
                  {summaryRows.map((row) => (
                    <li
                      key={row.workareaId}
                      className="flex items-center gap-2 text-xs"
                    >
                      <span className="font-mono text-muted">
                        @{row.composerName}
                      </span>
                      {row.blanked ? (
                        <span
                          className="italic text-faint"
                          data-testid="blanked-row"
                        >
                          [private workarea, name only]
                        </span>
                      ) : (
                        <>
                          {row.status && (
                            <StatusDot
                              status={workareaStatusToDot(row.status)}
                            />
                          )}
                          {row.branch && (
                            <Badge variant="neutral">{row.branch}</Badge>
                          )}
                        </>
                      )}
                    </li>
                  ))}
                </ul>
              )}

              {digest.chips.length > 0 && (
                <div
                  className="mt-2 flex flex-wrap gap-1.5"
                  data-testid="digest-chips"
                >
                  {digest.chips.map((chip) => (
                    <button
                      key={chip.rule_id}
                      type="button"
                      onClick={() => onChipClick?.(chip)}
                      className="inline-flex items-center gap-1 rounded-full border border-accent/30 bg-accent/10 px-2 py-0.5 text-xs text-accent hover:bg-accent/20 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent"
                    >
                      {chip.title}
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </section>
  );
}
