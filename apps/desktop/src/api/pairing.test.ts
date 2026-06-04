// Vitest unit tests for the pairing payload-decode envelope + the pairing
// command bindings (Task 219). Node-env: these mock `@tauri-apps/api/core`'s
// `invoke` and exercise the pure decode/encode helpers — no DOM needed.
//
// The decoded envelope shape is FROZEN (`design/12 §3.3`): it must match what
// `concerto pair` (Task 713) emits — `base64({core_pubkey, pairing_token,
// lan_endpoint/iroh_endpoint_id, relay_hint})`.

import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import {
  completePairingFromPayload,
  decodePairingPayload,
  encodePairingPayload,
  removePairedCore,
  renamePairedCore,
  startPairingShow,
  type PairingPayload,
} from "./cores";

const samplePayload: PairingPayload = {
  core_pubkey: "Y29yZS1wdWJrZXk=",
  pairing_token: "dG9rZW4tMzItYnl0ZXM=",
  lan_endpoint: "192.168.1.42:7777",
  iroh_endpoint_id: "iroh-endpoint-abc",
  relay_hint: "https://relay.example",
};

function fixtureToken(p: Partial<PairingPayload> = {}): string {
  return encodePairingPayload({ ...samplePayload, ...p });
}

beforeEach(() => {
  invoke.mockReset();
});

describe("decodePairingPayload", () => {
  it("round-trips a base64 JSON envelope (`concerto pair` shape)", () => {
    const token = encodePairingPayload(samplePayload);
    const decoded = decodePairingPayload(token);
    expect(decoded.core_pubkey).toBe(samplePayload.core_pubkey);
    expect(decoded.pairing_token).toBe(samplePayload.pairing_token);
    expect(decoded.lan_endpoint).toBe("192.168.1.42:7777");
    expect(decoded.iroh_endpoint_id).toBe("iroh-endpoint-abc");
    expect(decoded.relay_hint).toBe("https://relay.example");
  });

  it("accepts an iroh-only envelope (no lan_endpoint)", () => {
    const token = encodePairingPayload({
      core_pubkey: "cHVi",
      pairing_token: "dG9r",
      iroh_endpoint_id: "iroh-xyz",
    });
    const decoded = decodePairingPayload(token);
    expect(decoded.iroh_endpoint_id).toBe("iroh-xyz");
    expect(decoded.lan_endpoint).toBeNull();
  });

  it("trims surrounding whitespace", () => {
    const token = `  ${fixtureToken()}\n`;
    expect(() => decodePairingPayload(token)).not.toThrow();
  });

  it("rejects an empty string with a helpful message", () => {
    expect(() => decodePairingPayload("   ")).toThrow(/Paste the pairing token/);
  });

  it("rejects non-base64 garbage", () => {
    expect(() => decodePairingPayload("!!!not base64!!!")).toThrow(
      /invalid base64/,
    );
  });

  it("rejects base64 that is not JSON", () => {
    expect(() => decodePairingPayload(btoa("hello world"))).toThrow(/not JSON/);
  });

  it("rejects a JSON envelope missing core_pubkey", () => {
    const token = btoa(JSON.stringify({ pairing_token: "x" }));
    expect(() => decodePairingPayload(token)).toThrow(/Core public key/);
  });

  it("rejects a JSON envelope missing pairing_token", () => {
    const token = btoa(JSON.stringify({ core_pubkey: "x" }));
    expect(() => decodePairingPayload(token)).toThrow(/pairing secret/);
  });
});

describe("pairing command bindings", () => {
  it("startPairingShow invokes the frozen command name", async () => {
    invoke.mockResolvedValueOnce(samplePayload);
    const p = await startPairingShow();
    expect(invoke).toHaveBeenCalledWith("start_pairing_show");
    expect(p.core_pubkey).toBe(samplePayload.core_pubkey);
  });

  it("completePairingFromPayload forwards the raw token + returns id/name", async () => {
    invoke.mockResolvedValueOnce({
      core_id: "deadbeef",
      suggested_name: "workstation.local",
    });
    const result = await completePairingFromPayload("the-token");
    expect(invoke).toHaveBeenCalledWith("complete_pairing_from_payload", {
      token: "the-token",
    });
    expect(result.core_id).toBe("deadbeef");
    expect(result.suggested_name).toBe("workstation.local");
  });

  it("renamePairedCore forwards camelCase args", async () => {
    invoke.mockResolvedValueOnce(undefined);
    await renamePairedCore("deadbeef", "Home workstation");
    expect(invoke).toHaveBeenCalledWith("rename_paired_core", {
      coreId: "deadbeef",
      displayName: "Home workstation",
    });
  });

  it("removePairedCore forwards the core id", async () => {
    invoke.mockResolvedValueOnce(undefined);
    await removePairedCore("deadbeef");
    expect(invoke).toHaveBeenCalledWith("remove_paired_core", {
      coreId: "deadbeef",
    });
  });
});
