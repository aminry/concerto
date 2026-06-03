//! Authentication middleware: device-cert path (Iroh/WSS) + peer-UID fast
//! path (UDS) into the **identical** Tonic handlers (`design/10 §3.4`, §6,
//! §6.3, Task 210).
//!
//! # The two auth paths, one handler surface
//!
//! `design/10 §3.4` defines two equally-supported ways a request authenticates
//! into the **same** RPC handlers:
//!
//! - **UDS / named pipe** — the kernel attests to the peer. The connecting
//!   process's UID is read from the `UnixStream`'s peer credentials and compared
//!   against the Core's own (`geteuid`). A match is **implicit admin**: no cert
//!   is presented, and the middleware fabricates a [`local_uds_context`]
//!   "local-uds" pseudo-cert [`DeviceContext`] so handlers never branch on
//!   transport. A mismatch is the `design/12 §6.4` "a local non-Concerto process
//!   tries to connect" threat-row → the connection is refused `UNAUTHENTICATED`.
//! - **Iroh QUIC / WSS bridge** — every request carries a
//!   [`DEVICE_CERT_METADATA_KEY`] metadata header holding the base64 of the
//!   on-wire signed cert (`cert_bytes || signature`). The middleware decodes it,
//!   calls the Task-206 [`DeviceCertIssuer::validate`] (signature + expiry +
//!   revoked-set — the in-memory < 200 µs hot path, no DB hit), and on success
//!   injects the resulting [`DeviceContext`].
//!
//! Which path runs is chosen off the **Task-201 [`ConnTransport`] tag**, never
//! by sniffing socket internals. A request with no tag defaults to the UDS path
//! for back-compat (matching the handler default in `crate::conn_transport`).
//!
//! # The request-extension contract (FROZEN, Task 210)
//!
//! After the middleware runs, every authenticated request carries a
//! [`DeviceContext`] in its extensions. Handlers read it via [`device_context`].
//! It is populated **identically** on both paths so a handler cannot tell which
//! transport delivered the request — only the principal it authenticated.
//!
//! # Error mapping (FROZEN, `design/10 §8`)
//!
//! - invalid / expired / malformed / missing cert → `UNAUTHENTICATED` +
//!   `ConcertoError{code = "auth.invalid_cert"}`.
//! - revoked device → `PERMISSION_DENIED` + `ConcertoError{code =
//!   "auth.revoked"}`.
//!
//! # Windows named-pipe gap (gated TODO, V1.0)
//!
//! On Windows the co-located transport is a named pipe that Task 201 maps to
//! [`TransportKind::Uds`], but named-pipe peer attestation
//! (`GetNamedPipeClientProcessId` + token UID) is **not** implemented in V1.0.
//! The peer-UID glue here is gated `#[cfg(unix)]`; on Windows the co-located
//! path currently has **no** peer check (documented limitation — the Windows
//! Core lands with Task 701-adjacent work). See the Handoff Notes.

use base64::Engine as _;
use concerto_identity::{DeviceCertIssuer, DeviceContext, IdentityError, RevokedSet};
use concerto_persist::Persistence;
use concerto_proto::v1::TransportKind;
use std::sync::Arc;
use tonic::{Request, Status};

use crate::conn_transport::ConnTransport;
use crate::error_map::{auth_invalid_cert_status, auth_revoked_status};
use concerto_error::{Error, Result};

/// The metadata key every remote client presents the signed device cert under
/// (`design/10 §3.4`). **FROZEN (Task 210)** — clients (Tasks 218/511/520) key
/// off this exact ASCII string. The value is the base64 of the on-wire signed
/// cert form `cert_bytes || signature` (the bytes `complete_pairing` returns and
/// the device stores verbatim).
pub const DEVICE_CERT_METADATA_KEY: &str = "concerto-device-cert";

