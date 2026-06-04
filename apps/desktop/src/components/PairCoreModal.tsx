// "Pair with a remote Core" modal — the split-host pairing flow
// (`design/15 §3.10.3` / `design/12 §3.3`).
//
// Three steps, driven entirely through the shell's pairing Tauri commands
// (`src/api/cores.ts`); the renderer never speaks gRPC or runs Noise XX:
//
//   1. choose — Scan QR (webcam) or Paste token. Paste-token is the
//      always-available path; scan is the convenience path and falls back to
//      paste when the webcam is unavailable (no camera entitlement, denied
//      permission, no device).
//   2. pairing — decode the payload, surface the 60s-TTL countdown, call
//      `complete_pairing_from_payload` (Noise XX + `Devices.CompletePairing` +
//      the `PairedCore` write happen in the shell), show progress/errors.
//   3. name — pre-fill the suggested name (the Core hostname from
//      `GetCoreInfo`), let the user edit, persist via `rename_paired_core`, set
//      the new Core active, invalidate `["cores"]`, close.

import { useEffect, useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { BrowserQRCodeReader } from "@zxing/browser";

import {
  completePairingFromPayload,
  decodePairingPayload,
  renamePairedCore,
  setActiveCore,
  type CompletePairingResult,
} from "../api/cores";
import { formatError } from "../api/errors";
import { useUiStore } from "../state/useUiStore";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";
import { Input } from "./ui/input";

type Step = "choose" | "scan" | "paste" | "naming";

export function PairCoreModal(): JSX.Element {
  const open = useUiStore((s) => s.pairingOpen);
  const setOpen = useUiStore((s) => s.setPairingOpen);
  const queryClient = useQueryClient();

  const [step, setStep] = useState<Step>("choose");
  const [token, setToken] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [paired, setPaired] = useState<CompletePairingResult | null>(null);
  const [name, setName] = useState("");

  // Reset to the entry step every time the modal opens.
  useEffect(() => {
    if (open) {
      setStep("choose");
      setToken("");
      setError(null);
      setPaired(null);
      setName("");
    }
  }, [open]);

  // Drive the pairing ceremony from a decoded token.
  const pairMutation = useMutation({
    mutationFn: async (raw: string): Promise<CompletePairingResult> => {
      // Renderer-side validation first (clear message before hitting the
      // shell); the shell re-validates TTL / one-shot authoritatively.
      decodePairingPayload(raw);
      return completePairingFromPayload(raw);
    },
    onSuccess: (result) => {
      setPaired(result);
      setName(result.suggested_name);
      setError(null);
      setStep("naming");
    },
    onError: (e) => {
      setError(formatError(e));
    },
  });

  // Persist the chosen name + set the new Core active.
  const finishMutation = useMutation({
    mutationFn: async () => {
      if (!paired) throw new Error("no paired Core");
      const finalName = name.trim() || paired.suggested_name;
      await renamePairedCore(paired.core_id, finalName);
      await setActiveCore(paired.core_id);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["cores"] });
      setOpen(false);
    },
    onError: (e) => {
      setError(formatError(e));
    },
  });

  function submitToken(raw: string): void {
    setError(null);
    setStep("paste");
    setToken(raw);
    pairMutation.mutate(raw);
  }

  return (
    <Dialog
      open={open}
      onClose={() => setOpen(false)}
      title="Pair with a remote Core"
    >
      {step === "choose" && (
        <ChooseStep
          onScan={() => {
            setError(null);
            setStep("scan");
          }}
          onPaste={() => {
            setError(null);
            setStep("paste");
          }}
        />
      )}

      {step === "scan" && (
        <ScanStep
          onDecoded={submitToken}
          onFallback={() => setStep("paste")}
          onBack={() => setStep("choose")}
        />
      )}

      {step === "paste" && (
        <PasteStep
          token={token}
          setToken={setToken}
          pending={pairMutation.isPending}
          error={error}
          onSubmit={submitToken}
          onBack={() => {
            pairMutation.reset();
            setError(null);
            setStep("choose");
          }}
        />
      )}

      {step === "naming" && paired && (
        <NamingStep
          name={name}
          setName={setName}
          suggested={paired.suggested_name}
          pending={finishMutation.isPending}
          error={error}
          onSubmit={() => finishMutation.mutate()}
        />
      )}
    </Dialog>
  );
}

function ChooseStep({
  onScan,
  onPaste,
}: {
  onScan: () => void;
  onPaste: () => void;
}): JSX.Element {
  return (
    <div className="space-y-3">
      <p className="text-xs text-muted">
        On the Core machine, run{" "}
        <code className="font-mono text-foreground">concerto pair</code> (or use
        its tray menu) to show a QR / token. Then:
      </p>
      <div className="flex flex-col gap-2">
        <Button variant="outline" onClick={onScan}>
          Scan QR with camera
        </Button>
        <Button variant="primary" onClick={onPaste}>
          Paste token
        </Button>
      </div>
      <p className="text-[11px] text-faint">
        No camera? Paste-token always works. The pairing code expires 60s after
        it's shown.
      </p>
    </div>
  );
}

