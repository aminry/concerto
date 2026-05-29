// Multi-line text input under the session terminal.
//
// Cmd+Enter (or Ctrl+Enter on non-Mac) submits. Plain Enter inserts a
// newline. On submit, the text is encoded as UTF-8 with a trailing
// `\n` and forwarded to `Sessions.SendMessage`.
//
// Task 26 pre-decision (5): hand-rolled `<textarea>` with Tailwind
// classes — no shadcn `Textarea` primitive in V0.1.

import { useCallback, useState } from "react";

import { sendMessage } from "../api/sessions";
import { formatError } from "../api/errors";
import { Button } from "./ui/button";

export type SessionComposerProps = {
  sessionId: string;
  disabled?: boolean;
};

export function SessionComposer({
  sessionId,
  disabled = false,
}: SessionComposerProps): JSX.Element {
  const [value, setValue] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = useCallback(async () => {
    const text = value;
    if (!text || disabled || sending) return;
    setSending(true);
    setError(null);
    try {
      // Terminals transmit CR (\r, 0x0D) on Enter, not LF (\n). Claude's
      // raw-mode TUI only treats CR as "submit", so a trailing \n leaves
      // the text sitting unsubmitted in the input box. Convert every
      // newline (including the terminating one) to CR to mirror what a
      // real terminal sends.
      const wire = `${text}\n`.replace(/\n/g, "\r");
      const bytes = new TextEncoder().encode(wire);
      await sendMessage(sessionId, bytes);
      setValue("");
    } catch (e) {
      setError(formatError(e));
    } finally {
      setSending(false);
    }
  }, [sessionId, value, disabled, sending]);

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
    <div className="border-t border-slate-800 pt-2 pb-1 px-1 flex gap-2 items-end">
      <textarea
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={onKeyDown}
        rows={2}
        placeholder={
          disabled
            ? "Session stopped — input disabled"
            : "Type a message… Cmd+Enter to send"
        }
        disabled={disabled || sending}
        className="flex-1 resize-none bg-slate-900 border border-slate-800 rounded px-2 py-1 text-sm text-slate-100 placeholder:text-slate-500 focus:outline-none focus:ring-1 focus:ring-slate-500 disabled:opacity-50 font-mono"
      />
      <div className="flex flex-col items-end gap-1">
        <Button
          variant="default"
          onClick={() => void submit()}
          disabled={disabled || sending || value.length === 0}
        >
          {sending ? "Sending…" : "Send"}
        </Button>
        {error && (
          <p className="text-xs text-rose-400 max-w-[16rem] truncate">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}