/// The sentinel `device_id` of the implicit "local-uds" pseudo-cert
/// [`DeviceContext`] (`design/10 §3.4`). **FROZEN (Task 210)** — a fixed,
/// non-fingerprint 32-byte marker so any later code inspecting `device_id` can
/// recognise the kernel-attested local path. It is intentionally **not** a
/// `BLAKE2b-256(pubkey)` value (no device pubkey exists for the local pipe);
/// the all-`0xED` byte pattern is a fixed, easily-recognised marker in a hex
/// dump (a BLAKE2b fingerprint of a real keypair is overwhelmingly never a
/// 32-byte run of one value, so collision is not a practical concern).
pub const LOCAL_UDS_DEVICE_ID: [u8; 32] = [0xED_u8; 32];

/// The device name carried by the "local-uds" pseudo-cert. **FROZEN (Task
/// 210).**
pub const LOCAL_UDS_DEVICE_NAME: &str = "local-uds";

/// The single V1.0 capability token. Both auth paths grant exactly this set
/// (`design/10 §3.4`: UDS is implicit admin; the `LocalCoreIssuer` cert carries
/// `["admin"]`).
const ADMIN_CAPABILITY: &str = "admin";

/// Build the implicit "local-uds" pseudo-cert [`DeviceContext`] for a
/// kernel-attested same-UID UDS peer (`design/10 §3.4`). **FROZEN shape (Task
/// 210).**
pub fn local_uds_context() -> DeviceContext {
    DeviceContext {
        device_id: LOCAL_UDS_DEVICE_ID,
        device_name: LOCAL_UDS_DEVICE_NAME.to_string(),
        capabilities: vec![ADMIN_CAPABILITY.to_string()],
    }
}

/// Read the authenticated [`DeviceContext`] a successful auth-middleware run
/// injected into the request extensions.
///
/// **FROZEN accessor signature (Task 210).** Handlers call this to obtain the
/// uniform request-scoped principal; it returns `None` only on a request the
/// middleware never touched (direct in-process handler construction in tests).
/// Per-RPC authz consumption ([`AuthzScope`]) thickens later — V1.0 handlers do
/// not yet have to read it; this locks the seam.
pub fn device_context<T>(req: &Request<T>) -> Option<&DeviceContext> {
    req.extensions().get::<DeviceContext>()
}

/// The capability-scope check seam (`design/10 §6`'s `AuthzScope` box).
///
/// V1.0 is a binary model: a device either has the `"admin"` capability (every
/// `LocalCoreIssuer` cert + the local-uds pseudo-cert do) or it is rejected at
/// the cert layer before reaching here, so this **always allows**. Read/write/
/// admin scoping is V2.0 (`tasks/v1.0/README.md §2`, out of scope) — this exists
/// so the pipeline's `AuthzScope` stage is a real seam a later task thickens
/// without re-plumbing.
pub struct AuthzScope;

impl AuthzScope {
    /// Whether `ctx` is permitted to invoke a given RPC. V1.0: any context that
    /// carries the `"admin"` capability is allowed (always true for both auth
    /// paths). Returns `false` for a context with no `"admin"` token so the
    /// seam is honest about the binary model rather than blindly returning
    /// `true`.
    pub fn allows(ctx: &DeviceContext) -> bool {
        ctx.capabilities.iter().any(|c| c == ADMIN_CAPABILITY)
    }
}

/// The auth interceptor: authenticates each inbound request and injects the
/// uniform [`DeviceContext`], or short-circuits with the mapped [`Status`].
///
/// Holds an optional `Arc<dyn DeviceCertIssuer>` (the Task-206 `LocalCoreIssuer`
/// from boot) for the cert path and the Core's own UID for the UDS path. The
/// issuer is `None` on a Core that never established a keychain-backed identity
/// (a headless CI box): the UDS path still works (kernel attestation needs no
/// issuer), but the cert path then refuses every remote connection
/// `UNAUTHENTICATED` (no identity → cannot validate any cert).
#[derive(Clone)]
pub struct AuthInterceptor {
    issuer: Option<Arc<dyn DeviceCertIssuer>>,
    /// The Core process's effective UID, against which a UDS peer's UID is
    /// compared. `#[cfg(unix)]`-only; the field is absent on Windows where the
    /// peer-UID check is the documented gated gap.
    #[cfg(unix)]
    core_uid: u32,
}

