// Multi-line text input under the session terminal.
//
// Cmd+Enter (or Ctrl+Enter on non-Mac) submits. Plain Enter inserts a
// newline. On submit, the text is encoded as UTF-8 with a trailing
// carriage return `\r` and forwarded to `Sessions.SendMessage`. The `\r`
// (not `\n`) is the "Enter" keypress: interactive agent TUIs (Claude
// Code, Codex) run the PTY in raw mode and submit a turn on CR, whereas a
// bare LF is treated as a newline *inside* the prompt editor — so sending
// `\n` types the message but never submits it.
//
// Task 26 pre-decision (5): hand-rolled `<textarea>` with Tailwind
// classes — no shadcn `Textarea` primitive in V0.1.

import { useCallback, useState } from "react";

import { sendMessage } from "../api/sessions";
import { formatError } from "../api/errors";
import { Button } from "./ui/button";
import { Send } from "lucide-react";

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
    <div className="border-t border-border pt-2 pb-1 px-1 flex gap-2 items-end">
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
        className="flex-1 resize-none bg-surface border border-border rounded-md px-2 py-1 text-sm text-foreground placeholder:text-faint focus:outline-none focus-visible:ring-2 focus-visible:ring-accent disabled:opacity-50 font-mono"
      />
      <div className="flex flex-col items-end gap-1">
        <Button
          variant="primary"
          onClick={() => void submit()}
          disabled={disabled || sending || value.length === 0}
        >
          <Send size={14} />
          {sending ? "Sending…" : "Send"}
        </Button>
        <span className="text-xs text-faint">⌘+Enter</span>
        {error && (
          <p className="text-xs text-err max-w-[20rem] whitespace-normal break-words text-right">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}
