import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

// V0.1 Tauri shell smoke surface. The renderer NEVER speaks gRPC or
// the network directly: every interaction with the Core flows through
// the `concerto_rpc` Tauri command (see src-tauri/src/commands.rs).
// Phase 2 (Task 24+) replaces this scratch screen with shadcn/ui +
// Zustand + React Query against the real workspace surface.

type RpcResult = { kind: "idle" } | { kind: "ok"; payload: unknown } | { kind: "err"; message: string };

const PING_INITIAL: { kind: "idle" } | { kind: "ok"; value: string } | { kind: "err"; message: string } = {
  kind: "idle",
};

function App(): JSX.Element {
  const [rpc, setRpc] = useState<RpcResult>({ kind: "idle" });
  const [ping, setPing] = useState<typeof PING_INITIAL>(PING_INITIAL);
  const [busy, setBusy] = useState(false);

  async function onConnect(): Promise<void> {
    setBusy(true);
    try {
      const payload = await invoke<unknown>("concerto_rpc", {
        method: "Runtime.GetServerCapabilities",
        payload: {},
      });
      setRpc({ kind: "ok", payload });
    } catch (e: unknown) {
      setRpc({ kind: "err", message: String(e) });
    } finally {
      setBusy(false);
    }
  }

  async function onPing(): Promise<void> {
    try {
      const value = await invoke<string>("concerto_ping");
      setPing({ kind: "ok", value });
    } catch (e: unknown) {
      setPing({ kind: "err", message: String(e) });
    }
  }

  return (
    <main className="min-h-screen bg-slate-950 text-slate-100 p-8 font-mono">
      <h1 className="text-2xl font-semibold mb-4">Concerto — Desktop Shell</h1>
      <p className="text-slate-400 mb-6">
        V0.1 wire-only scaffold. Click "Connect" to call{" "}
        <code className="text-slate-200">Runtime.GetServerCapabilities</code> over UDS.
      </p>

      <div className="flex gap-3 mb-6">
        <button
          type="button"
          className="px-4 py-2 rounded bg-slate-700 hover:bg-slate-600 disabled:opacity-50"
          onClick={onConnect}
          disabled={busy}
        >
          {busy ? "Connecting…" : "Connect"}
        </button>
        <button
          type="button"
          className="px-4 py-2 rounded bg-slate-800 hover:bg-slate-700"
          onClick={onPing}
        >
          Ping IPC
        </button>
      </div>

      <section className="mb-6">
        <h2 className="text-sm uppercase tracking-wider text-slate-400 mb-2">concerto_ping</h2>
        {ping.kind === "idle" && <p className="text-slate-500">(not run)</p>}
        {ping.kind === "ok" && <p className="text-emerald-400">{ping.value}</p>}
        {ping.kind === "err" && <p className="text-rose-400">{ping.message}</p>}
      </section>

      <section>
        <h2 className="text-sm uppercase tracking-wider text-slate-400 mb-2">
          Runtime.GetServerCapabilities
        </h2>
        {rpc.kind === "idle" && <p className="text-slate-500">(not run)</p>}
        {rpc.kind === "err" && <pre className="text-rose-400 whitespace-pre-wrap">{rpc.message}</pre>}
        {rpc.kind === "ok" && (
          <pre className="bg-slate-900 border border-slate-800 rounded p-4 overflow-auto text-emerald-300">
            {JSON.stringify(rpc.payload, null, 2)}
          </pre>
        )}
      </section>
    </main>
  );
}

export default App;
