// The Concerto-chat composer (Task 415). Multi-line input mirroring
// `SessionComposer` (Cmd/Ctrl+Enter submits, plain Enter inserts a newline)
// that calls `sendToMaestro`. It renders the ROUTING AFFORDANCES only:
//
//   - live `@`-token highlighting + a workarea autocomplete sourced from
//     `Workareas.ListWorkareas` (React Query, `composer_name`);
//   - `/`-directive hints (`/digest` `/pause` `/new`);
//   - a one-line preview of the inferred target set.
//
// The composer does NOT parse routing or resolve targets — that is Task 408's
// server-side `pre_parse`. The renderer only AFFORDS the `@`/`/` grammar and
// previews the affected workareas; the authoritative parse happens in the Core
// (design/08 §3.5/§3.8). The draft text is held in the `useMaestroStore`
// UI-only slice so it survives a re-mount while typing.

import { useCallback, useMemo, useState } from "react";

import { Send } from "lucide-react";

import { sendToMaestro } from "../../api/maestro";
import { formatError } from "../../api/errors";
import { useWorkareas } from "../../hooks/useWorkareas";
import { useUiStore } from "../../state/useUiStore";
import { useMaestroStore } from "../../state/useMaestroStore";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";

/// The slash directives the composer affords (design/08 §3.8). The parse is
/// server-side (408); this is display-only autocomplete sugar.
export const SLASH_DIRECTIVES = ["/digest", "/pause", "/new"] as const;

/// The literal fanout tokens (design/08 §3.8) the composer highlights as a
/// routed target even though no workarea name matches them.
export const FANOUT_TOKENS = ["@all", "@idle", "@blocked"] as const;

/// Extract the `@token`s from composer text (no leading-`@`). Pure — used for
/// the live target preview + the autocomplete trigger. Mirrors the design's
/// `@workarea` / `@workarea/session` / `@a,@b` grammar at the token level only;
/// the authoritative parse is 408's.
export function parseAtTokens(text: string): string[] {
  const out: string[] = [];
  const re = /@([\w./-]+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    // `@workarea/session` → the workarea segment is the routing target.
    const tok = m[1].split("/")[0];
    if (tok) out.push(tok);
  }
  return out;
}

/// The trailing `@partial` fragment the user is currently typing (for the
/// autocomplete dropdown), or null if the caret isn't in an `@`-token.
export function trailingAtFragment(text: string): string | null {
  const m = /@([\w./-]*)$/.exec(text);
  return m ? m[1] : null;
}

export type MaestroComposerProps = {
  /// Overridable for tests; defaults to the selected workspace.
  workspaceId?: string | null;
};

