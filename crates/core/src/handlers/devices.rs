//! gRPC `Devices` service handler — pairing RPCs (Task 207).
//!
//! Thin delegator over [`crate::security::pairing::PairingCoordinator`]:
//!
//! - `StartPairing` mints a one-shot token + returns the QR [`PairingChallenge`].
//! - `CompletePairing` runs the device's signed `PairingRequest` through the
//!   coordinator (verify sig → consume token → issue cert → insert `devices`
//!   row) and returns the signed cert as opaque CBOR bytes (Decision D1).
//!
//! Task 209 extends the same `Devices` service with `ListDevices` /
//! `RevokeDevice` / `GetCoreInfo`, delegated to
//! [`crate::security::devices::DeviceManager`]; this handler now implements all
//! five RPCs the generated trait declares.
//!
//! Failure mapping (`design/12 §8`): the coordinator returns
//! `concerto_error::Error::Pairing(code)` with a wire-code string; the handler
//! maps `pairing.bad_signature` / `pairing.bad_*` to `UNAUTHENTICATED` and the
//! token-state failures (`pairing.expired` / `pairing.consumed`) to
//! `FAILED_PRECONDITION`. The management RPCs flow through [`error_to_status`]
//! (`RevokeDevice` on an unknown id → `Error::NotFound` → `NOT_FOUND`).

use std::sync::Arc;

use async_trait::async_trait;
use tonic::{Request, Response, Status};

use concerto_proto::v1::devices_server::Devices as DevicesService;
use concerto_proto::v1::{
    CompletePairingRequest, CompletePairingResponse, CoreInfo as ProtoCoreInfo, DeviceEntry,
    ListDevicesResponse, PairingChallenge as ProtoPairingChallenge, RevokeDeviceRequest,
};

use crate::error_map::error_to_status;
use crate::security::devices::{CoreInfo, DeviceManager, DeviceRecord};
use crate::security::pairing::{CompletePairingInput, PairingChallenge, PairingCoordinator};
use concerto_error::Error;

/// Implements the generated `Devices` service trait: the two pairing RPCs
/// (delegated to [`PairingCoordinator`]) plus the three management RPCs
/// (delegated to [`DeviceManager`]).
#[derive(Clone)]
pub struct DevicesHandler {
    coordinator: Arc<PairingCoordinator>,
    devices: Arc<DeviceManager>,
}

impl DevicesHandler {
    pub fn new(coordinator: Arc<PairingCoordinator>, devices: Arc<DeviceManager>) -> Self {
        Self {
            coordinator,
            devices,
        }
    }
}

#[async_trait]
impl DevicesService for DevicesHandler {
    #[tracing::instrument(skip_all, name = "Devices::StartPairing")]
    async fn start_pairing(
        &self,
        _request: Request<()>,
    ) -> Result<Response<ProtoPairingChallenge>, Status> {
        let challenge = self.coordinator.start_pairing().map_err(error_to_status)?;
        Ok(Response::new(challenge_to_proto(challenge)))
    }

    #[tracing::instrument(skip_all, name = "Devices::CompletePairing")]
    async fn complete_pairing(
        &self,
        request: Request<CompletePairingRequest>,
    ) -> Result<Response<CompletePairingResponse>, Status> {
        let req = request.into_inner();

        let device_pubkey: [u8; 32] = req
            .device_pubkey
            .as_slice()
            .try_into()
            .map_err(|_| Status::invalid_argument("device_pubkey must be 32 bytes"))?;
        let signature: [u8; 64] = req
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| Status::invalid_argument("signature must be 64 bytes"))?;
        if req.pairing_token.is_empty() {
            return Err(Status::invalid_argument("pairing_token is required"));
        }
        if req.device_name.is_empty() {
            return Err(Status::invalid_argument("device_name is required"));
        }

        let input = CompletePairingInput {
            device_pubkey,
            device_name: req.device_name,
            nonce: req.nonce,
            signature,
            pairing_token: req.pairing_token,
        };

