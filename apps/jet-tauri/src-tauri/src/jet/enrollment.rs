//! Enrollment: this computer pairs itself with a remote Plane over SSH
//! (ADR-0017), or pairs again in place with one it already knows.
//!
//! The flow is draft → claim → ticket → complete. A draft names the SSH
//! address; a claim presents the one-time code and this computer's public
//! key and gets back the pairing transcript; the ticket keeps the validated
//! transcript until the person at the target confirms; completion signs it.
//! Every piece of native authority here is bounded (4 each) and expires.
//!
//! Secrets: the typed code is kept only as a zeroizing 8-byte array for an
//! exact-body retry of an uncertain claim (never as a map key, so rehashing
//! never copies it), for at most 2 minutes. The pairing key is unlocked, and
//! the transcript signed, before ssh starts. The webview gets the locally
//! computed authentication string, never the server's.
use std::{
    fmt,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use jet_client::SshEndpoint;
use jet_protocol::{
    ClientPublicKey, PairingKeyAlgorithm, PairingProgress, RemotePairingRequest,
    RemotePairingResponse,
};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use super::{
    errors::PublicError,
    keystore::{key_missing, same_bytes, Credential},
    pairing_transcript::{
        authentication_string, transcript_invalid, validate, ValidatedTranscript,
    },
    planes::{
        local_not_removable,
        registry_file::validate_destination,
        remote::{enroll, login},
        NewRemote, PlaneBinding, PlaneId, PlaneView, PlanesView,
    },
    JetBridge,
};

const MAX_DRAFTS: usize = 4;
const DRAFT_LIFETIME: Duration = Duration::from_secs(10 * 60);
const MAX_CLAIMS: usize = 4;
/// An uncertain claim may be retried with the exact same body this long.
const CLAIM_RETENTION: Duration = Duration::from_secs(2 * 60);
const MAX_TICKETS: usize = 4;
/// Native cap only. The Plane's own windows decide when a pairing ends.
const TICKET_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// Characters accepted around the eight code digits.
const MAXIMUM_CODE_INPUT: usize = 32;

#[derive(Clone)]
struct Draft {
    id: Uuid,
    destination: String,
    endpoint: SshEndpoint,
    /// Pair again: the entry being re-paired and its stored identity.
    repair_of: Option<PlaneBinding>,
    credential: Credential,
    created: Instant,
}

/// The code of an uncertain claim, kept for an exact-body retry.
struct RetainedClaim {
    draft_id: Uuid,
    digits: Zeroizing<[u8; 8]>,
    command_id: Uuid,
    until: Instant,
}

impl Drop for RetainedClaim {
    fn drop(&mut self) {
        self.digits.zeroize();
        #[cfg(test)]
        tests::DROPPED_CLAIMS.with(|dropped| dropped.borrow_mut().push(*self.digits));
    }
}

#[derive(Clone)]
struct Ticket {
    id: Uuid,
    draft_id: Uuid,
    destination: String,
    endpoint: SshEndpoint,
    /// The entry this pairing creates, or the one it repairs.
    plane: Uuid,
    repair_of: Option<PlaneBinding>,
    offer_id: Uuid,
    transcript: ValidatedTranscript,
    complete_command: Uuid,
    credential: Credential,
    hard_expiry: Instant,
}

/// Native enrollment authority. It holds no key material; a retained code
/// is zeroized when dropped, and `Debug` shows counts only.
#[derive(Default)]
pub(crate) struct EnrollmentState {
    drafts: Mutex<Vec<Draft>>,
    claims: Mutex<Vec<RetainedClaim>>,
    tickets: Mutex<Vec<Ticket>>,
}

impl fmt::Debug for EnrollmentState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = |length: Option<usize>| length.unwrap_or(0);
        formatter
            .debug_struct("EnrollmentState")
            .field("drafts", &count(self.drafts.lock().ok().map(|v| v.len())))
            .field("claims", &count(self.claims.lock().ok().map(|v| v.len())))
            .field("tickets", &count(self.tickets.lock().ok().map(|v| v.len())))
            .finish()
    }
}

fn draft_expired() -> PublicError {
    PublicError::invalid_input(
        "enrollment.draft_expired",
        "This pairing request expired. Start adding the Plane again.",
    )
}

fn ticket_expired() -> PublicError {
    PublicError::invalid_input(
        "enrollment.ticket_expired",
        "This pairing request expired. Get a new code on the other computer.",
    )
}

fn code_invalid() -> PublicError {
    PublicError::invalid_input(
        "enrollment.code_invalid",
        "Type the 8-digit code shown on the other computer.",
    )
}

impl EnrollmentState {
    fn add_draft(&self, draft: Draft, now: Instant) -> Result<(), PublicError> {
        let mut drafts = self.drafts.lock().map_err(|_| PublicError::internal())?;
        drafts.retain(|draft| now.saturating_duration_since(draft.created) <= DRAFT_LIFETIME);
        if drafts.len() >= MAX_DRAFTS {
            let evicted = drafts.remove(0);
            self.drop_claims(evicted.id);
        }
        drafts.push(draft);
        Ok(())
    }

    fn draft(&self, id: Uuid, now: Instant) -> Result<Draft, PublicError> {
        let mut drafts = self.drafts.lock().map_err(|_| PublicError::internal())?;
        drafts.retain(|draft| now.saturating_duration_since(draft.created) <= DRAFT_LIFETIME);
        drafts
            .iter()
            .find(|draft| draft.id == id)
            .cloned()
            .ok_or_else(draft_expired)
    }