impl AuthInterceptor {
    /// Build the interceptor from the optional cert issuer. The Core's UID is
    /// read once from `geteuid()` (`#[cfg(unix)]`).
    pub fn new(issuer: Option<Arc<dyn DeviceCertIssuer>>) -> Self {
        Self {
            issuer,
            #[cfg(unix)]
            // SAFETY: `geteuid` is always-succeeds and has no preconditions.
            core_uid: unsafe { libc::geteuid() } as u32,
        }
    }

    /// Test/explicit constructor pinning the Core UID (so a unit test can
    /// exercise both the match and mismatch branches deterministically without
    /// a same-UID/other-UID socket pair).
    #[cfg(unix)]
    pub fn with_core_uid(issuer: Option<Arc<dyn DeviceCertIssuer>>, core_uid: u32) -> Self {
        Self { issuer, core_uid }
    }

    /// Authenticate one request, returning it with a [`DeviceContext`] injected
    /// or an auth [`Status`]. The transport is chosen off the [`ConnTransport`]
    /// tag (defaulting to UDS when absent, matching the handler default).
    #[allow(clippy::result_large_err)]
    pub fn authenticate(&self, mut req: Request<()>) -> Result<Request<()>, Status> {
        let kind = req
            .extensions()
            .get::<ConnTransport>()
            .map(|t| t.kind())
            .unwrap_or(TransportKind::Uds);

        match kind {
            // Co-located, kernel-attested. The named-pipe Windows variant maps
            // here too (Task 201) but has no peer check yet — gated TODO below.
            TransportKind::Uds => self.authenticate_uds(req),
            // Cert-bearing remote transports. Identical handling for both.
            TransportKind::Iroh | TransportKind::WssBridge => {
                let ctx = self.validate_cert(&req)?;
                req.extensions_mut().insert(ctx);
                Ok(req)
            }
            // `Unspecified` is never produced by a real listener (every listener
            // tags a concrete kind); treat it as the cert path so an untagged
            // remote cannot slip through as implicit-admin.
            TransportKind::Unspecified => {
                let ctx = self.validate_cert(&req)?;
                req.extensions_mut().insert(ctx);
                Ok(req)
            }
        }
    }

    /// UDS path: compare the kernel-attested peer UID against the Core's UID.
    #[cfg(unix)]
    #[allow(clippy::result_large_err)]
    fn authenticate_uds(&self, mut req: Request<()>) -> Result<Request<()>, Status> {
        use tonic::transport::server::UdsConnectInfo;

        // The live UDS server inserts `UdsConnectInfo` (with the peer's
        // credentials) into every request's extensions before this interceptor
        // runs. Its absence means the request was not delivered over a real
        // Unix socket (e.g. a direct in-process test request tagged `Uds`);
        // refuse it rather than granting implicit admin to an unattested peer.
        let peer_uid = req
            .extensions()
            .get::<UdsConnectInfo>()
            .and_then(|info| info.peer_cred)
            .map(|cred| cred.uid());

        match peer_uid {
            Some(uid) if uid == self.core_uid => {
                req.extensions_mut().insert(local_uds_context());
                Ok(req)
            }
            Some(_) => Err(auth_invalid_cert_status(
                "uds peer uid does not match the Core's owning uid",
            )),
            None => Err(auth_invalid_cert_status(
                "uds connection carries no peer credentials",
            )),
        }
    }

    /// UDS path on non-Unix targets: the documented V1.0 gap. The Windows
    /// named-pipe co-located transport maps to `TransportKind::Uds` (Task 201)
    /// but peer attestation (`GetNamedPipeClientProcessId`) is not implemented;
    /// until the Windows Core (Task 701-adjacent) lands, the co-located path has
    /// no peer check and grants the local-uds context unconditionally. Stated
    /// loudly in the Handoff Notes. The whole UDS gRPC server is itself
    /// unsupported on non-Unix in V1.0 (`api_server` errors out), so this branch
    /// is unreachable in practice — it exists only to keep the module compiling
    /// on the Windows CI lane.
    #[cfg(not(unix))]
    #[allow(clippy::result_large_err)]
    fn authenticate_uds(&self, mut req: Request<()>) -> Result<Request<()>, Status> {
        req.extensions_mut().insert(local_uds_context());
        Ok(req)
    }

