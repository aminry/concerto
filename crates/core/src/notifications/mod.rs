//! Notifications & Push sub-system (design/14, sub-system 14).
//!
//! Phase-5 Track A. This module is the in-process home of the notification
//! model, the inbox/de-dup engine, the multi-device push fan-out, the chip
//! dispatcher, and the `PushBackend` seam. It is built contract-first:
//!
//! - **501 (this task)** freezes the domain [`model`] (kinds/subjects/severity +
//!   the DB ⇄ proto mapping + [`model::NotifyRequest`]) over the `notifications`
//!   / `notification_deliveries` tables (migration 0017) and the
//!   `notifications.proto` messages. No actor/handle/service yet.
//! - **502** adds the inbox feed + 5-min de-dup window + retention.
//! - **503** adds `PushBackend` + `ExpoPushBackend` + `MockPushBackend` + the
//!   ID-only `WakeupPayload` shape.
//! - **504/505** add the multi-device fan-out + first-wins + `ActOnChip`.
//! - **507** adds the `NotificationHandle` + the `Notifications` gRPC service +
//!   the `notification.events` subject + the live `notify_user` sink.
//!
//! Each later task adds its own `pub mod X;` line below in a distinct region
//! (additive; auto-merges on rebase).

pub mod dedup;
pub mod fanout;
pub mod model;
pub mod push;