    /// The command ID for this exact claim body: reused for the same draft
    /// and the same digits (compared in constant time) within 2 minutes.
    fn claim_command(
        &self,
        draft_id: Uuid,
        digits: &Zeroizing<[u8; 8]>,
        now: Instant,
    ) -> Result<Uuid, PublicError> {
        let mut claims = self.claims.lock().map_err(|_| PublicError::internal())?;
        claims.retain(|claim| claim.until > now);
        if let Some(claim) = claims
            .iter()
            .find(|claim| claim.draft_id == draft_id && same_bytes(&claim.digits[..], &digits[..]))
        {
            return Ok(claim.command_id);
        }
        // A different code for the same draft is a new body.
        claims.retain(|claim| claim.draft_id != draft_id);
        if claims.len() >= MAX_CLAIMS {
            claims.remove(0);
        }
        let command_id = Uuid::new_v4();
        claims.push(RetainedClaim {
            draft_id,
            digits: Zeroizing::new(**digits),
            command_id,
            until: now + CLAIM_RETENTION,
        });
        Ok(command_id)
    }

    fn drop_claims(&self, draft_id: Uuid) {
        if let Ok(mut claims) = self.claims.lock() {
            claims.retain(|claim| claim.draft_id != draft_id);
        }
    }

    fn add_ticket(&self, ticket: Ticket, now: Instant) -> Result<(), PublicError> {
        let mut tickets = self.tickets.lock().map_err(|_| PublicError::internal())?;
        tickets.retain(|ticket| ticket.hard_expiry > now);
        // One ticket per draft: a newer claim replaces the older ticket.
        tickets.retain(|existing| existing.draft_id != ticket.draft_id);
        if tickets.len() >= MAX_TICKETS {
            tickets.remove(0);
        }
        tickets.push(ticket);
        Ok(())
    }

    fn ticket(&self, id: Uuid, now: Instant) -> Result<Ticket, PublicError> {
        let mut tickets = self.tickets.lock().map_err(|_| PublicError::internal())?;
        tickets.retain(|ticket| ticket.hard_expiry > now);
        tickets
            .iter()
            .find(|ticket| ticket.id == id)
            .cloned()
            .ok_or_else(ticket_expired)
    }

    /// `pairing.not_confirmed` and other definite refusals are durable
    /// answers: the next attempt must be a new command.
    fn renew_completion(&self, id: Uuid) {
        if let Ok(mut tickets) = self.tickets.lock() {
            if let Some(ticket) = tickets.iter_mut().find(|ticket| ticket.id == id) {
                ticket.complete_command = Uuid::new_v4();
            }
        }
    }

    fn drop_ticket(&self, id: Uuid) {
        if let Ok(mut tickets) = self.tickets.lock() {
            tickets.retain(|ticket| ticket.id != id);
        }
    }

    /// Ends one enrollment, whether `id` names its draft or its ticket.
    fn cancel(&self, id: Uuid) {
        let draft_of_ticket = self.tickets.lock().ok().and_then(|mut tickets| {
            let draft = tickets
                .iter()
                .find(|ticket| ticket.id == id)
                .map(|ticket| ticket.draft_id);
            tickets.retain(|ticket| ticket.id != id && ticket.draft_id != id);
            draft
        });
        let draft = draft_of_ticket.unwrap_or(id);
        if let Ok(mut drafts) = self.drafts.lock() {
            drafts.retain(|existing| existing.id != draft);
        }
        self.drop_claims(draft);
    }

