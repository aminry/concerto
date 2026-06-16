//! `@concerto/client` — the shared, transport-agnostic Concerto data layer
//! (Task 507.5). Consumed by `apps/web` (519/520), `apps/mobile` (508/510), and
//! eventually `apps/desktop`.
//!
//! - The [`DataClient`] seam + [`createConnectWebDataClient`] live here.
//! - Generated proto types + service descriptors live under `./gen/...` and are
//!   imported via the `@concerto/client/gen/concerto/v1/<file>_pb` subpath,
//!   e.g. `import { Notifications } from "@concerto/client/gen/concerto/v1/notifications_pb"`.
//! - `createClient` (connect-es) is re-exported so callers build typed service
//!   clients: `createClient(Notifications, dc.transport)`.

export * from "./data-client";
export * from "./notifications";

export { createClient } from "@connectrpc/connect";
export type { Client, Transport } from "@connectrpc/connect";
