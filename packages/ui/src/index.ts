//! `@concerto/ui` — the shared, transport-agnostic Concerto React-DOM renderer
//! (Task 523, decision D11). Consumed by `apps/web` (the connect-web client) and
//! `apps/desktop` (the Tauri shell); mobile builds its own RN tree (D11).
//!
//! - [`Inbox`] is the notifications-inbox component. The host owns the connection
//!   + data fetching and passes the notification list + handlers + load state as
//!   props (see [`InboxProps`]), so the component stays transport-agnostic.
//! - Co-located styling lives in `@concerto/ui/inbox.css` — the consumer imports
//!   it once (web inherits it; desktop scopes it under its own surface).

export { Inbox, NotificationCard, relativeTime, kindLabel } from "./Inbox";
export type { InboxProps, InboxStatus } from "./Inbox";