    /// A forgotten Plane cannot be paired again through an older request.
    pub(crate) fn forget_plane(&self, plane: PlaneId) {
        let repairs = |binding: &Option<PlaneBinding>| binding.is_some_and(|b| b.plane == plane);
        let dropped: Vec<Uuid> = self
            .drafts
            .lock()
            .map(|mut drafts| {
                let dropped = drafts
                    .iter()
                    .filter(|draft| repairs(&draft.repair_of))
                    .map(|draft| draft.id)
                    .collect();
                drafts.retain(|draft| !repairs(&draft.repair_of));
                dropped
            })
            .unwrap_or_default();
        for draft in dropped {
            self.drop_claims(draft);
        }
        if let Ok(mut tickets) = self.tickets.lock() {
            tickets.retain(|ticket| !repairs(&ticket.repair_of));
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum AddPlaneResult {
    Connected {
        plane: Box<PlaneView>,
    },
    PairingRequired {
        draft_id: String,
        destination: String,
    },
}

/// What the confirm step shows. The authentication string is the one this
/// computer computed from the validated transcript.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EnrollmentView {
    ticket_id: String,
    destination: String,
    plane_identity: String,
    authentication_string: String,
    confirm_by_unix_ms: String,
    session_only: bool,
}

fn credential(session_only: Option<bool>) -> Credential {
    if session_only.unwrap_or(false) {
        Credential::Session
    } else {
        Credential::Durable
    }
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// A refused login that pairing (again) can fix.
fn needs_pairing(error: &PublicError) -> bool {
    matches!(
        error.code.as_str(),
        "connection.unauthorized" | "identity.key_missing" | "identity.session_ended"
    )
}

/// Keeps only the eight digits; spaces and hyphens may separate them.
fn parse_code(code: &str) -> Result<Zeroizing<[u8; 8]>, PublicError> {
    if code.len() > MAXIMUM_CODE_INPUT {
        return Err(code_invalid());
    }
    let mut digits = Zeroizing::new([0u8; 8]);
    let mut count = 0;
    for byte in code.bytes() {
        match byte {
            b'0'..=b'9' if count < digits.len() => {
                digits[count] = byte;
                count += 1;
            }
            b' ' | b'-' => {}
            _ => return Err(code_invalid()),
        }
    }
    if count != digits.len() {
        return Err(code_invalid());
    }
    Ok(digits)
}

fn parse_request_id(value: &str, missing: fn() -> PublicError) -> Result<Uuid, PublicError> {
    Uuid::parse_str(value).map_err(|_| missing())
}

// ---------------------------------------------------------------------------
// Flows (plain functions over the bridge, so tests drive them directly)
// ---------------------------------------------------------------------------

/// Logs in if a key exists; otherwise (or when the Plane refuses this
/// computer) prepares a pairing draft. A durable key is only ever created
/// after the ADR-0076 probe succeeded.
async fn start(
    bridge: &JetBridge,
    destination: String,
    endpoint: SshEndpoint,
    repair_of: Option<PlaneBinding>,
    credential: Credential,
    now: Instant,
) -> Result<AddPlaneResult, PublicError> {
    let keys = bridge.planes.keys();
    let client_id = bridge.planes.local().client_id();
    let has_seed = keys.has_seed(client_id, credential).await;
    if has_seed && repair_of.is_none() {
        match login(
            bridge.planes.spawner().as_ref(),
            keys,
            &endpoint,
            client_id,
            credential,
            None,
        )
        .await
        {
            Ok((session, status)) => {
                let plane = bridge.planes.add_remote(
                    NewRemote {
                        id: Uuid::new_v4(),
                        destination,
                        endpoint,
                        plane_identity: status.plane_id,
                        credential,
                        session: Some((session, status)),
                    },
                    unix_ms(),
                )?;
                return Ok(AddPlaneResult::Connected {
                    plane: Box::new(bridge.planes.view(plane)?),
                });
            }
            Err(error) if needs_pairing(&error) => {}
            Err(error) => return Err(error),
        }
    }
    if credential == Credential::Durable && !has_seed {
        keys.probe().await?;
    }
    let id = Uuid::new_v4();
    bridge.enrollment.add_draft(
        Draft {
            id,
            destination: destination.clone(),
            endpoint,
            repair_of,
            credential,
            created: now,
        },
        now,
    )?;
    Ok(AddPlaneResult::PairingRequired {
        draft_id: id.to_string(),
        destination,
    })
}

pub(crate) async fn add(
    bridge: &JetBridge,
    destination: &str,
    session_only: Option<bool>,
    now: Instant,
) -> Result<AddPlaneResult, PublicError> {
    let (destination, endpoint) = validate_destination(destination)?;
    bridge.planes.check_destination(&destination, None)?;
    bridge.planes.check_capacity()?;
    start(
        bridge,
        destination,
        endpoint,
        None,
        credential(session_only),
        now,
    )
    .await
}

pub(crate) async fn repair(
    bridge: &JetBridge,
    plane_id: &str,
    session_only: Option<bool>,
    now: Instant,
) -> Result<AddPlaneResult, PublicError> {
    let target = bridge.planes.remote_target(plane_id)?;
    let credential = credential(session_only);
    let plane = target.binding.plane;
    if credential == target.credential
        && bridge
            .planes
            .keys()
            .has_seed(bridge.planes.local().client_id(), credential)
            .await
    {
        // The owner may have enabled this computer again: a login is enough.
        target.client.reset().await;
        match target.client.connect().await {
            Ok(_) => {
                return Ok(AddPlaneResult::Connected {
                    plane: Box::new(bridge.planes.view(plane)?),
                })
            }
            Err(error) => {
                let error = PublicError::from_client(&error);
                if !needs_pairing(&error) {
                    return Err(bridge.planes.settle(plane, error));
                }
            }
        }
    }
    start(
        bridge,
        target.destination,
        target.endpoint,
        Some(target.binding),
        credential,
        now,
    )
    .await
    .map_err(|error| bridge.planes.settle(plane, error))
}

pub(crate) async fn claim(
    bridge: &JetBridge,
    draft_id: &str,
    code: &str,
    now: Instant,
) -> Result<EnrollmentView, PublicError> {
    let draft_id = parse_request_id(draft_id, draft_expired)?;
    let digits = parse_code(code)?;
    let draft = bridge.enrollment.draft(draft_id, now)?;
    let client_id = bridge.planes.local().client_id();
    // The key exists, and its public half is known, before ssh starts.
    let own_key = bridge
        .planes
        .keys()
        .ensure_public(client_id, draft.credential)
        .await?;
    let command_id = bridge.enrollment.claim_command(draft_id, &digits, now)?;
    let mut request = RemotePairingRequest::Claim {
        command_id,
        secret: String::from_utf8(digits.to_vec()).map_err(|_| code_invalid())?,
        key: ClientPublicKey {
            algorithm: PairingKeyAlgorithm::Ed25519,
            key: own_key,
        },
    };
    let outcome = enroll(
        bridge.planes.spawner().as_ref(),
        &draft.endpoint,
        client_id,
        &request,
    )
    .await;
    if let RemotePairingRequest::Claim { secret, .. } = &mut request {
        secret.zeroize();
    }
    let response = match outcome {
        Ok(response) => response,
        Err(failure) => {
            if failure.definite {
                // The Plane answered; retrying the same body would only
                // repeat that answer. The draft stays for a new code.
                bridge.enrollment.drop_claims(draft_id);
            }
            return Err(failure.error);
        }
    };
    bridge.enrollment.drop_claims(draft_id);
    let RemotePairingResponse::Claimed {
        pending,
        signing_bytes,
    } = response
    else {
        return Err(transcript_invalid());
    };
    let transcript = validate(
        &signing_bytes,
        pending.offer_id,
        client_id,
        &own_key,
        draft.repair_of.and_then(|binding| binding.identity),
    )?;
    let shown = authentication_string(&transcript);
    // Showing the server's string would defeat the key-substitution check.
    match &pending.progress {
        PairingProgress::AwaitingConfirmation {
            client_id: claimant,
            authentication_string,
        } if *claimant == client_id && *authentication_string == shown => {}
        _ => return Err(transcript_invalid()),
    }
    let repaired = match draft.repair_of {
        Some(PlaneBinding {
            plane: PlaneId::Remote(id),
            ..
        }) => Some(id),
        _ => None,
    };
    bridge
        .planes
        .check_identity(transcript.plane_identity(), repaired)?;
    let ticket_id = Uuid::new_v4();
    let plane_identity = transcript.plane_identity();
    bridge.enrollment.add_ticket(
        Ticket {
            id: ticket_id,
            draft_id,
            destination: draft.destination.clone(),
            endpoint: draft.endpoint.clone(),
            plane: repaired.unwrap_or_else(Uuid::new_v4),
            repair_of: draft.repair_of,
            offer_id: pending.offer_id,
            transcript,
            complete_command: Uuid::new_v4(),
            credential: draft.credential,
            hard_expiry: now + TICKET_LIFETIME,
        },
        now,
    )?;
    Ok(EnrollmentView {
        ticket_id: ticket_id.to_string(),
        destination: draft.destination,
        plane_identity: plane_identity.simple().to_string()[..8].to_owned(),
        authentication_string: shown,
        confirm_by_unix_ms: pending.expires_at_unix_ms.to_string(),
        session_only: draft.credential == Credential::Session,
    })
}

pub(crate) async fn complete(
    bridge: &JetBridge,
    ticket_id: &str,
    now: Instant,
) -> Result<PlaneView, PublicError> {
    let ticket_id = parse_request_id(ticket_id, ticket_expired)?;
    // Only the native cap applies here; the Plane's windows decide.
    let ticket = bridge.enrollment.ticket(ticket_id, now)?;
    if let Some(binding) = &ticket.repair_of {
        // The entry must still be the Plane this pairing was claimed for.
        bridge.planes.bound(binding)?;
    }
    let client_id = bridge.planes.local().client_id();
    let keys = bridge.planes.keys();
    // Unlock and sign before ssh starts.
    let identity = keys
        .unlock_for_handshake(client_id, ticket.credential)
        .await?;
    let own_key = identity.public_key();
    if !ticket.transcript.names_key(&own_key) {
        return Err(key_missing());
    }
    let signature = identity.sign_pairing(&ticket.transcript);
    drop(identity);
    let request = RemotePairingRequest::Complete {
        command_id: ticket.complete_command,
        offer_id: ticket.offer_id,
        signature,
    };
    let outcome = enroll(
        bridge.planes.spawner().as_ref(),
        &ticket.endpoint,
        client_id,
        &request,
    )
    .await;
    match outcome {
        Ok(RemotePairingResponse::Completed { client }) => {
            if client.client_id != client_id || client.key.key != own_key {
                bridge.enrollment.drop_ticket(ticket.id);
                return Err(transcript_invalid());
            }
            finish(bridge, &ticket).await
        }
        Ok(_) => {
            bridge.enrollment.drop_ticket(ticket.id);
            Err(transcript_invalid())
        }
        Err(failure) => {
            if failure.definite {
                match failure.error.code.as_str() {
                    "pairing.offer_expired"
                    | "pairing.offer_ended"
                    | "pairing.offer_superseded"
                    | "pairing.signature_rejected"
                    | "pairing.completion_by_other" => bridge.enrollment.drop_ticket(ticket.id),
                    // Not confirmed yet (or another durable refusal): keep
                    // the ticket, and make the next attempt a new command.
                    _ => bridge.enrollment.renew_completion(ticket.id),
                }
            }
            Err(failure.error)
        }
    }
}

/// Records a completed pairing. A new Plane is persisted only after a login
/// proved its identity; a repaired one keeps its `PlaneId` and selection.
async fn finish(bridge: &JetBridge, ticket: &Ticket) -> Result<PlaneView, PublicError> {
    let plane = PlaneId::Remote(ticket.plane);
    match &ticket.repair_of {
        Some(binding) => {
            let client = bridge.planes.repaired(binding, ticket.credential).await?;
            bridge.enrollment.cancel(ticket.id);
            // The pairing is done; a failed login shows in the Plane's state.
            let _ = client.connect().await;
            bridge.planes.view(plane)
        }
        None => {
            let (session, status) = login(
                bridge.planes.spawner().as_ref(),
                bridge.planes.keys(),
                &ticket.endpoint,
                bridge.planes.local().client_id(),
                ticket.credential,
                Some(ticket.transcript.plane_identity()),
            )
            .await?;
            bridge.planes.add_remote(
                NewRemote {
                    id: ticket.plane,
                    destination: ticket.destination.clone(),
                    endpoint: ticket.endpoint.clone(),
                    plane_identity: status.plane_id,
                    credential: ticket.credential,
                    session: Some((session, status)),
                },
                unix_ms(),
            )?;
            bridge.enrollment.cancel(ticket.id);
            bridge.planes.view(plane)
        }
    }
}

/// Removes a remote Plane from this computer only. Nothing is sent to it.
pub(crate) async fn forget(bridge: &JetBridge, plane_id: &str) -> Result<PlanesView, PublicError> {
    let plane = PlaneId::parse(plane_id)?;
    if plane == PlaneId::Local {
        return Err(local_not_removable());
    }
    bridge.planes.forget(plane).await?;
    bridge.feeds.forget(plane);
    bridge.notifications.forget(plane);
    bridge.pairing.forget_plane(plane);
    bridge.enrollment.forget_plane(plane);
    bridge.planes.snapshot(bridge.restorable_selection()?)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn add_remote_plane(
    bridge: State<'_, JetBridge>,
    destination: String,
    session_only: Option<bool>,
) -> Result<AddPlaneResult, PublicError> {
    add(&bridge, &destination, session_only, Instant::now()).await
}

#[tauri::command]
pub(crate) async fn repair_remote_plane(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    session_only: Option<bool>,
) -> Result<AddPlaneResult, PublicError> {
    repair(&bridge, &plane_id, session_only, Instant::now()).await
}

#[tauri::command]
pub(crate) async fn claim_remote_pairing(
    bridge: State<'_, JetBridge>,
    draft_id: String,
    code: String,
) -> Result<EnrollmentView, PublicError> {
    let mut code = code;
    let result = claim(&bridge, &draft_id, &code, Instant::now()).await;
    code.zeroize();
    result
}

#[tauri::command]
pub(crate) async fn complete_remote_pairing(
    bridge: State<'_, JetBridge>,
    ticket_id: String,
) -> Result<PlaneView, PublicError> {
    complete(&bridge, &ticket_id, Instant::now()).await
}

/// Drops a draft or ticket. No Plane Command is sent: the offer expires on
/// the target, whose owner can also close pairing there.
#[tauri::command]
pub(crate) fn cancel_remote_pairing(
    bridge: State<'_, JetBridge>,
    id: String,
) -> Result<(), PublicError> {
    if let Ok(id) = Uuid::parse_str(&id) {
        bridge.enrollment.cancel(id);
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn forget_remote_plane(
    bridge: State<'_, JetBridge>,
    plane_id: String,
) -> Result<PlanesView, PublicError> {
    forget(&bridge, &plane_id).await
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{cell::RefCell, sync::Arc};

    use jet_protocol::{
        ErrorCategory, PairedClient, PairedClientAccess, PairingMethod, PendingPairing,
    };

    use super::*;
    use crate::jet::{
        keystore::{
            tests::{CountingStore, Fail},
            IdentityKeys,
        },
        pairing_transcript::tests::golden,
        planes::{
            remote::tests::{
                answer_pairing, open, serve_status, welcome, wire, Opening, CLIENT, PLANE,
            },
            spawner::fake::FakeSpawner,
        },
    };

    thread_local! {
        pub(crate) static DROPPED_CLAIMS: RefCell<Vec<[u8; 8]>> = const { RefCell::new(Vec::new()) };
    }

    const OFFER: Uuid = Uuid::from_u128(0x0ff);

    pub(crate) struct Setup {
        pub(crate) directory: tempfile::TempDir,
        pub(crate) bridge: JetBridge,
        pub(crate) spawner: Arc<FakeSpawner>,
        pub(crate) keys: Arc<IdentityKeys>,
        pub(crate) store: Arc<CountingStore>,
    }

    pub(crate) fn setup() -> Setup {
        let directory = tempfile::tempdir().unwrap();
        let spawner = Arc::new(FakeSpawner::default());
        let store = Arc::new(CountingStore::new(Fail::Nothing));
        let keys = Arc::new(IdentityKeys::new(store.clone()));
        let bridge = JetBridge::for_test(directory.path(), CLIENT, spawner.clone(), keys.clone());
        Setup {
            directory,
            bridge,
            spawner,
            keys,
            store,
        }
    }

    fn pending(progress: PairingProgress) -> PendingPairing {
        PendingPairing {
            offer_id: OFFER,
            method: PairingMethod::ManualCode,
            progress,
            attempts_remaining: 5,
            opened_at_unix_ms: 1_700_000_000_000,
            expires_at_unix_ms: 1_700_000_120_000,
        }
    }

    /// A target that accepts the claim and answers with a transcript for
    /// `plane` and the authentication string the Plane would compute.
    pub(crate) fn claim_script(
        spawner: &FakeSpawner,
        plane: Uuid,
        key: [u8; 32],
        tamper_string: bool,
    ) -> Arc<Mutex<Option<(Uuid, String)>>> {
        let seen = Arc::new(Mutex::new(None));
        let record = seen.clone();
        spawner.push(move |stream| async move {
            let Opening::Pairing {
                mut writer,
                request,
            } = open(stream).await
            else {
                panic!("expected a pairing request");
            };
            let RemotePairingRequest::Claim {
                command_id,
                secret,
                key: presented,
            } = request
            else {
                panic!("expected a claim");
            };
            assert_eq!(presented.key, key);
            *record.lock().unwrap() = Some((command_id, secret));
            let signing_bytes = golden(plane, OFFER, CLIENT, &key);
            let validated = validate(&signing_bytes, OFFER, CLIENT, &key, None).unwrap();
            let mut string = authentication_string(&validated);
            if tamper_string {
                string = "000-000".into();
            }
            answer_pairing(
                &mut writer,
                RemotePairingResponse::Claimed {
                    pending: pending(PairingProgress::AwaitingConfirmation {
                        client_id: CLIENT,
                        authentication_string: string,
                    }),
                    signing_bytes,
                },
            )
            .await;
            Some(0)
        });
        seen
    }

    pub(crate) fn complete_script(
        spawner: &FakeSpawner,
        answer: Result<[u8; 32], &'static str>,
    ) -> Arc<Mutex<Option<Uuid>>> {
        let seen = Arc::new(Mutex::new(None));
        let record = seen.clone();
        spawner.push(move |stream| async move {
            let Opening::Pairing {
                mut writer,
                request,
            } = open(stream).await
            else {
                panic!("expected a pairing request");
            };
            let RemotePairingRequest::Complete {
                command_id,
                offer_id,
                ..
            } = request
            else {
                panic!("expected a completion");
            };
            assert_eq!(offer_id, OFFER);
            *record.lock().unwrap() = Some(command_id);
            let response = match answer {
                Ok(key) => RemotePairingResponse::Completed {
                    client: PairedClient {
                        client_id: CLIENT,
                        key: ClientPublicKey {
                            algorithm: PairingKeyAlgorithm::Ed25519,
                            key,
                        },
                        pairing_protocol: "jet.pairing.v1".into(),
                        access: PairedClientAccess::Enabled,
                        paired_at_unix_ms: 1,
                    },
                },
                Err(code) => RemotePairingResponse::Rejected {
                    error: wire(ErrorCategory::Conflict, code),
                },
            };
            answer_pairing(&mut writer, response).await;
            Some(0)
        });
        seen
    }

    pub(crate) fn login_script(spawner: &FakeSpawner, key: [u8; 32], plane: Uuid) {
        spawner.push(move |stream| async move {
            let (reader, writer) = welcome(open(stream).await, &key).await;
            serve_status(reader, writer, plane).await;
            Some(0)
        });
    }

    pub(crate) fn unauthorized_script(spawner: &FakeSpawner) {
        spawner.push(|stream| async move {
            let Opening::Login { mut writer, .. } = open(stream).await else {
                panic!("expected a login");
            };
            crate::jet::planes::remote::tests::reject(
                &mut writer,
                wire(ErrorCategory::Unauthorized, "connection.unauthorized"),
            )
            .await;
            Some(0)
        });
    }

    fn draft_of(result: AddPlaneResult) -> String {
        match result {
            AddPlaneResult::PairingRequired { draft_id, .. } => draft_id,
            AddPlaneResult::Connected { .. } => panic!("expected pairing to be required"),
        }
    }

    /// Adds and fully pairs `destination`, returning its Plane handle.
    pub(crate) async fn pair(setup: &Setup, destination: &str, plane: Uuid) -> String {
        let now = Instant::now();
        let draft = draft_of(add(&setup.bridge, destination, None, now).await.unwrap());
        let key = setup
            .keys
            .ensure_public(CLIENT, Credential::Durable)
            .await
            .unwrap();
        claim_script(&setup.spawner, plane, key, false);
        let enrollment = claim(&setup.bridge, &draft, "1234-5678", now)
            .await
            .unwrap();
        complete_script(&setup.spawner, Ok(key));
        login_script(&setup.spawner, key, plane);
        let view = complete(&setup.bridge, &enrollment.ticket_id, now)
            .await
            .unwrap();
        serde_json::to_value(&view).unwrap()["planeId"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn enrollment_claims_waits_for_confirmation_and_completes_late() {
        let setup = setup();
        let now = Instant::now();
        let draft = draft_of(
            add(&setup.bridge, "alice@build-box", None, now)
                .await
                .unwrap(),
        );
        // No key existed, so the probe ran and nothing was spawned yet.
        assert_eq!(setup.spawner.spawns(), 0);
        assert_eq!(setup.keys.view().key, "not_created");

        let key = setup
            .keys
            .ensure_public(CLIENT, Credential::Durable)
            .await
            .unwrap();
        let claimed = claim_script(&setup.spawner, PLANE, key, false);
        let enrollment = claim(&setup.bridge, &draft, " 1234 5678 ", now)
            .await
            .unwrap();
        let json = serde_json::to_value(&enrollment).unwrap();
        assert_eq!(json["confirmByUnixMs"], "1700000120000");
        assert_eq!(json["destination"], "alice@build-box");
        assert_eq!(json["planeIdentity"], &PLANE.simple().to_string()[..8]);
        assert_eq!(json["sessionOnly"], false);
        let validated = validate(
            &golden(PLANE, OFFER, CLIENT, &key),
            OFFER,
            CLIENT,
            &key,
            None,
        )
        .unwrap();
        assert_eq!(
            json["authenticationString"],
            authentication_string(&validated)
        );
        assert_eq!(claimed.lock().unwrap().as_ref().unwrap().1, "12345678");

        // Not confirmed yet: a definite refusal keeps the ticket and makes
        // the next attempt a new command.
        let first = complete_script(&setup.spawner, Err("pairing.not_confirmed"));
        let error = complete(&setup.bridge, &enrollment.ticket_id, now)
            .await
            .unwrap_err();
        assert_eq!(error.code, "pairing.not_confirmed");
        let first = first.lock().unwrap().unwrap();

        // Three minutes later the shell still attempts: no local expiry.
        let second = complete_script(&setup.spawner, Ok(key));
        login_script(&setup.spawner, key, PLANE);
        let later = now + Duration::from_secs(3 * 60);
        let view = complete(&setup.bridge, &enrollment.ticket_id, later)
            .await
            .unwrap();
        assert_ne!(second.lock().unwrap().unwrap(), first);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["connection"]["state"], "online");
        assert_eq!(json["credential"], "durable");
        assert_eq!(json["label"], "alice@build-box");
        let saved = std::fs::read_to_string(setup.directory.path().join("planes.json")).unwrap();
        assert!(saved.contains(&PLANE.to_string()));
        assert!(saved.contains("alice@build-box"));
        assert_eq!(setup.spawner.pending(), 0);

        // The ticket is gone after completion.
        assert_eq!(
            complete(&setup.bridge, &enrollment.ticket_id, later)
                .await
                .unwrap_err()
                .code,
            "enrollment.ticket_expired"
        );
        // Past the native cap a ticket is gone as well. The key exists now,
        // so adding first tries a login, which this target refuses.
        unauthorized_script(&setup.spawner);
        let draft = draft_of(add(&setup.bridge, "ci-box", None, now).await.unwrap());
        claim_script(&setup.spawner, Uuid::from_u128(0x77), key, false);
        let other = claim(&setup.bridge, &draft, "12345678", now).await.unwrap();
        assert_eq!(
            complete(&setup.bridge, &other.ticket_id, now + TICKET_LIFETIME)
                .await
                .unwrap_err()
                .code,
            "enrollment.ticket_expired"
        );
    }

    #[tokio::test]
    async fn a_server_authentication_string_that_disagrees_is_refused() {
        let setup = setup();
        let now = Instant::now();
        let draft = draft_of(add(&setup.bridge, "build-box", None, now).await.unwrap());
        let key = setup
            .keys
            .ensure_public(CLIENT, Credential::Durable)
            .await
            .unwrap();
        claim_script(&setup.spawner, PLANE, key, true);
        let error = claim(&setup.bridge, &draft, "12345678", now)
            .await
            .unwrap_err();
        assert_eq!(error.code, "enrollment.transcript_invalid");
    }

    #[tokio::test]
    async fn an_existing_key_logs_in_directly_and_duplicates_are_refused() {
        let setup = setup();
        let key = setup
            .keys
            .ensure_public(CLIENT, Credential::Durable)
            .await
            .unwrap();
        login_script(&setup.spawner, key, PLANE);
        let now = Instant::now();
        let AddPlaneResult::Connected { plane } = add(&setup.bridge, "alice@build-box", None, now)
            .await
            .unwrap()
        else {
            panic!("expected a direct login");
        };
        assert_eq!(
            serde_json::to_value(&plane).unwrap()["connection"]["state"],
            "online"
        );
        let existing = serde_json::to_value(&plane).unwrap()["planeId"]
            .as_str()
            .unwrap()
            .to_owned();
        // Same address, case-insensitive host: refused before any ssh, and
        // the error names the entry it duplicates.
        let spawns = setup.spawner.spawns();
        let error = add(&setup.bridge, "alice@BUILD-BOX", None, now)
            .await
            .unwrap_err();
        assert_eq!(error.code, "plane.already_registered");
        assert_eq!(error.plane_id.as_deref(), Some(existing.as_str()));
        assert_eq!(setup.spawner.spawns(), spawns);
        // Another alias that reaches the same Plane is refused by identity.
        login_script(&setup.spawner, key, PLANE);
        let error = add(&setup.bridge, "build-alias", None, now)
            .await
            .unwrap_err();
        assert_eq!(error.code, "plane.already_registered");
        assert_eq!(error.plane_id.as_deref(), Some(existing.as_str()));
        assert_eq!(setup.bridge.planes.remote_count(), 1);
    }

    #[tokio::test]
    async fn local_identity_learned_later_marks_the_duplicate() {
        let setup = setup();
        let key = setup
            .keys
            .ensure_public(CLIENT, Credential::Durable)
            .await
            .unwrap();
        login_script(&setup.spawner, key, PLANE);
        let AddPlaneResult::Connected { plane } =
            add(&setup.bridge, "localhost", None, Instant::now())
                .await
                .unwrap()
        else {
            panic!("expected a direct login");
        };
        let plane_id = serde_json::to_value(&plane).unwrap()["planeId"]
            .as_str()
            .unwrap()
            .to_owned();
        // Local jetd reports the same identity once it is reachable.
        setup.bridge.planes.observe_status(
            PlaneId::Local,
            &crate::jet::planes::remote::tests::status(PLANE),
        );
        let view = serde_json::to_value(
            setup
                .bridge
                .planes
                .view(PlaneId::parse(&plane_id).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(view["connection"]["state"], "failed");
        assert_eq!(
            view["connection"]["error"]["code"],
            "plane.duplicates_local"
        );
        let saved = std::fs::read_to_string(setup.directory.path().join("planes.json")).unwrap();
        assert!(saved.contains(&format!(r#""localIdentity": "{PLANE}""#)));

        // Neither Retry nor Pair again brings the duplicate online: only
        // Forget ends it, whatever the webview asks.
        let spawns = setup.spawner.spawns();
        let (_, client) = setup.bridge.plane(Some(&plane_id)).unwrap();
        client.reset().await;
        assert!(client.status().await.is_err());
        let error = repair(&setup.bridge, &plane_id, None, Instant::now())
            .await
            .unwrap_err();
        assert_eq!(error.code, "plane.duplicates_local");
        assert_eq!(setup.spawner.spawns(), spawns);
        let view = serde_json::to_value(
            setup
                .bridge
                .planes
                .view(PlaneId::parse(&plane_id).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            view["connection"]["error"]["code"],
            "plane.duplicates_local"
        );
    }

    #[tokio::test]
    async fn pair_again_updates_the_entry_in_place() {
        let setup = setup();
        let plane_id = pair(&setup, "alice@build-box", PLANE).await;
        let plane = PlaneId::parse(&plane_id).unwrap();
        // The owner revoked this computer: the login is refused and sticky.
        setup
            .bridge
            .planes
            .fail(plane, PublicError::internal())
            .await;
        unauthorized_script(&setup.spawner);
        let now = Instant::now();
        let draft = draft_of(repair(&setup.bridge, &plane_id, None, now).await.unwrap());
        let key = setup
            .keys
            .ensure_public(CLIENT, Credential::Durable)
            .await
            .unwrap();
        claim_script(&setup.spawner, PLANE, key, false);
        let enrollment = claim(&setup.bridge, &draft, "87654321", now).await.unwrap();
        complete_script(&setup.spawner, Ok(key));
        login_script(&setup.spawner, key, PLANE);
        let view = complete(&setup.bridge, &enrollment.ticket_id, now)
            .await
            .unwrap();
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["planeId"], plane_id, "the same PlaneId is kept");
        assert_eq!(json["connection"]["state"], "online");
        assert_eq!(setup.bridge.planes.remote_count(), 1);

        // A repair that reaches another Plane is refused.
        setup
            .bridge
            .planes
            .fail(plane, PublicError::internal())
            .await;
        unauthorized_script(&setup.spawner);
        let draft = draft_of(repair(&setup.bridge, &plane_id, None, now).await.unwrap());
        claim_script(&setup.spawner, Uuid::from_u128(0xbad), key, false);
        assert_eq!(
            claim(&setup.bridge, &draft, "87654321", now)
                .await
                .unwrap_err()
                .code,
            "plane.identity_changed"
        );
        assert_eq!(
            repair(&setup.bridge, "local", None, now)
                .await
                .unwrap_err()
                .code,
            "plane.local_not_removable"
        );
    }

    #[tokio::test]
    async fn the_code_is_never_retained_in_clear_and_is_wiped_when_dropped() {
        let setup = setup();
        let now = Instant::now();
        let draft = draft_of(add(&setup.bridge, "build-box", None, now).await.unwrap());
        // First attempt: the target drops the connection without answering.
        let seen = Arc::new(Mutex::new(None));
        let record = seen.clone();
        setup.spawner.push(move |stream| async move {
            let Opening::Pairing { request, .. } = open(stream).await else {
                panic!("expected a pairing request");
            };
            if let RemotePairingRequest::Claim { command_id, .. } = request {
                *record.lock().unwrap() = Some(command_id);
            }
            Some(0)
        });
        let error = claim(&setup.bridge, &draft, "1234-5678", now)
            .await
            .unwrap_err();
        assert!(error.retryable, "{}", error.code);
        let first = seen.lock().unwrap().unwrap();

        // The uncertain retry sends the exact same body.
        let key = setup
            .keys
            .ensure_public(CLIENT, Credential::Durable)
            .await
            .unwrap();
        let retried = claim_script(&setup.spawner, PLANE, key, false);
        DROPPED_CLAIMS.with(|dropped| dropped.borrow_mut().clear());
        claim(&setup.bridge, &draft, "12345678", now).await.unwrap();
        assert_eq!(retried.lock().unwrap().as_ref().unwrap().0, first);
        // Known outcome: the retained code was dropped, and wiped.
        DROPPED_CLAIMS.with(|dropped| {
            let dropped = dropped.borrow();
            assert!(!dropped.is_empty());
            assert!(dropped.iter().all(|digits| digits == &[0u8; 8]));
        });

        for text in [
            format!("{:?}", setup.bridge.enrollment),
            format!("{:?}", setup.bridge.pairing),
            format!("{:?}", setup.bridge.planes),
            std::fs::read_to_string(setup.directory.path().join("planes.json")).unwrap_or_default(),
        ] {
            assert!(!text.contains("12345678"), "{text}");
            assert!(!text.contains("1234-5678"), "{text}");
        }
    }

    #[tokio::test]
    async fn invalid_codes_and_unknown_requests_are_refused_before_ssh() {
        let setup = setup();
        let now = Instant::now();
        let draft = draft_of(add(&setup.bridge, "build-box", None, now).await.unwrap());
        for invalid in ["1234567", "123456789", "1234-567a", "１２３４５６７８", ""] {
            assert_eq!(
                claim(&setup.bridge, &draft, invalid, now)
                    .await
                    .unwrap_err()
                    .code,
                "enrollment.code_invalid",
                "{invalid}"
            );
        }
        assert_eq!(
            claim(&setup.bridge, "nope", "12345678", now)
                .await
                .unwrap_err()
                .code,
            "enrollment.draft_expired"
        );
        assert_eq!(
            claim(
                &setup.bridge,
                &draft,
                "12345678",
                now + DRAFT_LIFETIME + Duration::from_secs(1)
            )
            .await
            .unwrap_err()
            .code,
            "enrollment.draft_expired"
        );
        assert_eq!(
            add(&setup.bridge, "-oProxyCommand=x", None, now)
                .await
                .unwrap_err()
                .code,
            "plane.destination_invalid"
        );
        assert_eq!(setup.spawner.spawns(), 0);
    }

    #[tokio::test]
    async fn secure_storage_failures_stop_before_a_draft_and_session_only_works() {
        let setup = setup();
        *setup.store.fail.lock().unwrap() = Fail::Create;
        let now = Instant::now();
        assert_eq!(
            add(&setup.bridge, "build-box", None, now)
                .await
                .unwrap_err()
                .code,
            "identity.secret_store_unavailable"
        );
        // Pair for this session only: no durable store access at all.
        let accesses = setup.store.count();
        let draft = draft_of(
            add(&setup.bridge, "build-box", Some(true), now)
                .await
                .unwrap(),
        );
        let key = setup
            .keys
            .ensure_public(CLIENT, Credential::Session)
            .await
            .unwrap();
        claim_script(&setup.spawner, PLANE, key, false);
        let enrollment = claim(&setup.bridge, &draft, "12345678", now).await.unwrap();
        assert!(serde_json::to_value(&enrollment).unwrap()["sessionOnly"]
            .as_bool()
            .unwrap());
        complete_script(&setup.spawner, Ok(key));
        login_script(&setup.spawner, key, PLANE);
        let view = complete(&setup.bridge, &enrollment.ticket_id, now)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&view).unwrap()["credential"],
            "session"
        );
        assert_eq!(setup.store.count(), accesses);
        let saved = std::fs::read_to_string(setup.directory.path().join("planes.json")).unwrap();
        assert!(saved.contains(r#""credential": "session""#));

        // After a restart the session key is gone: the Plane says so.
        let restarted = JetBridge::for_test(
            setup.directory.path(),
            CLIENT,
            setup.spawner.clone(),
            Arc::new(IdentityKeys::new(setup.store.clone())),
        );
        let views = serde_json::to_value(restarted.planes.views().unwrap()).unwrap();
        assert_eq!(
            views[1]["connection"]["error"]["code"],
            "identity.session_ended"
        );
    }

    #[tokio::test]
    async fn forget_removes_the_plane_and_its_pending_repairs_only_here() {
        let setup = setup();
        let plane_id = pair(&setup, "build-box", PLANE).await;
        let plane = PlaneId::parse(&plane_id).unwrap();
        let (binding, _) = setup.bridge.plane(Some(&plane_id)).unwrap();
        let spawns = setup.spawner.spawns();
        let view = forget(&setup.bridge, &plane_id).await.unwrap();
        assert_eq!(
            serde_json::to_value(&view).unwrap()["planes"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            setup.spawner.spawns(),
            spawns,
            "nothing is sent to the Plane"
        );
        assert_eq!(
            setup.bridge.bound(&binding).err().unwrap().code,
            "plane.review_moved"
        );
        assert!(!setup.bridge.planes.contains(plane));
        assert_eq!(
            forget(&setup.bridge, "local").await.unwrap_err().code,
            "plane.local_not_removable"
        );
        let saved = std::fs::read_to_string(setup.directory.path().join("planes.json")).unwrap();
        assert!(!saved.contains("build-box"));
    }

    #[test]
    fn cancelling_a_ticket_ends_its_draft_too() {
        let state = EnrollmentState::default();
        let now = Instant::now();
        let draft = Draft {
            id: Uuid::from_u128(1),
            destination: "host".into(),
            endpoint: SshEndpoint::new("host").unwrap(),
            repair_of: None,
            credential: Credential::Durable,
            created: now,
        };
        state.add_draft(draft, now).unwrap();
        let digits = Zeroizing::new(*b"12345678");
        let first = state
            .claim_command(Uuid::from_u128(1), &digits, now)
            .unwrap();
        assert_eq!(
            state
                .claim_command(Uuid::from_u128(1), &digits, now)
                .unwrap(),
            first
        );
        let other = Zeroizing::new(*b"12345679");
        assert_ne!(
            state
                .claim_command(Uuid::from_u128(1), &other, now)
                .unwrap(),
            first
        );
        state.cancel(Uuid::from_u128(1));
        assert_eq!(
            state.draft(Uuid::from_u128(1), now).err().unwrap().code,
            "enrollment.draft_expired"
        );
        assert!(state.claims.lock().unwrap().is_empty());
        for index in 0..6 {
            state
                .add_draft(
                    Draft {
                        id: Uuid::from_u128(10 + index),
                        destination: "host".into(),
                        endpoint: SshEndpoint::new("host").unwrap(),
                        repair_of: None,
                        credential: Credential::Durable,
                        created: now,
                    },
                    now,
                )
                .unwrap();
        }
        assert_eq!(state.drafts.lock().unwrap().len(), MAX_DRAFTS);
        assert!(!format!("{state:?}").contains("host"));
    }
}