        let outcome = self
            .coordinator
            .complete_pairing(input)
            .await
            .map_err(pairing_error_to_status)?;

        Ok(Response::new(CompletePairingResponse {
            signed_device_cert: outcome.signed_device_cert,
            core_pubkey: outcome.core_pubkey.to_vec(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Devices::ListDevices")]
    async fn list_devices(
        &self,
        _request: Request<()>,
    ) -> Result<Response<ListDevicesResponse>, Status> {
        let records = self.devices.list_devices().await.map_err(error_to_status)?;
        Ok(Response::new(ListDevicesResponse {
            devices: records.into_iter().map(record_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Devices::RevokeDevice")]
    async fn revoke_device(
        &self,
        request: Request<RevokeDeviceRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.device_id.is_empty() {
            return Err(Status::invalid_argument("device_id is required"));
        }
        self.devices
            .revoke_device(&req.device_id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Devices::GetCoreInfo")]
    async fn get_core_info(
        &self,
        _request: Request<()>,
    ) -> Result<Response<ProtoCoreInfo>, Status> {
        Ok(Response::new(core_info_to_proto(self.devices.core_info())))
    }
}

/// Map a [`DeviceRecord`] to the proto [`DeviceEntry`]. The `0`-sentinel for the
/// nullable columns is already applied in [`DeviceManager::list_devices`].
fn record_to_proto(r: DeviceRecord) -> DeviceEntry {
    DeviceEntry {
        device_id: r.device_id,
        name: r.name,
        public_key: r.public_key,
        paired_at: r.paired_at,
        last_seen_at: r.last_seen_at,
        revoked_at: r.revoked_at,
    }
}

/// Map [`CoreInfo`] to the proto [`ProtoCoreInfo`].
fn core_info_to_proto(c: CoreInfo) -> ProtoCoreInfo {
    ProtoCoreInfo {
        core_pubkey: c.core_pubkey.to_vec(),
        core_version: c.core_version,
        core_host_os: c.core_host_os,
        core_hostname: c.core_hostname,
    }
}

/// Map the coordinator's `Pairing(code)` errors to the precise gRPC status of
/// `design/12 §8`. Non-pairing errors fall through to the standard mapping.
fn pairing_error_to_status(err: Error) -> Status {
    if let Error::Pairing(code) = &err {
        // Token-state failures are precondition violations (the caller can
        // recover by starting a fresh pairing); auth failures are
        // UNAUTHENTICATED.
        if code.starts_with("pairing.expired") || code.starts_with("pairing.consumed") {
            return Status::failed_precondition(code.clone());
        }
        if code.starts_with("pairing.bad_signature")
            || code.starts_with("pairing.bad_device_pubkey")
            || code.starts_with("pairing.bad_nonce")
        {
            return Status::unauthenticated(code.clone());
        }
    }
    error_to_status(err)
}

/// Map the coordinator's [`PairingChallenge`] to the proto message.
fn challenge_to_proto(c: PairingChallenge) -> ProtoPairingChallenge {
    ProtoPairingChallenge {
        core_pubkey: c.core_pubkey.to_vec(),
        pairing_token: c.pairing_token.to_vec(),
        lan_endpoint: c.lan_endpoint,
        relay_hint: c.relay_hint,
        expires_at: Some(system_time_to_prost(c.expires_at)),
    }
}

/// Total `SystemTime` → prost `Timestamp` conversion (never panics on a bad
/// clock; mirrors `runtime::system_time_to_prost`).
fn system_time_to_prost(t: std::time::SystemTime) -> prost_types::Timestamp {
    match t.duration_since(std::time::SystemTime::UNIX_EPOCH) {
        Ok(d) => prost_types::Timestamp {
            seconds: d.as_secs() as i64,
            nanos: d.subsec_nanos() as i32,
        },
        Err(_) => prost_types::Timestamp {
            seconds: 0,
            nanos: 0,
        },
    }
}
