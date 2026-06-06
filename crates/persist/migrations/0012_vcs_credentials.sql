-- Migration 0012 — Task 313: VCS credential metadata (`design/13 §4`,
-- `tasks/v1.0/PHASE3_PLANNING.md §3` reserved number 0012).
--
-- Stores the **non-secret** metadata the Core needs to decide *which* VCS
-- credential to use and *when* to refresh it. The secret material itself —
-- GitHub App private keys, webhook HMAC secrets, Linear/Jira OAuth tokens —
-- NEVER lands here: it lives only in the OS keychain via the parameterized
-- `VcsSecretSlot` accessor (`Secrets::{get,set,delete}_vcs_secret`, account
-- `vcs.<scope_id>.<slot_slug>`), per locked decision D4. A reviewer can `grep`
-- this table and find zero key/token columns — that is the invariant.
--
-- `scope_id` mirrors the keychain `scope_id` so a row here points 1:1 at the
-- secret(s) in the keychain: the GitHub App id (App auth), the repo id (webhook
-- secret), or the provider account id (Linear/Jira OAuth account).
--
-- The existing GitHub PAT path (`SecretKind::GithubPat`, the V0.1 `gh` shell-out)
-- needs no row here — it is a singleton keychain entry with no metadata. This
-- table is for the V1.0 multi-credential classes (App / Linear / Jira) that
-- 314/317 populate.
--
-- `token_expires_at` (epoch ms, nullable) lets the Core proactively refresh an
-- OAuth token / App installation token before it lapses (`design/13 §3.9`).
-- `app_id` / `installation_id` are the GitHub App references Task 314 reads.
-- `external_account` is the human-facing login / org (display only).
--
-- `UNIQUE(provider, scope_id)` enforces one metadata row per (provider, scope).

CREATE TABLE vcs_credentials (
    id                TEXT PRIMARY KEY,
    -- 'github' | 'linear' | 'jira' (free-form TEXT; same forward-compat posture
    -- as pull_requests.provider).
    provider          TEXT NOT NULL,
    -- App id (App auth) / repo id (webhook) / provider account id (Linear/Jira).
    scope_id          TEXT NOT NULL,
    -- Human-facing login / org name (display only; not a secret).
    external_account  TEXT,
    -- GitHub App id (App auth only; NULL for PAT/webhook/Linear/Jira rows).
    app_id            TEXT,
    -- GitHub App installation id (App auth only).
    installation_id   TEXT,
    -- When the keychain-held token/installation token expires (epoch ms),
    -- nullable (PATs / personal keys do not expire on a schedule).
    token_expires_at  INTEGER,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL,
    UNIQUE(provider, scope_id)
);

CREATE INDEX idx_vcs_credentials_provider ON vcs_credentials(provider);