function ScanStep({
  onDecoded,
  onFallback,
  onBack,
}: {
  onDecoded: (token: string) => void;
  onFallback: () => void;
  onBack: () => void;
}): JSX.Element {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [cameraError, setCameraError] = useState<string | null>(null);

  useEffect(() => {
    const reader = new BrowserQRCodeReader();
    let stopped = false;
    let controls: { stop: () => void } | null = null;

    void reader
      .decodeFromVideoDevice(undefined, videoRef.current ?? undefined, (result) => {
        if (stopped) return;
        if (result) {
          stopped = true;
          controls?.stop();
          onDecoded(result.getText());
        }
      })
      .then((c) => {
        if (stopped) {
          c.stop();
        } else {
          controls = c;
        }
      })
      .catch((e: unknown) => {
        // No camera entitlement / permission denied / no device → graceful
        // fallback to the always-available paste path (`design/15` impl note).
        setCameraError(formatError(e));
      });

    return () => {
      stopped = true;
      controls?.stop();
    };
  }, [onDecoded]);

  return (
    <div className="space-y-3">
      {cameraError ? (
        <div className="space-y-2">
          <p className="text-xs text-warn" role="alert">
            Camera unavailable — {cameraError}
          </p>
          <Button variant="primary" onClick={onFallback}>
            Paste a token instead
          </Button>
        </div>
      ) : (
        <>
          {/* eslint-disable-next-line jsx-a11y/media-has-caption */}
          <video
            ref={videoRef}
            className="aspect-square w-full rounded-md border border-border bg-black"
            aria-label="Camera viewfinder for pairing QR"
          />
          <p className="text-xs text-muted">
            Point the camera at the pairing QR on the Core machine.
          </p>
        </>
      )}
      <Button variant="ghost" onClick={onBack}>
        Back
      </Button>
    </div>
  );
}

function PasteStep({
  token,
  setToken,
  pending,
  error,
  onSubmit,
  onBack,
}: {
  token: string;
  setToken: (v: string) => void;
  pending: boolean;
  error: string | null;
  onSubmit: (token: string) => void;
  onBack: () => void;
}): JSX.Element {
  return (
    <form
      className="space-y-3"
      onSubmit={(e) => {
        e.preventDefault();
        if (pending) return;
        onSubmit(token);
      }}
    >
      <div>
        <label className="mb-1 block text-xs uppercase tracking-wider text-faint">
          Pairing token
        </label>
        <textarea
          value={token}
          onChange={(e) => setToken(e.target.value)}
          placeholder="Paste the base64 token from `concerto pair`"
          aria-label="Pairing token"
          autoFocus
          className="h-24 w-full resize-none rounded-md border border-border-strong bg-background px-2.5 py-1.5 font-mono text-xs text-foreground placeholder:text-faint focus:outline-none focus-visible:ring-2 focus-visible:ring-accent"
        />
      </div>
      {pending && (
        <p className="text-xs text-muted">
          Pairing… completing the secure handshake with the Core.
        </p>
      )}
      {error && (
        <p className="text-xs text-err" role="alert">
          {error}
        </p>
      )}
      <div className="flex justify-between pt-1">
        <Button type="button" variant="ghost" onClick={onBack} disabled={pending}>
          Back
        </Button>
        <Button type="submit" variant="primary" disabled={pending || !token.trim()}>
          {pending ? "Pairing…" : "Pair"}
        </Button>
      </div>
    </form>
  );
}

function NamingStep({
  name,
  setName,
  suggested,
  pending,
  error,
  onSubmit,
}: {
  name: string;
  setName: (v: string) => void;
  suggested: string;
  pending: boolean;
  error: string | null;
  onSubmit: () => void;
}): JSX.Element {
  return (
    <form
      className="space-y-3"
      onSubmit={(e) => {
        e.preventDefault();
        if (pending) return;
        onSubmit();
      }}
    >
      <p className="text-xs text-ok">Paired. Give this Core a name.</p>
      <div>
        <label className="mb-1 block text-xs uppercase tracking-wider text-faint">
          Name
        </label>
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={suggested}
          aria-label="Name"
          autoFocus
        />
        <p className="mt-1 text-[11px] text-faint">
          Suggested: {suggested} (the Core's hostname).
        </p>
      </div>
      {error && (
        <p className="text-xs text-err" role="alert">
          {error}
        </p>
      )}
      <div className="flex justify-end pt-1">
        <Button type="submit" variant="primary" disabled={pending}>
          {pending ? "Saving…" : "Connect"}
        </Button>
      </div>
    </form>
  );
}
