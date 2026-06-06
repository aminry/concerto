-- Migration 0013: webhook_deliveries — restart-surviving delivery-id idempotency
-- (Task 315, design/13 §6.2, PHASE3_PLANNING §3/D9).
--
-- The Core dedupes inbound GitHub webhooks on the `X-GitHub-Delivery` id BEFORE
-- HMAC + parse (design/13 §6.2 ordering). The dedup must survive a Core restart
-- (a GitHub redelivery seconds after a Core bounce is still a replay), so it is a
-- persisted table, not an in-memory cache. The 1h TTL (design/13 §6.2) is the
-- prune window, swept by `webhook_deliveries::prune_expired`.
--
-- Secrets never touch this table — the per-repo HMAC secret lives ONLY in the
-- keychain (VcsSecretSlot::WebhookSecret, D4). This table holds only the
-- delivery id, the repo it targeted, and when it was received.
CREATE TABLE webhook_deliveries (
    delivery_id TEXT PRIMARY KEY,        -- the X-GitHub-Delivery UUID (idempotency key)
    repo_id     TEXT NOT NULL,           -- the repository this delivery targeted
    received_at INTEGER NOT NULL         -- epoch ms when first ingested (drives the TTL prune)
);

-- The prune sweep deletes rows older than the TTL window by `received_at`.
CREATE INDEX idx_webhook_deliveries_received_at ON webhook_deliveries (received_at);
