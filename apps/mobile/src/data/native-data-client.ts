// The native `DataClient` adapter (Task 510, design/16 §3 / design/17 §3.1).
//
// Bridges the OPAQUE-BYTES `ConcertoIroh` native module (Task 509) up to the
// transport-agnostic `DataClient` seam from `@concerto/client` — the SAME seam
// the web client implements (`createConnectWebDataClient`). The screens / data
// facades (Task 513's `WorkspacesClient`, the Inbox, …) program against
// `DataClient` and never see the native module.
//
// How: 509 is a pure byte passthrough keyed on the FULL gRPC path
// "/concerto.v1.Service/Method" (identity codec). connect-es' typed clients
// (`createClient(Service, transport)`) call a `Transport` with a `DescMethod`
// descriptor + a structured message. So this file builds a connect-es
// `Transport` whose `unary`/`stream`:
//   1. derive the gRPC path from the method descriptor,
//   2. `toBinary(method.input, msg)` → request bytes,
//   3. hand the bytes to `module.rpcUnary` / `module.rpcStream`,
//   4. `fromBinary(method.output, bytes)` → typed response.
// The adapter OWNS encode/decode (509 never touches the wire shape). The
// resulting transport is wrapped by `@concerto/client`'s `dataClientFromTransport`,
// so `subscribe(subject, …)` rides the generic `Streams.Subscribe` server-stream
// over the same `stream` path.

import {
  create,
  fromBinary,
  toBinary,
  type DescMessage,
  type DescMethodStreaming,
  type DescMethodUnary,
  type MessageInitShape,
} from "@bufbuild/protobuf";
import type {
  StreamResponse,
  Transport,
  UnaryResponse,
} from "@connectrpc/connect";

import { type DataClient, dataClientFromTransport } from "@concerto/client";

import type { ConcertoIrohModule, StreamEventCallback } from "../native/ConcertoIroh";

/** The fully-qualified gRPC path connect uses, e.g. `/concerto.v1.Streams/Subscribe`. */
function grpcPath(method: {
  parent: { typeName: string };
  name: string;
}): string {
  return `/${method.parent.typeName}/${method.name}`;
}

/**
 * Build a connect-es [`Transport`] over the opaque-bytes [`ConcertoIrohModule`]
 * for one open session `handle`. Unary RPCs go through `rpcUnary`; server
 * streams through `rpcStream`. Client-/bidi-streaming are NOT supported by the
 * native transport (the Core's API surface is unary + server-streaming only).
 */
export function nativeTransport(
  module: ConcertoIrohModule,
  handle: number,
): Transport {
  return {
    async unary<I extends DescMessage, O extends DescMessage>(
      method: DescMethodUnary<I, O>,
      signal: AbortSignal | undefined,
      _timeoutMs: number | undefined,
      _header: HeadersInit | undefined,
      input: MessageInitShape<I>,
    ): Promise<UnaryResponse<I, O>> {
      const reqMsg = create(method.input, input);
      const reqBytes = toBinary(method.input, reqMsg);
      const respBytes = await module.rpcUnary(handle, grpcPath(method), reqBytes);
      if (signal?.aborted) {
        // Honor a late abort: surface the same shape connect would on cancel.
        throw signal.reason ?? new Error("aborted");
      }
      const message = fromBinary(method.output, respBytes);
      return {
        stream: false,
        service: method.parent,
        method,
        header: new Headers(),
        trailer: new Headers(),
        message,
      };
    },

    async stream<I extends DescMessage, O extends DescMessage>(
      method: DescMethodStreaming<I, O>,
      signal: AbortSignal | undefined,
      _timeoutMs: number | undefined,
      _header: HeadersInit | undefined,
      input: AsyncIterable<MessageInitShape<I>>,
    ): Promise<StreamResponse<I, O>> {
      if (method.methodKind !== "server_streaming") {
        throw new Error(
          `native transport supports server-streaming only, got "${method.methodKind}" ` +
            `for ${grpcPath(method)}`,
        );
      }
      // Server-streaming: read the single request message from the iterable.
      let first: MessageInitShape<I> | undefined;
      for await (const m of input) {
        first = m;
        break;
      }
      const reqMsg = create(method.input, first as MessageInitShape<I>);
      const reqBytes = toBinary(method.input, reqMsg);

      // A queue bridging the native callback (push) to the connect async-iterable (pull).
      type Item =
        | { kind: "msg"; bytes: Uint8Array }
        | { kind: "done" }
        | { kind: "err"; message: string };
      const buffer: Item[] = [];
      let resolveNext: ((v: void) => void) | undefined;
      const wake = () => {
        resolveNext?.();
        resolveNext = undefined;
      };

      const callback: StreamEventCallback = {
        onEvent(bytes) {
          buffer.push({ kind: "msg", bytes });
          wake();
        },
        onComplete() {
          buffer.push({ kind: "done" });
          wake();
        },
        onError(message) {
          buffer.push({ kind: "err", message });
          wake();
        },
      };

      const subIdPromise = module.rpcStream(handle, grpcPath(method), reqBytes, callback);

      type Out = StreamResponse<I, O>["message"] extends AsyncIterable<infer T> ? T : never;
      async function* messages(): AsyncGenerator<Out> {
        const subId = await subIdPromise;
        const onAbort = () => {
          module.cancelSubscription(handle, subId);
          buffer.push({ kind: "done" });
          wake();
        };
        if (signal) {
          if (signal.aborted) {
            onAbort();
          } else {
            signal.addEventListener("abort", onAbort, { once: true });
          }
        }
        try {
          for (;;) {
            if (buffer.length === 0) {
              await new Promise<void>((r) => {
                resolveNext = r;
              });
            }
            const item = buffer.shift();
            if (!item) continue;
            if (item.kind === "msg") {
              yield fromBinary(method.output, item.bytes) as Out;
            } else if (item.kind === "done") {
              return;
            } else {
              throw new Error(item.message);
            }
          }
        } finally {
          signal?.removeEventListener("abort", onAbort);
          module.cancelSubscription(handle, subId);
        }
      }

      return {
        stream: true,
        service: method.parent,
        method,
        header: new Headers(),
        trailer: new Headers(),
        // connect's StreamResponse.message is AsyncIterable<MessageShape<O>>;
        // our generator yields decoded output messages.
        message: messages(),
      };
    },
  };
}

/**
 * Build a native [`DataClient`] over an OPEN session `handle` of the
 * [`ConcertoIrohModule`]. `rpc(method, msg)` → `rpcUnary`; `subscribe(subject,…)`
 * rides `Streams.Subscribe` over `rpcStream` (via `dataClientFromTransport`).
 *
 * The caller opens the session first (`module.openSession(blob, cert)`), then
 * passes the returned handle here; `closeSession` is the caller's responsibility
 * (e.g. on app background, per design/16 §3.12).
 */
export function createNativeDataClient(
  module: ConcertoIrohModule,
  handle: number,
): DataClient {
  return dataClientFromTransport(nativeTransport(module, handle));
}
