// Vitest unit tests for the connected-Core registry binding (Task 218).
// Mocks `@tauri-apps/api/core`'s `invoke` so the binding shape + the command
// names/args are pinned without a running Tauri shell.

import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import {
  getActiveCore,
  listPairedCores,
  setActiveCore,
  type PairedCore,
} from "./cores";

const sampleCore: PairedCore = {
  core_id: "abc123",
  display_name: "Home workstation",
  transport_kind: "iroh",
  iroh_endpoint_id: "endpoint-xyz",
  last_connected_at: 1717459200,
  is_active: true,
};

const localCore: PairedCore = {
  core_id: "local-machine",
  display_name: "This machine",
  transport_kind: "uds",
  iroh_endpoint_id: null,
  last_connected_at: null,
  is_active: false,
};

beforeEach(() => {
  invoke.mockReset();
});

describe("cores binding", () => {
  it("listPairedCores invokes the list command and returns the rows", async () => {
    invoke.mockResolvedValueOnce([sampleCore, localCore]);
    const cores = await listPairedCores();
    expect(invoke).toHaveBeenCalledWith("list_paired_cores");
    expect(cores).toHaveLength(2);
    expect(cores[0].transport_kind).toBe("iroh");
    expect(cores[1].transport_kind).toBe("uds");
    // The shape the picker reads: cleartext metadata, no secret fields.
    expect(cores[0]).not.toHaveProperty("device_cert");
    expect(cores[0]).not.toHaveProperty("core_pubkey");
  });

  it("getActiveCore returns the active row", async () => {
    invoke.mockResolvedValueOnce(sampleCore);
    const active = await getActiveCore();
    expect(invoke).toHaveBeenCalledWith("get_active_core");
    expect(active?.core_id).toBe("abc123");
    expect(active?.is_active).toBe(true);
  });

  it("getActiveCore returns null when no Core is active", async () => {
    invoke.mockResolvedValueOnce(null);
    const active = await getActiveCore();
    expect(active).toBeNull();
  });

  it("setActiveCore forwards the coreId arg in the Tauri camelCase shape", async () => {
    invoke.mockResolvedValueOnce(undefined);
    await setActiveCore("abc123");
    expect(invoke).toHaveBeenCalledWith("set_active_core", { coreId: "abc123" });
  });
});