    /// Cert path: read + base64-decode the `concerto-device-cert` header and run
    /// the Task-206 validator, mapping its `Err` variants to the FROZEN auth
    /// statuses (`design/10 §8`).
    #[allow(clippy::result_large_err)]
    fn validate_cert(&self, req: &Request<()>) -> Result<DeviceContext, Status> {
        let issuer = self.issuer.as_ref().ok_or_else(|| {
            // No keychain-backed identity → no Core key to validate any cert
            // against. A remote presenting a cert cannot be authenticated.
            auth_invalid_cert_status("this Core has no identity; cannot validate device certs")
        })?;

        // Missing header = an invalid cert (`design/10 §8`: "no cert is an
        // invalid cert"). `MetadataValue::to_str` rejects non-ASCII; a binary
        // value under this ASCII key is itself malformed.
        let raw_header = req
            .metadata()
            .get(DEVICE_CERT_METADATA_KEY)
            .ok_or_else(|| auth_invalid_cert_status("missing concerto-device-cert metadata"))?
            .to_str()
            .map_err(|_| auth_invalid_cert_status("concerto-device-cert metadata is not ASCII"))?;

        let raw = base64::engine::general_purpose::STANDARD
            .decode(raw_header)
            .map_err(|_| auth_invalid_cert_status("concerto-device-cert is not valid base64"))?;

        issuer.validate(&raw).map_err(map_validate_err)
    }
}

/// Encode the on-wire signed cert (`cert_bytes || signature`) into the value a
/// client puts under [`DEVICE_CERT_METADATA_KEY`]. The middleware base64-decodes
/// the same way. Exposed so clients (and the Tier-1 tests) produce the exact
/// wire form rather than re-deriving the encoding. **FROZEN encoding (Task
/// 210).**
pub fn encode_cert_metadata(signed_device_cert: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(signed_device_cert)
}

/// Map a Task-206 [`IdentityError`] from `validate` to the FROZEN auth status
/// (`design/10 §8`). Revoked → `PERMISSION_DENIED`/`auth.revoked`; everything
/// else (signature/structure invalid, expired, wrong-Core, malformed) →
/// `UNAUTHENTICATED`/`auth.invalid_cert`.
fn map_validate_err(err: IdentityError) -> Status {
    match err {
        IdentityError::Revoked => auth_revoked_status("device certificate has been revoked"),
        IdentityError::Expired => auth_invalid_cert_status("device certificate has expired"),
        IdentityError::WrongCore => {
            auth_invalid_cert_status("device certificate was issued by a different Core")
        }
        IdentityError::BadSignature
        | IdentityError::BadPublicKey
        | IdentityError::Truncated
        | IdentityError::BadCbor(_)
        | IdentityError::Rng(_)
        | IdentityError::Noise(_) => {
            auth_invalid_cert_status("device certificate is invalid or malformed")
        }
    }
}

/// Re-populate the in-memory `revoked_set` from the `devices` table at startup
/// — closes the Task-209 gap (its Handoff: boot does not re-mirror revoked rows,
/// so the set starts empty each boot and a previously-revoked cert is accepted
/// after a restart until re-revoked).
///
/// Runs `SELECT id FROM devices WHERE revoked_at IS NOT NULL` and inserts each
/// decoded 32-byte `device_id` into the shared set the auth path + the Task-206
/// validator read. **Must run BEFORE the gRPC server (auth path) goes live** so
/// a revoked device stays revoked across a Core restart. Returns the number of
/// ids restored. Only ever *adds* to the set (never removes), so a concurrent
/// live `RevokeDevice` racing this mirror can only converge to "revoked" — there
/// is no fail-open. Rows with an unparseable id (impossible for a real
/// fingerprint) are skipped with a warning.
pub async fn mirror_revoked_devices(
    persistence: &Persistence,
    revoked_set: &RevokedSet,
) -> Result<usize> {
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM devices WHERE revoked_at IS NOT NULL")
            .fetch_all(persistence.readers())
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;

    let mut restored = 0usize;
    let mut set = match revoked_set.write() {
        Ok(guard) => guard,
        // A poisoned lock means a panicked writer; recover and still insert —
        // the revoked set must never silently drop a revocation.
        Err(poisoned) => poisoned.into_inner(),
    };
    for hex_id in ids {
        match decode_hex_device_id(&hex_id) {
            Some(raw) => {
                set.insert(raw);
                restored += 1;
            }
            None => tracing::warn!(
                device_id = %hex_id,
                "skipping unparseable revoked devices.id during startup mirror"
            ),
        }
    }
    Ok(restored)
}

