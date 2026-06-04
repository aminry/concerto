// "Reveal pairing QR" — the co-located QR-show affordance (`design/15 §3.10.3`
// / §3.11).
//
// Renders the *local* Core's pairing payload (from `Devices.StartPairing` via
// the shell's `start_pairing_show` command) as a QR image plus the raw base64
// token, so another device (phone / web / a second Desktop) can pair by
// scanning or pasting.
//
// **UDS-gated.** Per `design/15 §3.11`, "Reveal pairing QR" is *disabled* in
// split-host mode — there's no local Core on this machine to pair another
// device against. The parent gates rendering on the active Core's
// `transport_kind`; this component additionally renders the split-host hint
// when handed a non-UDS transport, so it never silently shows a stale/empty QR.
//
// The token has a 60s TTL (`design/12 §3.3`); a countdown is shown and the QR
// can be regenerated on expiry by re-invoking `start_pairing_show`.

import { useEffect, useRef, useState } from "react";
import QRCode from "qrcode";

import {
  encodePairingPayload,
  startPairingShow,
  type PairingPayload,
} from "../api/cores";
import { TransportKind } from "../api/runtime";
import { formatError } from "../api/errors";
import { Button } from "./ui/button";

/// The pairing token's TTL, surfaced as a countdown (`design/12 §3.3`).
const TOKEN_TTL_SECONDS = 60;

export function ShowPairingQr({
  transportKind,
}: {
  /// The active Core's transport. The QR-show is only meaningful for a
  /// co-located UDS Core; any remote transport renders the hint instead.
  transportKind: TransportKind;
}): JSX.Element {
  if (transportKind !== TransportKind.Uds) {
    return (
      <div className="rounded-md border border-border bg-surface-2 px-3 py-2 text-xs text-muted">
        Revealing a pairing QR is only available on the Core machine. To pair
        another device with a remote Core, use that machine's tray menu or run{" "}
        <code className="font-mono text-foreground">concerto pair</code> there.
      </div>
    );
  }
  return <LocalQr />;
}

function LocalQr(): JSX.Element {
  const [payload, setPayload] = useState<PairingPayload | null>(null);
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [secondsLeft, setSecondsLeft] = useState(TOKEN_TTL_SECONDS);
  // Guards against a late `start_pairing_show` resolving after unmount.
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  async function reveal(): Promise<void> {
    setLoading(true);
    setError(null);
    try {
      const p = await startPairingShow();
      if (!mountedRef.current) return;
      const url = await QRCode.toDataURL(encodePairingPayload(p), {
        margin: 1,
        width: 220,
      });
      if (!mountedRef.current) return;
      setPayload(p);
      setDataUrl(url);
      setSecondsLeft(TOKEN_TTL_SECONDS);
    } catch (e) {
      if (mountedRef.current) setError(formatError(e));
    } finally {
      if (mountedRef.current) setLoading(false);
    }
  }

  // Tick the TTL countdown while a payload is live.
  useEffect(() => {
    if (!payload) return;
    if (secondsLeft <= 0) return;
    const t = setTimeout(() => setSecondsLeft((s) => s - 1), 1000);
    return () => clearTimeout(t);
  }, [payload, secondsLeft]);

  const expired = payload !== null && secondsLeft <= 0;

  return (
    <div className="space-y-3">
      {!payload && (
        <Button variant="outline" onClick={() => void reveal()} disabled={loading}>
          {loading ? "Generating…" : "Reveal pairing QR"}
        </Button>
      )}
      {error && <p className="text-xs text-err">{error}</p>}
      {payload && dataUrl && (
        <div className="space-y-2">
          <img
            src={dataUrl}
            alt="Pairing QR code"
            className={`rounded-md border border-border bg-white p-2 ${
              expired ? "opacity-30" : ""
            }`}
            width={220}
            height={220}
          />
          {expired ? (
            <p className="text-xs text-warn" role="alert">
              This pairing code expired. Generate a new one to pair a device.
            </p>
          ) : (
            <p className="text-xs text-muted">
              Scan this from the other device, or paste the token below. Expires
              in {secondsLeft}s.
            </p>
          )}
          <textarea
            readOnly
            value={encodePairingPayload(payload)}
            aria-label="Pairing token"
            className="h-20 w-full resize-none rounded-md border border-border-strong bg-background px-2 py-1.5 font-mono text-[10px] text-muted"
          />
          <Button variant="ghost" onClick={() => void reveal()} disabled={loading}>
            {loading ? "Generating…" : "Regenerate"}
          </Button>
        </div>
      )}
    </div>
  );
}