export function MaestroComposer({
  workspaceId,
}: MaestroComposerProps): JSX.Element {
  const selectedWorkspaceId = useUiStore((s) => s.selectedWorkspaceId);
  const wsId = workspaceId ?? selectedWorkspaceId;

  const draft = useMaestroStore((s) => s.composerDraft);
  const setDraft = useMaestroStore((s) => s.setComposerDraft);

  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const { data: workareasData } = useWorkareas(wsId);
  const workareas = workareasData?.workareas ?? [];

  const atTokens = useMemo(() => parseAtTokens(draft), [draft]);

  // The inferred target set, for the preview line. A literal fanout token
  // (@all/@idle/@blocked) is surfaced as-is; a named token resolves against
  // the workarea composer names (case-insensitive prefix), display-only.
  const previewTargets = useMemo(() => {
    const names = new Set<string>();
    for (const tok of atTokens) {
      const lit = FANOUT_TOKENS.find((f) => f.slice(1) === tok.toLowerCase());
      if (lit) {
        names.add(lit);
        continue;
      }
      const match = workareas.find(
        (w) => w.composer_name.toLowerCase() === tok.toLowerCase(),
      );
      if (match) names.add(`@${match.composer_name}`);
      else names.add(`@${tok}`);
    }
    return [...names];
  }, [atTokens, workareas]);

  const fragment = trailingAtFragment(draft);
  const suggestions = useMemo(() => {
    if (fragment == null) return [];
    const frag = fragment.toLowerCase();
    return workareas
      .filter((w) => w.composer_name.toLowerCase().startsWith(frag))
      .slice(0, 6);
  }, [fragment, workareas]);

  const slashHint = useMemo(() => {
    if (!draft.startsWith("/")) return null;
    const frag = draft.toLowerCase();
    return SLASH_DIRECTIVES.filter((d) => d.startsWith(frag.split(/\s/)[0]));
  }, [draft]);

  const applySuggestion = useCallback(
    (composerName: string) => {
      // Replace the trailing `@fragment` with the completed `@composer_name `.
      setDraft(draft.replace(/@([\w./-]*)$/, `@${composerName} `));
    },
    [draft, setDraft],
  );

  const submit = useCallback(async () => {
    const text = draft;
    if (!text || sending) return;
    setSending(true);
    setError(null);
    try {
      // V1.0 is text-only (design/08 R-9); attachments is the empty seam.
      await sendToMaestro(text, []);
      setDraft("");
    } catch (e) {
      setError(formatError(e));
    } finally {
      setSending(false);
    }
  }, [draft, sending, setDraft]);

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      const cmdOrCtrl = e.metaKey || e.ctrlKey;
      if (cmdOrCtrl && e.key === "Enter") {
        e.preventDefault();
        void submit();
      }
    },
    [submit],
  );

  return (
    <div className="border-t border-border pt-2 pb-1 px-2">
      {previewTargets.length > 0 && (
        <div
          className="flex flex-wrap items-center gap-1 pb-1 text-xs text-muted"
          data-testid="route-preview"
        >
          <span className="text-faint">Routing to</span>
          {previewTargets.map((t) => (
            <Badge key={t} variant="accent">
              {t}
            </Badge>
          ))}
        </div>
      )}
      {slashHint && slashHint.length > 0 && (
        <div
          className="flex flex-wrap items-center gap-1 pb-1 text-xs text-faint"
          data-testid="slash-hint"
        >
          {slashHint.map((d) => (
            <span key={d} className="font-mono">
              {d}
            </span>
          ))}
        </div>
      )}
      <div className="flex gap-2 items-end">
        <div className="relative flex-1">
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onKeyDown}
            rows={2}
            aria-label="Message the Concerto chat"
            placeholder="Message the Concerto chat… @workarea to route, /digest for a summary. Cmd+Enter to send"
            disabled={sending}
            className="w-full resize-none bg-surface border border-border rounded-md px-2 py-1 text-sm text-foreground placeholder:text-faint focus:outline-none focus-visible:ring-2 focus-visible:ring-accent disabled:opacity-50"
          />
          {suggestions.length > 0 && (
            <ul
              data-testid="workarea-autocomplete"
              className="absolute z-10 bottom-full mb-1 w-full max-h-40 overflow-auto rounded-md border border-border bg-surface-2 text-sm shadow-md"
            >
              {suggestions.map((w) => (
                <li key={w.id}>
                  <button
                    type="button"
                    onMouseDown={(e) => {
                      // mouseDown so the textarea doesn't blur+swallow it.
                      e.preventDefault();
                      applySuggestion(w.composer_name);
                    }}
                    className="block w-full px-2 py-1 text-left hover:bg-raised"
                  >
                    <span className="font-mono text-accent">
                      @{w.composer_name}
                    </span>
                    <span className="ml-2 text-xs text-faint">{w.status}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
        <div className="flex flex-col items-end gap-1">
          <Button
            variant="primary"
            onClick={() => void submit()}
            disabled={sending || draft.length === 0}
          >
            <Send size={14} />
            {sending ? "Sending…" : "Send"}
          </Button>
          <span className="text-xs text-faint">⌘+Enter</span>
        </div>
      </div>
      {error && (
        <p className="pt-1 text-xs text-err whitespace-normal break-words">
          {error}
        </p>
      )}
    </div>
  );
}
