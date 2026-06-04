// Vitest unit tests for the typed `transport_kind` (Task 218). Pins the
// numeric enum values against the proto ordinals and the remote-transport
// predicate Task 602 branches on.

import { describe, expect, it } from "vitest";

import { isRemoteTransport, TransportKind } from "./runtime";

describe("TransportKind", () => {
  it("matches the proto enum ordinals (FROZEN)", () => {
    expect(TransportKind.Unspecified).toBe(0);
    expect(TransportKind.Uds).toBe(1);
    expect(TransportKind.Iroh).toBe(2);
    expect(TransportKind.WssBridge).toBe(3);
  });

  it("treats Iroh and WSS bridge as remote, UDS as co-located", () => {
    expect(isRemoteTransport(TransportKind.Uds)).toBe(false);
    expect(isRemoteTransport(TransportKind.Unspecified)).toBe(false);
    expect(isRemoteTransport(TransportKind.Iroh)).toBe(true);
    expect(isRemoteTransport(TransportKind.WssBridge)).toBe(true);
  });
});