/// Decode a hex `devices.id` into the raw 32-byte fingerprint the revoked set
/// keys on. Returns `None` for non-hex or wrong-length input.
fn decode_hex_device_id(hex_id: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(hex_id).ok()?;
    bytes.as_slice().try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use concerto_identity::{PairingRequest, SignedDeviceCert};

    /// The canned outcome a [`StubIssuer`] returns. We model the error as a
    /// distinct variant (rather than holding a `Result<_, IdentityError>`)
    /// because `IdentityError` is intentionally not `Clone` (it can wrap a
    /// `snow` diagnostic `String`); the stub re-materializes a fresh error each
    /// `validate` call instead.
    enum Outcome {
        Ok(DeviceContext),
        Err(fn() -> IdentityError),
    }

    /// A canned-result `DeviceCertIssuer` stub: `validate` returns the wired
    /// outcome independent of the bytes, so the auth layer is exercised without
    /// real keys (`design/10` Tier-1 determinism note).
    struct StubIssuer {
        outcome: Outcome,
    }

    #[async_trait]
    impl DeviceCertIssuer for StubIssuer {
        async fn issue(&self, _req: PairingRequest) -> concerto_identity::Result<SignedDeviceCert> {
            unreachable!("issue is not exercised by the auth-layer tests")
        }
        fn validate(&self, _raw: &[u8]) -> concerto_identity::Result<DeviceContext> {
            match &self.outcome {
                Outcome::Ok(ctx) => Ok(ctx.clone()),
                Outcome::Err(make) => Err(make()),
            }
        }
        fn supported_capabilities(&self) -> &'static [&'static str] {
            &["admin"]
        }
    }

    fn ok_stub(ctx: DeviceContext) -> Arc<dyn DeviceCertIssuer> {
        Arc::new(StubIssuer {
            outcome: Outcome::Ok(ctx),
        })
    }

    fn err_stub(make: fn() -> IdentityError) -> Arc<dyn DeviceCertIssuer> {
        Arc::new(StubIssuer {
            outcome: Outcome::Err(make),
        })
    }

    fn admin_ctx() -> DeviceContext {
        DeviceContext {
            device_id: [9u8; 32],
            device_name: "Test Phone".to_string(),
            capabilities: vec!["admin".to_string()],
        }
    }

    /// Build an Iroh-tagged request carrying a (base64) cert header.
    fn iroh_request_with_cert(cert_bytes: &[u8]) -> Request<()> {
        let mut req = Request::new(());
        req.extensions_mut()
            .insert(ConnTransport(TransportKind::Iroh));
        req.metadata_mut().insert(
            DEVICE_CERT_METADATA_KEY,
            encode_cert_metadata(cert_bytes).parse().unwrap(),
        );
        req
    }

    #[test]
    fn local_uds_sentinel_is_frozen_and_not_a_real_fingerprint() {
        // The sentinel is a fixed pattern, distinct from any BLAKE2b id.
        assert_eq!(LOCAL_UDS_DEVICE_ID, [0xED_u8; 32]);
        let ctx = local_uds_context();
        assert_eq!(ctx.device_id, LOCAL_UDS_DEVICE_ID);
        assert_eq!(ctx.device_name, "local-uds");
        assert_eq!(ctx.capabilities, vec!["admin".to_string()]);
    }

    #[test]
    fn cert_path_valid_injects_device_context() {
        let interceptor = AuthInterceptor::new(Some(ok_stub(admin_ctx())));
        let req = iroh_request_with_cert(b"any-bytes");
        let out = interceptor
            .authenticate(req)
            .expect("valid cert authenticates");
        let ctx = device_context(&out).expect("DeviceContext injected");
        assert_eq!(ctx.device_id, admin_ctx().device_id);
        assert_eq!(ctx.capabilities, vec!["admin".to_string()]);
        assert!(AuthzScope::allows(ctx));
    }

    #[test]
    fn cert_path_expired_is_unauthenticated_invalid_cert() {
        let interceptor = AuthInterceptor::new(Some(err_stub(|| IdentityError::Expired)));
        let err = interceptor
            .authenticate(iroh_request_with_cert(b"x"))
            .expect_err("expired must reject");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert_eq!(
            crate::error_map::concerto_code(&err).as_deref(),
            Some("auth.invalid_cert")
        );
    }

    #[test]
    fn cert_path_revoked_is_permission_denied_revoked() {
        let interceptor = AuthInterceptor::new(Some(err_stub(|| IdentityError::Revoked)));
        let err = interceptor
            .authenticate(iroh_request_with_cert(b"x"))
            .expect_err("revoked must reject");
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
        assert_eq!(
            crate::error_map::concerto_code(&err).as_deref(),
            Some("auth.revoked")
        );
    }

    #[test]
    fn cert_path_wrong_core_is_invalid_cert() {
        let interceptor = AuthInterceptor::new(Some(err_stub(|| IdentityError::WrongCore)));
        let err = interceptor
            .authenticate(iroh_request_with_cert(b"x"))
            .expect_err("wrong-core must reject");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert_eq!(
            crate::error_map::concerto_code(&err).as_deref(),
            Some("auth.invalid_cert")
        );
    }

    #[test]
    fn cert_path_missing_header_is_invalid_cert() {
        let interceptor = AuthInterceptor::new(Some(ok_stub(admin_ctx())));
        let mut req = Request::new(());
        req.extensions_mut()
            .insert(ConnTransport(TransportKind::Iroh));
        // No metadata header.
        let err = interceptor
            .authenticate(req)
            .expect_err("missing cert rejects");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert_eq!(
            crate::error_map::concerto_code(&err).as_deref(),
            Some("auth.invalid_cert")
        );
    }

    #[test]
    fn cert_path_garbage_base64_is_invalid_cert() {
        let interceptor = AuthInterceptor::new(Some(ok_stub(admin_ctx())));
        let mut req = Request::new(());
        req.extensions_mut()
            .insert(ConnTransport(TransportKind::Iroh));
        req.metadata_mut().insert(
            DEVICE_CERT_METADATA_KEY,
            "!!!not base64!!!".parse().unwrap(),
        );
        let err = interceptor.authenticate(req).expect_err("garbage rejects");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert_eq!(
            crate::error_map::concerto_code(&err).as_deref(),
            Some("auth.invalid_cert")
        );
    }

    #[test]
    fn cert_path_with_no_issuer_rejects() {
        // A Core with no identity cannot validate any cert.
        let interceptor = AuthInterceptor::new(None);
        let err = interceptor
            .authenticate(iroh_request_with_cert(b"x"))
            .expect_err("no issuer rejects");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert_eq!(
            crate::error_map::concerto_code(&err).as_deref(),
            Some("auth.invalid_cert")
        );
    }

    // The UDS peer-uid comparison's positive/negative branches are driven over a
    // real Unix socket pair (so `UdsConnectInfo` is genuinely present) in
    // `crates/core/tests/auth_middleware.rs`. Here we cover the no-credentials
    // refusal: a request tagged `Uds` but lacking `UdsConnectInfo` must NOT be
    // granted implicit admin.
    #[cfg(unix)]
    #[test]
    fn uds_path_without_peer_cred_is_refused() {
        let interceptor = AuthInterceptor::with_core_uid(None, 1000);
        let mut req = Request::new(());
        req.extensions_mut()
            .insert(ConnTransport(TransportKind::Uds));
        let err = interceptor
            .authenticate(req)
            .expect_err("untagged-cred uds peer must be refused");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    #[test]
    fn authz_scope_rejects_a_context_without_admin() {
        let ctx = DeviceContext {
            device_id: [1u8; 32],
            device_name: "x".into(),
            capabilities: vec![],
        };
        assert!(!AuthzScope::allows(&ctx));
    }
}
