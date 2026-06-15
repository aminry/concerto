//! The transport-agnostic Concerto data client (Task 507.5, design/17 §3.1).
//
// `DataClient` is the one seam both clients implement: the web client (gRPC-Web
// over the Core's connect-web bridge, default binary framing — D10) and, later,
// the desktop/native adapters. It exposes a connect-es `Transport` (so callers
// make typed unary + server-streaming calls via `createClient(Service,
// dc.transport)`) plus a convenience `subscribe` over the `Streams.Subscribe`
// server stream. Auth headers / TLS pinning / relay routing are layered onto the
// transport by Tasks 520–522.

import { createClient, type Transport } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";

import type { Event } from "./gen/concerto/v1/streams_pb";
import { Streams } from "./gen/concerto/v1/streams_pb";

/** Handle returned by [`DataClient.subscribe`]; call to end the subscription. */
export type Unsubscribe = () => void;

/** The transport-agnostic data client (design/17 §3.1). */
export interface DataClient {
  /** The connect-es transport — use `createClient(Service, transport)` for typed RPCs. */
  readonly transport: Transport;
  /**
   * Subscribe to a streams subject (the `Streams.Subscribe` server stream);
   * `onEvent` fires per [`Event`] frame. Returns an unsubscribe that aborts the
   * stream. Errors after the first frame go to `onError` (unless unsubscribed).
   */
  subscribe(
    subject: string,
    onEvent: (ev: Event) => void,
    onError?: (err: unknown) => void,
  ): Unsubscribe;
}

/** Options for [`createConnectWebDataClient`]. */
export interface ConnectWebOptions {
  /** Base URL of the Core's connect-web bridge, e.g. `http://127.0.0.1:<port>`. */
  baseUrl: string;
  /** Optional `fetch` override (Tasks 521/522 inject auth + TLS-pinned fetch). */
  fetch?: typeof globalThis.fetch;
}

/**
 * Build a web [`DataClient`] over the Core's gRPC-Web bridge (binary framing,
 * D10 — no JSON-casing mismatch). The bridge is Task 204; auth/TLS/pairing are
 * layered by 520–522.
 */
export function createConnectWebDataClient(opts: ConnectWebOptions): DataClient {
  const transport = createGrpcWebTransport({
    baseUrl: opts.baseUrl,
    ...(opts.fetch ? { fetch: opts.fetch } : {}),
  });
  return dataClientFromTransport(transport);
}

/**
 * Wrap any connect-es [`Transport`] into a [`DataClient`]. Shared by the web
 * client and the future Tauri/native adapters (which supply a custom transport
 * routing through Tauri `invoke` / the `ConcertoIroh` native module).
 */
export function dataClientFromTransport(transport: Transport): DataClient {
  return {
    transport,
    subscribe(subject, onEvent, onError) {
      const ac = new AbortController();
      const streams = createClient(Streams, transport);
      void (async () => {
        try {
          for await (const ev of streams.subscribe({ subject }, { signal: ac.signal })) {
            onEvent(ev);
          }
        } catch (err) {
          if (!ac.signal.aborted) {
            onError?.(err);
          }
        }
      })();
      return () => ac.abort();
    },
  };
}
