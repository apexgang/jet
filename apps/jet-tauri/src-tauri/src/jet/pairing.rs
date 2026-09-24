//! Owner-side Pairing on any Plane this computer is connected to (ADR-0017):
//! the Pairing gate, one-time manual-code offers, typed confirmation of a
//! claim, and enabling, disabling or revoking already Paired clients.
//!
//! The owner operations are authorized for local interactive clients and for
//! remote Paired clients alike, so every command takes an explicit Plane
//! handle. The shell holds every command ID natively (ADR-0093): an ID is
//! kept only while an outcome is uncertain and dropped on success or on a
//! definite refusal, so a retry after a lost reply resends the exact same
//! body and a later attempt after a refusal is a new request.
//!
//! Enrollment (this computer pairing itself with a remote Plane) lives in
//! `enrollment.rs`.
use std::{
    collections::HashMap,
    fmt,
    hash::Hash,
    sync::Mutex,
    time::{Duration, Instant},
};

use jet_client::ClientError;
use jet_protocol::{
    PairedClient, PairedClientAccess, PairingDisclosure, PairingEnd, PairingGate, PairingMethod,
    PairingProgress, PairingSnapshot, PendingPairing, PAIRING_MINOR,
};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

pub(crate) use super::keystore::fingerprint;
use super::{
    client::{Connection, PlaneClient},
    command_ids::{definite, settle_command},
    errors::PublicError,
    planes::{PlaneBinding, PlaneHealth, PlaneId},
    JetBridge,
};

/// Pending command IDs per kind of Pairing Command. Each map holds one entry
/// per exact body, so a handful of uncertain requests is the realistic worst
/// case; the cap only bounds a misbehaving webview.
const MAX_PENDING_COMMANDS: usize = 64;
/// Prepared client changes retained at once.
const MAX_REVIEWS: usize = 32;
/// How long an unattempted client-change review stays valid.
const REVIEW_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// Paired clients forwarded in one view.
const MAX_CLIENTS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Gate {
    Open,
    Closed,
}

impl Gate {
    fn parse(value: &str) -> Result<Self, PublicError> {
        match value {
            "open" => Ok(Self::Open),
            "closed" => Ok(Self::Closed),
            _ => Err(PublicError::invalid_input(
                "owner.gate_invalid",
                "Choose whether Pairing is open or closed.",
            )),
        }
    }

    fn wire(self) -> PairingGate {
        match self {
            Self::Open => PairingGate::Open,
            Self::Closed => PairingGate::Closed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Enable,
    Disable,
    Revoke,
}

impl Change {
    fn parse(value: &str) -> Result<Self, PublicError> {
        match value {
            "enable" => Ok(Self::Enable),
            "disable" => Ok(Self::Disable),
            "revoke" => Ok(Self::Revoke),
            _ => Err(PublicError::invalid_input(
                "owner.change_invalid",
                "Choose to enable, disable or revoke a paired computer.",
            )),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Revoke => "revoke",
        }
    }

    /// The access an enable or disable leaves the client with.
    fn access(self) -> Option<PairedClientAccess> {
        match self {
            Self::Enable => Some(PairedClientAccess::Enabled),
            Self::Disable => Some(PairedClientAccess::Disabled),
            Self::Revoke => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConfirmKey {
    plane: PlaneId,
    offer_id: Uuid,
    authentication_string: String,
}

/// One prepared change to a Paired client. The review ID is also the daemon
/// command ID, and the stored binding is the only Plane it executes on.
struct Review {
    order: u64,
    binding: PlaneBinding,
    client_id: Uuid,
    change: Change,
    /// The target is this computer and the Plane is reached remotely, so the
    /// change can end the very connection it is sent over.
    via_this_plane_connection: bool,
    created: Instant,
    attempted: bool,
    result: Option<ClientChangeReceipt>,
}

impl Review {
    /// One unresolved change is allowed per Paired client on each Plane.
    fn scope(&self) -> (PlaneId, Uuid) {
        (self.binding.plane, self.client_id)
    }

    fn unresolved(&self) -> bool {
        self.attempted && self.result.is_none()
    }

    fn expired(&self, now: Instant) -> bool {
        !self.attempted && now.saturating_duration_since(self.created) > REVIEW_LIFETIME
    }
}

struct Attempt {
    binding: PlaneBinding,
    client_id: Uuid,
    change: Change,
    via_this_plane_connection: bool,
    known: Option<ClientChangeReceipt>,
}

/// Native owner-side Pairing authority: command IDs for uncertain requests
/// and the immutable client-change ledger. It holds no pairing codes.
#[derive(Default)]
pub(crate) struct PairingState {
    gate_commands: Mutex<HashMap<(PlaneId, Gate), Uuid>>,
    offer_commands: Mutex<HashMap<PlaneId, Uuid>>,
    confirm_commands: Mutex<HashMap<ConfirmKey, Uuid>>,
    reviews: Mutex<HashMap<Uuid, Review>>,
}

impl fmt::Debug for PairingState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reviews = self
            .reviews
            .lock()
            .map(|reviews| reviews.len())
            .unwrap_or(0);
        formatter
            .debug_struct("PairingState")
            .field("reviews", &reviews)
            .finish_non_exhaustive()
    }
}

impl PairingState {
    /// Drops the command IDs kept for a forgotten Plane. Its reviews stay
    /// (bounded and expiring) so that executing one answers a receipt with
    /// `plane.review_moved` instead of silently disappearing.
    pub(crate) fn forget_plane(&self, plane: PlaneId) {
        if let Ok(mut commands) = self.gate_commands.lock() {
            commands.retain(|(owner, _), _| *owner != plane);
        }
        if let Ok(mut commands) = self.offer_commands.lock() {
            commands.remove(&plane);
        }
        if let Ok(mut commands) = self.confirm_commands.lock() {
            commands.retain(|key, _| key.plane != plane);
        }
    }

    fn prepare(
        &self,
        binding: PlaneBinding,
        client_id: Uuid,
        change: Change,
        via_this_plane_connection: bool,
        now: Instant,
    ) -> Result<Uuid, PublicError> {
        let mut reviews = self.reviews.lock().map_err(|_| PublicError::internal())?;
        reviews.retain(|_, review| !review.expired(now));
        if reviews
            .values()
            .any(|review| review.scope() == (binding.plane, client_id) && review.unresolved())
        {
            return Err(request_unresolved());
        }
        if reviews.len() >= MAX_REVIEWS {
            // Never evict an unresolved review: it is the only record of an
            // uncertain request. Unattempted reviews go first, then the
            // oldest resolved receipts.
            let oldest = reviews
                .iter()
                .filter(|(_, review)| !review.attempted)
                .min_by_key(|(_, review)| review.order)
                .or_else(|| {
                    reviews
                        .iter()
                        .filter(|(_, review)| review.result.is_some())
                        .min_by_key(|(_, review)| review.order)
                })
                .map(|(id, _)| *id);
            match oldest {
                Some(id) => {
                    reviews.remove(&id);
                }
                None => {
                    return Err(PublicError::invalid_input(
                        "owner.request_limit",
                        "Too many paired-computer changes are waiting. Resolve them before preparing another.",
                    ))
                }
            }
        }
        let order = reviews
            .values()
            .map(|review| review.order)
            .max()
            .unwrap_or(0)
            + 1;
        let id = Uuid::new_v4();
        reviews.insert(
            id,
            Review {
                order,
                binding,
                client_id,
                change,
                via_this_plane_connection,
                created: now,
                attempted: false,
                result: None,
            },
        );
        Ok(id)
    }

    fn attempt(&self, id: Uuid, now: Instant) -> Result<Attempt, PublicError> {
        let mut reviews = self.reviews.lock().map_err(|_| PublicError::internal())?;
        let review = reviews.get(&id).ok_or_else(review_expired)?;
        if review.expired(now) {
            reviews.remove(&id);
            return Err(review_expired());
        }
        let scope = review.scope();
        if reviews
            .iter()
            .any(|(other, review)| *other != id && review.scope() == scope && review.unresolved())
        {
            return Err(request_unresolved());
        }
        let review = reviews.get_mut(&id).ok_or_else(PublicError::internal)?;
        review.attempted = true;
        Ok(Attempt {
            binding: review.binding,
            client_id: review.client_id,
            change: review.change,
            via_this_plane_connection: review.via_this_plane_connection,
            known: review.result.clone(),
        })
    }

    fn record(&self, id: Uuid, receipt: ClientChangeReceipt) -> Result<(), PublicError> {
        if let Some(review) = self
            .reviews
            .lock()
            .map_err(|_| PublicError::internal())?
            .get_mut(&id)
        {
            review.result = Some(receipt);
        }
        Ok(())
    }
}

fn review_expired() -> PublicError {
    PublicError::invalid_input(
        "owner.review_expired",
        "Review this change to the paired computer again.",
    )
}

fn request_unresolved() -> PublicError {
    PublicError::conflict(
        "owner.request_unresolved",
        "Resolve the previous change to this paired computer first.",
    )
}

/// The command ID for one exact body, created on first use.
fn command_id<K: Hash + Eq>(
    commands: &Mutex<HashMap<K, Uuid>>,
    key: K,
) -> Result<Uuid, PublicError> {
    let mut commands = commands.lock().map_err(|_| PublicError::internal())?;
    if let Some(id) = commands.get(&key) {
        return Ok(*id);
    }
    if commands.len() >= MAX_PENDING_COMMANDS {
        return Err(PublicError::invalid_input(
            "client.too_many_pending_commands",
            "Finish or retry an earlier Pairing change before starting another one.",
        ));
    }
    Ok(*commands.entry(key).or_insert_with(Uuid::new_v4))
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MutationsView {
    allowed: bool,
    /// `security_degraded` or `store_read_only`.
    reason: Option<&'static str>,
}

impl MutationsView {
    fn from_health(health: PlaneHealth) -> Self {
        let reason = health.mutations_paused();
        Self {
            allowed: reason.is_none(),
            reason,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairingView {
    plane_id: String,
    cursor: String,
    gate: &'static str,
    pending: Option<PendingPairingView>,
    clients: Vec<PairedClientView>,
    this_client_id: String,
    mutations: MutationsView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingPairingView {
    offer_id: String,
    method: &'static str,
    progress: ProgressView,
    attempts_remaining: u32,
    opened_at_unix_ms: String,
    expires_at_unix_ms: String,
}

/// How far an offer has got. The authentication string is deliberately
/// omitted: the owner types the string shown on the other computer and jetd
/// compares it, so this view can never be clicked through.
#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum ProgressView {
    Offered,
    AwaitingConfirmation {
        client_id: String,
        client_is_this_computer: bool,
    },
    Confirmed {
        client_id: String,
    },
    Ended {
        reason: &'static str,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairedClientView {
    client_id: String,
    fingerprint: String,
    access: &'static str,
    paired_at_unix_ms: String,
    pairing_protocol: String,
    is_this_computer: bool,
}

/// The one-time code crosses into the webview so the owner can read it out.
/// It is validated here, never stored natively after this reply and never
/// logged; this type deliberately has no `Debug`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OfferDisclosureView {
    offer_id: String,
    code: Option<String>,
    already_disclosed: bool,
    expires_at_unix_ms: String,
    attempts_remaining: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClientChangeReview {
    review_id: String,
    plane_id: String,
    plane_label: String,
    plane_identity: Option<String>,
    client_id: String,
    fingerprint: String,
    change: &'static str,
    is_this_computer: bool,
    via_this_plane_connection: bool,
}

/// What became of one reviewed client change. A definite refusal is a
/// receipt; an uncertain transport outcome is an error and stays retryable
/// with the same review.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum ClientChangeReceipt {
    Applied {
        client: Option<PairedClientView>,
        revoked: bool,
    },
    /// This computer changed its own access over the connection the change
    /// ended. The reconnect was refused, so the change almost certainly
    /// committed, but it cannot be read back from here.
    AppliedUnverified {
        change: &'static str,
    },
    Refused {
        error: PublicError,
    },
}

fn gate_name(gate: PairingGate) -> &'static str {
    match gate {
        PairingGate::Open => "open",
        PairingGate::Closed => "closed",
    }
}

fn access_name(access: PairedClientAccess) -> &'static str {
    match access {
        PairedClientAccess::Enabled => "enabled",
        PairedClientAccess::Disabled => "disabled",
    }
}

fn end_reason(reason: PairingEnd) -> &'static str {
    match reason {
        PairingEnd::Expired => "expired",
        PairingEnd::TooManyAttempts => "too_many_attempts",
        PairingEnd::GateClosed => "gate_closed",
    }
}

/// A Pairing protocol name such as `jet.pairing.v1`, bounded and allowlisted.
fn pairing_protocol(value: &str) -> String {
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.')
    {
        value.to_owned()
    } else {
        "unknown".to_owned()
    }
}

fn client_view(client: &PairedClient, this_client: Uuid) -> PairedClientView {
    PairedClientView {
        client_id: client.client_id.to_string(),
        fingerprint: fingerprint(&client.key.key),
        access: access_name(client.access),
        paired_at_unix_ms: client.paired_at_unix_ms.to_string(),
        pairing_protocol: pairing_protocol(&client.pairing_protocol),
        is_this_computer: client.client_id == this_client,
    }
}

fn pending_view(pending: &PendingPairing, this_client: Uuid) -> PendingPairingView {
    PendingPairingView {
        offer_id: pending.offer_id.to_string(),
        method: match pending.method {
            PairingMethod::ManualCode => "manual_code",
            PairingMethod::QrPayload { .. } => "qr_payload",
        },
        progress: match &pending.progress {
            PairingProgress::Offered => ProgressView::Offered,
            PairingProgress::AwaitingConfirmation { client_id, .. } => {
                ProgressView::AwaitingConfirmation {
                    client_id: client_id.to_string(),
                    client_is_this_computer: *client_id == this_client,
                }
            }
            PairingProgress::Confirmed { client_id, .. } => ProgressView::Confirmed {
                client_id: client_id.to_string(),
            },
            PairingProgress::Ended { reason } => ProgressView::Ended {
                reason: end_reason(*reason),
            },
        },
        attempts_remaining: pending.attempts_remaining,
        opened_at_unix_ms: pending.opened_at_unix_ms.to_string(),
        expires_at_unix_ms: pending.expires_at_unix_ms.to_string(),
    }
}

fn pairing_view(
    plane: PlaneId,
    snapshot: &PairingSnapshot,
    this_client: Uuid,
    health: PlaneHealth,
) -> PairingView {
    PairingView {
        plane_id: plane.to_string(),
        cursor: snapshot.cursor.to_string(),
        gate: gate_name(snapshot.gate),
        pending: snapshot
            .pending
            .as_ref()
            .map(|pending| pending_view(pending, this_client)),
        clients: snapshot
            .clients
            .iter()
            .take(MAX_CLIENTS)
            .map(|client| client_view(client, this_client))
            .collect(),
        this_client_id: this_client.to_string(),
        mutations: MutationsView::from_health(health),
    }
}

/// The disclosed code only when it has the documented `dddd-dddd` shape.
fn manual_code(code: &str) -> Option<String> {
    let bytes = code.as_bytes();
    (bytes.len() == 9
        && bytes[4] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || byte.is_ascii_digit()))
    .then(|| code.to_owned())
}

/// Normalizes the typed authentication string to `ddd-ddd`: spaces are
/// ignored, and six digits with an optional hyphen after the third are
/// required.
fn authentication_string(value: &str) -> Result<String, PublicError> {
    let compact: String = value
        .chars()
        .filter(|character| *character != ' ')
        .collect();
    let digits = match compact.len() {
        6 => compact.clone(),
        7 if compact.as_bytes()[3] == b'-' => format!("{}{}", &compact[..3], &compact[4..]),
        _ => String::new(),
    };
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(PublicError::invalid_input(
            "owner.authentication_string_invalid",
            "Type the 6-digit code shown on the other computer.",
        ));
    }
    Ok(format!("{}-{}", &digits[..3], &digits[3..]))
}

fn parse_id(value: &str) -> Result<Uuid, PublicError> {
    Uuid::parse_str(value).map_err(|_| {
        PublicError::invalid_input(
            "owner.identifier_invalid",
            "Choose a valid pairing request or paired computer.",
        )
    })
}

fn client_error(error: &ClientError) -> PublicError {
    PublicError::from_client(error)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Reads Pairing and refreshes Plane health from a status read in the same
/// session, so the mutation controls reflect the Plane's current state.
async fn read_pairing(
    bridge: &JetBridge,
    plane: PlaneId,
    connection: &Connection,
) -> Result<PairingView, PublicError> {
    let status = connection
        .query(connection.status())
        .await
        .map_err(|error| client_error(&error))?;
    bridge.planes.observe_status(plane, &status);
    let snapshot = connection
        .query(connection.pairing())
        .await
        .map_err(|error| client_error(&error))?;
    bridge.planes.observe_success(plane, PAIRING_MINOR);
    Ok(pairing_view(
        plane,
        &snapshot,
        bridge.planes.local().client_id(),
        bridge.planes.health(plane),
    ))
}

async fn connect(client: &PlaneClient) -> Result<Connection, PublicError> {
    client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))
}

#[tauri::command]
pub(crate) async fn load_pairing(
    bridge: State<'_, JetBridge>,
    plane_id: String,
) -> Result<PairingView, PublicError> {
    load(&bridge, &plane_id).await
}

pub(crate) async fn load(bridge: &JetBridge, plane_id: &str) -> Result<PairingView, PublicError> {
    let (binding, client) = bridge.plane(Some(plane_id))?;
    async {
        let connection = connect(&client).await?;
        read_pairing(bridge, binding.plane, &connection).await
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn set_pairing_gate(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    gate: String,
) -> Result<PairingView, PublicError> {
    set_gate(&bridge, &plane_id, &gate).await
}

pub(crate) async fn set_gate(
    bridge: &JetBridge,
    plane_id: &str,
    gate: &str,
) -> Result<PairingView, PublicError> {
    let gate = Gate::parse(gate)?;
    let (binding, client) = bridge.plane(Some(plane_id))?;
    async {
        let key = (binding.plane, gate);
        let id = command_id(&bridge.pairing.gate_commands, key)?;
        let connection = connect(&client).await?;
        let outcome = connection
            .command(connection.set_pairing_gate(id, gate.wire()))
            .await;
        settle_command(&bridge.pairing.gate_commands, &key, &outcome)?;
        if outcome.map_err(|error| client_error(&error))? != gate.wire() {
            return Err(PublicError::internal());
        }
        read_pairing(bridge, binding.plane, &connection).await
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

/// Opens one manual-code offer. The code is disclosed once: a retry after a
/// lost reply is answered with `already_disclosed`, and the next call opens
/// a new offer because the command ID is dropped once the outcome is known.
#[tauri::command]
pub(crate) async fn open_pairing_offer(
    bridge: State<'_, JetBridge>,
    plane_id: String,
) -> Result<OfferDisclosureView, PublicError> {
    open_offer(&bridge, &plane_id).await
}

pub(crate) async fn open_offer(
    bridge: &JetBridge,
    plane_id: &str,
) -> Result<OfferDisclosureView, PublicError> {
    let (binding, client) = bridge.plane(Some(plane_id))?;
    async {
        let id = command_id(&bridge.pairing.offer_commands, binding.plane)?;
        let connection = connect(&client).await?;
        let outcome = connection
            .command(connection.open_pairing(id, PairingMethod::ManualCode))
            .await;
        settle_command(&bridge.pairing.offer_commands, &binding.plane, &outcome)?;
        let (pending, disclosure) = outcome.map_err(|error| client_error(&error))?;
        bridge.planes.observe_success(binding.plane, PAIRING_MINOR);
        if pending.method != PairingMethod::ManualCode {
            return Err(PublicError::internal());
        }
        let (code, already_disclosed) = match disclosure {
            PairingDisclosure::ManualCode { code } => (
                Some(manual_code(&code).ok_or_else(PublicError::internal)?),
                false,
            ),
            PairingDisclosure::AlreadyDisclosed => (None, true),
            PairingDisclosure::QrPayload { .. } => return Err(PublicError::internal()),
        };
        Ok(OfferDisclosureView {
            offer_id: pending.offer_id.to_string(),
            code,
            already_disclosed,
            expires_at_unix_ms: pending.expires_at_unix_ms.to_string(),
            attempts_remaining: pending.attempts_remaining,
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn confirm_pairing_request(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    offer_id: String,
    authentication_string: String,
) -> Result<PendingPairingView, PublicError> {
    confirm(&bridge, &plane_id, &offer_id, &authentication_string).await
}

pub(crate) async fn confirm(
    bridge: &JetBridge,
    plane_id: &str,
    offer_id: &str,
    authentication_string: &str,
) -> Result<PendingPairingView, PublicError> {
    let offer_id = parse_id(offer_id)?;
    let typed = self::authentication_string(authentication_string)?;
    let (binding, client) = bridge.plane(Some(plane_id))?;
    async {
        let key = ConfirmKey {
            plane: binding.plane,
            offer_id,
            authentication_string: typed.clone(),
        };
        let id = command_id(&bridge.pairing.confirm_commands, key.clone())?;
        let connection = connect(&client).await?;
        let outcome = connection
            .command(connection.confirm_pairing(id, offer_id, &typed))
            .await;
        settle_command(&bridge.pairing.confirm_commands, &key, &outcome)?;
        let pending = outcome.map_err(|error| client_error(&error))?;
        if pending.offer_id != offer_id {
            return Err(PublicError::internal());
        }
        Ok(pending_view(&pending, bridge.planes.local().client_id()))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

/// Reads Pairing fresh and stores an immutable review of one client change.
#[tauri::command]
pub(crate) async fn prepare_paired_client_change(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    client_id: String,
    change: String,
) -> Result<ClientChangeReview, PublicError> {
    prepare_change(&bridge, &plane_id, &client_id, &change).await
}

pub(crate) async fn prepare_change(
    bridge: &JetBridge,
    plane_id: &str,
    client_id: &str,
    change: &str,
) -> Result<ClientChangeReview, PublicError> {
    let target = parse_id(client_id)?;
    let change = Change::parse(change)?;
    let (binding, client) = bridge.plane(Some(plane_id))?;
    async {
        let connection = connect(&client).await?;
        let snapshot = connection
            .query(connection.pairing())
            .await
            .map_err(|error| client_error(&error))?;
        bridge.planes.observe_success(binding.plane, PAIRING_MINOR);
        let row = snapshot
            .clients
            .iter()
            .find(|row| row.client_id == target)
            .ok_or_else(|| {
                PublicError::invalid_input(
                    "owner.client_absent",
                    "That computer is no longer paired with this Plane.",
                )
            })?;
        if change.access() == Some(row.access) {
            return Err(PublicError::invalid_input(
                "owner.no_change",
                "That computer already has this access.",
            ));
        }
        let this_client = bridge.planes.local().client_id();
        let is_this_computer = target == this_client;
        let via_this_plane_connection =
            is_this_computer && matches!(binding.plane, PlaneId::Remote(_));
        let review_id = bridge.pairing.prepare(
            binding,
            target,
            change,
            via_this_plane_connection,
            Instant::now(),
        )?;
        Ok(ClientChangeReview {
            review_id: review_id.to_string(),
            plane_id: binding.plane.to_string(),
            plane_label: bridge
                .planes
                .label(binding.plane)
                .ok_or_else(PublicError::internal)?,
            plane_identity: binding.identity.map(|identity| identity.to_string()),
            client_id: target.to_string(),
            fingerprint: fingerprint(&row.key.key),
            change: change.name(),
            is_this_computer,
            via_this_plane_connection,
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

enum Changed {
    Access(PairedClient),
    Revoked(Uuid),
}

async fn send_change(
    connection: &Connection,
    id: Uuid,
    target: Uuid,
    change: Change,
) -> Result<Changed, Box<ClientError>> {
    match change.access() {
        Some(access) => connection
            .command(connection.set_paired_client_access(id, target, access))
            .await
            .map(Changed::Access),
        None => connection
            .command(connection.revoke_paired_client(id, target))
            .await
            .map(Changed::Revoked),
    }
}

/// The connection went away underneath a request that may have been sent.
fn transport_lost(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Io(_)
            | ClientError::Closed
            | ClientError::Disconnected(_)
            | ClientError::Frame(
                jet_protocol::FrameError::Io(_) | jet_protocol::FrameError::Closed
            )
    )
}

/// Executes a reviewed change on the Plane it was reviewed against, and
/// nowhere else. An attempted review answers with its cached receipt.
#[tauri::command]
pub(crate) async fn execute_paired_client_change(
    bridge: State<'_, JetBridge>,
    review_id: String,
) -> Result<ClientChangeReceipt, PublicError> {
    let id = parse_id(&review_id)?;
    execute_change(&bridge, id).await
}

pub(crate) async fn execute_change(
    bridge: &JetBridge,
    id: Uuid,
) -> Result<ClientChangeReceipt, PublicError> {
    let attempt = match bridge.pairing.attempt(id, Instant::now()) {
        Ok(attempt) => attempt,
        Err(error) => return Ok(ClientChangeReceipt::Refused { error }),
    };
    if let Some(known) = attempt.known {
        return Ok(known);
    }
    let binding = attempt.binding;
    let client = match bridge.bound(&binding) {
        Ok(client) => client,
        Err(error) => {
            let receipt = ClientChangeReceipt::Refused { error };
            bridge.pairing.record(id, receipt.clone())?;
            return Ok(receipt);
        }
    };
    // Nothing was sent if the Plane cannot be reached; the review stays
    // retryable with the same ID.
    let connection = connect(&client)
        .await
        .map_err(|error| bridge.settle(&binding, error))?;
    let mut outcome = send_change(&connection, id, attempt.client_id, attempt.change).await;
    let ends_own_access = attempt.via_this_plane_connection && attempt.change != Change::Enable;
    if ends_own_access
        && outcome
            .as_ref()
            .err()
            .is_some_and(|error| transport_lost(error))
    {
        // The Plane ends this computer's sessions right after the change
        // commits, which races the reply. Make exactly one fresh attempt.
        client.invalidate(&connection).await;
        match client.connect().await {
            Ok(again) => {
                // The same command ID returns the durable answer (ADR-0093).
                outcome = send_change(&again, id, attempt.client_id, attempt.change).await;
            }
            Err(error) => {
                let public = PublicError::from_client(&error);
                if public.code == "connection.unauthorized" {
                    let receipt = ClientChangeReceipt::AppliedUnverified {
                        change: attempt.change.name(),
                    };
                    bridge.pairing.record(id, receipt.clone())?;
                    bridge.planes.fail(binding.plane, lost_access()).await;
                    return Ok(receipt);
                }
                return Err(bridge.settle(&binding, public));
            }
        }
    }
    let this_client = bridge.planes.local().client_id();
    match outcome {
        Ok(changed) => {
            let receipt = match changed {
                Changed::Access(row)
                    if row.client_id == attempt.client_id
                        && Some(row.access) == attempt.change.access() =>
                {
                    ClientChangeReceipt::Applied {
                        client: Some(client_view(&row, this_client)),
                        revoked: false,
                    }
                }
                Changed::Revoked(revoked)
                    if revoked == attempt.client_id && attempt.change == Change::Revoke =>
                {
                    ClientChangeReceipt::Applied {
                        client: None,
                        revoked: true,
                    }
                }
                _ => return Err(bridge.settle(&binding, PublicError::internal())),
            };
            bridge.planes.observe_success(binding.plane, PAIRING_MINOR);
            bridge.pairing.record(id, receipt.clone())?;
            if ends_own_access {
                // This computer just lost access to the Plane it spoke over.
                bridge.planes.fail(binding.plane, lost_access()).await;
            }
            Ok(receipt)
        }
        Err(error) => {
            let public = bridge.settle(&binding, client_error(&error));
            if definite(&error) {
                let receipt = ClientChangeReceipt::Refused { error: public };
                bridge.pairing.record(id, receipt.clone())?;
                return Ok(receipt);
            }
            Err(public)
        }
    }
}

fn lost_access() -> PublicError {
    PublicError::unauthorized(
        "connection.unauthorized",
        "This computer is no longer allowed on this Plane.",
    )
}

#[cfg(test)]
mod tests {
    use std::{io, sync::Arc};

    use jet_protocol::{ClientPublicKey, ErrorCategory, PairingKeyAlgorithm, WireError};

    use super::*;
    use crate::jet::planes::{Security, Store};

    fn local() -> PlaneBinding {
        PlaneBinding {
            plane: PlaneId::Local,
            identity: Some(Uuid::from_u128(10)),
        }
    }

    fn remote() -> PlaneBinding {
        PlaneBinding {
            plane: PlaneId::Remote(Uuid::from_u128(2)),
            identity: Some(Uuid::from_u128(20)),
        }
    }

    fn refusal(category: ErrorCategory, code: &str) -> ClientError {
        ClientError::Remote(WireError {
            category,
            code: code.into(),
            retryable: false,
            message: "daemon text never crosses".into(),
            revision_conflict: None,
            restart: None,
            recovery_actions: Vec::new(),
        })
    }

    fn paired(id: u128, access: PairedClientAccess) -> PairedClient {
        PairedClient {
            client_id: Uuid::from_u128(id),
            key: ClientPublicKey {
                algorithm: PairingKeyAlgorithm::Ed25519,
                key: [id as u8; 32],
            },
            pairing_protocol: "jet.pairing.v1".into(),
            access,
            paired_at_unix_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn an_uncertain_change_resends_the_same_review_and_blocks_another() {
        let state = PairingState::default();
        let now = Instant::now();
        let target = Uuid::from_u128(5);
        let id = state
            .prepare(local(), target, Change::Disable, false, now)
            .unwrap();
        let first = state.attempt(id, now).unwrap();
        assert_eq!(first.binding, local());
        assert_eq!(first.change, Change::Disable);
        assert!(first.known.is_none());
        // A lost reply: the same review (and so the same command ID) is sent
        // again with the same body, and no other change may start meanwhile.
        let again = state.attempt(id, now).unwrap();
        assert_eq!(again.client_id, target);
        assert!(again.known.is_none());
        assert_eq!(
            state
                .prepare(local(), target, Change::Revoke, false, now)
                .err()
                .unwrap()
                .code,
            "owner.request_unresolved"
        );
        // Another client, or the same client on another Plane, is unaffected.
        assert!(state
            .prepare(local(), Uuid::from_u128(6), Change::Revoke, false, now)
            .is_ok());
        assert!(state
            .prepare(remote(), target, Change::Revoke, false, now)
            .is_ok());
    }

    #[test]
    fn a_definite_refusal_becomes_the_reviews_receipt() {
        let state = PairingState::default();
        let now = Instant::now();
        let target = Uuid::from_u128(5);
        let id = state
            .prepare(local(), target, Change::Revoke, false, now)
            .unwrap();
        state.attempt(id, now).unwrap();
        let error =
            PublicError::from_client(&refusal(ErrorCategory::Conflict, "security.audit_degraded"));
        state
            .record(id, ClientChangeReceipt::Refused { error })
            .unwrap();
        let replay = state.attempt(id, now).unwrap();
        assert!(matches!(
            replay.known,
            Some(ClientChangeReceipt::Refused { ref error }) if error.code == "security.audit_degraded"
        ));
        // Resolved, so a new change for the same client may be prepared.
        assert!(state
            .prepare(local(), target, Change::Revoke, false, now)
            .is_ok());
    }

    #[test]
    fn unattempted_reviews_expire_after_five_minutes() {
        let state = PairingState::default();
        let then = Instant::now();
        let id = state
            .prepare(local(), Uuid::from_u128(5), Change::Enable, false, then)
            .unwrap();
        let later = then + REVIEW_LIFETIME + Duration::from_secs(1);
        assert_eq!(
            state.attempt(id, later).err().unwrap().code,
            "owner.review_expired"
        );
        assert_eq!(
            state.attempt(id, later).err().unwrap().code,
            "owner.review_expired"
        );

        // An attempted review never expires: it may hold an uncertain request.
        let attempted = state
            .prepare(local(), Uuid::from_u128(6), Change::Disable, false, then)
            .unwrap();
        state.attempt(attempted, then).unwrap();
        assert!(state.attempt(attempted, later).is_ok());
        assert_eq!(
            state.attempt(Uuid::from_u128(99), then).err().unwrap().code,
            "owner.review_expired"
        );
    }

    #[test]
    fn capacity_evicts_the_oldest_unattempted_review_only() {
        let state = PairingState::default();
        let now = Instant::now();
        let mut attempted = Vec::new();
        for client in 0..MAX_REVIEWS as u128 - 1 {
            let id = state
                .prepare(
                    local(),
                    Uuid::from_u128(client),
                    Change::Disable,
                    false,
                    now,
                )
                .unwrap();
            state.attempt(id, now).unwrap();
            attempted.push(id);
        }
        let oldest_open = state
            .prepare(local(), Uuid::from_u128(500), Change::Disable, false, now)
            .unwrap();
        let newer_open = state
            .prepare(local(), Uuid::from_u128(501), Change::Disable, false, now)
            .unwrap();
        assert_eq!(
            state.attempt(oldest_open, now).err().unwrap().code,
            "owner.review_expired"
        );
        assert!(state.attempt(newer_open, now).is_ok());
        for id in &attempted {
            assert!(state.attempt(*id, now).is_ok());
        }
        // Every retained review is now attempted: nothing may be evicted.
        assert_eq!(
            state
                .prepare(local(), Uuid::from_u128(502), Change::Disable, false, now)
                .err()
                .unwrap()
                .code,
            "owner.request_limit"
        );
        assert_eq!(state.reviews.lock().unwrap().len(), MAX_REVIEWS);
    }

    #[test]
    fn command_ids_are_kept_only_while_the_outcome_is_uncertain() {
        let commands = Mutex::new(HashMap::new());
        let key = (PlaneId::Local, Gate::Open);
        let first = command_id(&commands, key).unwrap();
        let uncertain: Result<(), ClientError> = Err(ClientError::Io(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "lost",
        )));
        settle_command(&commands, &key, &uncertain).unwrap();
        assert_eq!(command_id(&commands, key).unwrap(), first);

        let unknown: Result<(), ClientError> = Err(refusal(
            ErrorCategory::OutcomeUnknown,
            "command.outcome_unknown",
        ));
        settle_command(&commands, &key, &unknown).unwrap();
        assert_eq!(command_id(&commands, key).unwrap(), first);

        let refused: Result<(), ClientError> =
            Err(refusal(ErrorCategory::Conflict, "security.audit_degraded"));
        settle_command(&commands, &key, &refused).unwrap();
        let second = command_id(&commands, key).unwrap();
        assert_ne!(second, first);

        settle_command(&commands, &key, &Ok::<(), ClientError>(())).unwrap();
        assert_ne!(command_id(&commands, key).unwrap(), second);

        // The same gate on another Plane is another body.
        let remote = command_id(&commands, (remote().plane, Gate::Open)).unwrap();
        assert_ne!(remote, command_id(&commands, key).unwrap());
    }

    #[test]
    fn mutations_follow_the_planes_security_and_store_health() {
        let view = |security, store| {
            serde_json::to_value(MutationsView::from_health(PlaneHealth::for_test(
                security, store,
            )))
            .unwrap()
        };
        assert_eq!(
            view(Security::Trusted, Store::Serving),
            serde_json::json!({"allowed": true, "reason": null})
        );
        // Unknown health never pre-refuses: jetd decides.
        assert_eq!(
            view(Security::Unknown, Store::Unknown),
            serde_json::json!({"allowed": true, "reason": null})
        );
        assert_eq!(
            view(Security::Degraded, Store::Serving),
            serde_json::json!({"allowed": false, "reason": "security_degraded"})
        );
        assert_eq!(
            view(Security::Trusted, Store::ReadOnly),
            serde_json::json!({"allowed": false, "reason": "store_read_only"})
        );
        assert_eq!(
            view(Security::Degraded, Store::ReadOnly)["reason"],
            "security_degraded"
        );
    }

    #[test]
    fn pairing_views_omit_the_authentication_string_and_mark_this_computer() {
        let this_client = Uuid::from_u128(7);
        let snapshot = PairingSnapshot {
            cursor: 18_446_744_073_709_551_615,
            gate: PairingGate::Open,
            pending: Some(PendingPairing {
                offer_id: Uuid::from_u128(3),
                method: PairingMethod::ManualCode,
                progress: PairingProgress::AwaitingConfirmation {
                    client_id: Uuid::from_u128(8),
                    authentication_string: "482-913".into(),
                },
                attempts_remaining: 4,
                opened_at_unix_ms: 1,
                expires_at_unix_ms: 120_001,
            }),
            clients: vec![
                paired(7, PairedClientAccess::Enabled),
                PairedClient {
                    pairing_protocol: "<b>JET</b>".into(),
                    ..paired(9, PairedClientAccess::Disabled)
                },
            ],
        };
        let view = pairing_view(
            PlaneId::Local,
            &snapshot,
            this_client,
            PlaneHealth::default(),
        );
        let json = serde_json::to_value(&view).unwrap();
        let text = json.to_string();
        assert!(!text.contains("482"), "the owner must type the string");
        assert_eq!(json["cursor"], "18446744073709551615");
        assert_eq!(json["gate"], "open");
        assert_eq!(json["thisClientId"], this_client.to_string());
        assert_eq!(
            json["pending"]["progress"],
            serde_json::json!({
                "kind": "awaiting_confirmation",
                "clientId": Uuid::from_u128(8).to_string(),
                "clientIsThisComputer": false,
            })
        );
        assert_eq!(json["pending"]["expiresAtUnixMs"], "120001");
        assert_eq!(json["clients"][0]["isThisComputer"], true);
        assert_eq!(json["clients"][0]["access"], "enabled");
        assert_eq!(json["clients"][0]["pairedAtUnixMs"], "1700000000000");
        assert_eq!(json["clients"][1]["pairingProtocol"], "unknown");
        assert_eq!(json["clients"][1]["access"], "disabled");
    }

    #[test]
    fn fingerprints_are_sixteen_grouped_hex_characters_of_the_key_digest() {
        let print = fingerprint(&[0; 32]);
        // SHA-256 of 32 zero bytes starts 66687aadf862bd77.
        assert_eq!(print, "6668 7aad f862 bd77");
        assert_ne!(fingerprint(&[1; 32]), print);
    }

    #[test]
    fn webview_inputs_are_validated_before_any_plane_is_reached() {
        assert_eq!(authentication_string("482913").unwrap(), "482-913");
        assert_eq!(authentication_string(" 482-913 ").unwrap(), "482-913");
        assert_eq!(authentication_string("482 913").unwrap(), "482-913");
        for invalid in [
            "",
            "48291",
            "4829134",
            "482_913",
            "48-2913",
            "４８２９１３",
            "abc-def",
        ] {
            assert_eq!(
                authentication_string(invalid).err().unwrap().code,
                "owner.authentication_string_invalid",
                "{invalid}"
            );
        }
        assert_eq!(Gate::parse("open").unwrap(), Gate::Open);
        assert_eq!(Gate::parse("closed").unwrap(), Gate::Closed);
        assert_eq!(
            Gate::parse("Open").err().unwrap().code,
            "owner.gate_invalid"
        );
        assert_eq!(Change::parse("revoke").unwrap(), Change::Revoke);
        assert_eq!(
            Change::parse("limit").err().unwrap().code,
            "owner.change_invalid"
        );
        assert_eq!(
            parse_id("client").err().unwrap().code,
            "owner.identifier_invalid"
        );
    }

    #[test]
    fn only_a_well_formed_manual_code_crosses_the_boundary() {
        assert_eq!(manual_code("1234-5678").as_deref(), Some("1234-5678"));
        for invalid in [
            "12345678",
            "1234-567",
            "1234 5678",
            "abcd-efgh",
            "1234-5678\n",
        ] {
            assert_eq!(manual_code(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn stable_refusals_are_definite_and_transport_loss_is_not() {
        assert!(definite(&refusal(
            ErrorCategory::InvalidInput,
            "pairing.authentication_string_mismatch"
        )));
        assert!(definite(&ClientError::FeatureUnavailable {
            required_minor: 6,
            negotiated_minor: 5,
        }));
        assert!(!definite(&refusal(
            ErrorCategory::OutcomeUnknown,
            "command.outcome_unknown"
        )));
        assert!(!definite(&ClientError::Closed));
    }

    #[tokio::test]
    async fn a_self_revoke_over_the_planes_own_connection_is_applied_unverified() {
        use jet_protocol::{
            ClientMessage, CommandRequest, QueryRequest, QueryResponse, ServerMessage,
        };

        use crate::jet::{
            enrollment::{
                self,
                tests::{setup, unauthorized_script},
            },
            keystore::Credential,
            planes::remote::tests::{
                next_message, open, reply, request_id, status, welcome, CLIENT, PLANE,
            },
        };

        let setup = setup();
        let key = setup
            .keys
            .ensure_public(CLIENT, Credential::Durable)
            .await
            .unwrap();
        let sent = Arc::new(Mutex::new(None));
        let record = sent.clone();
        setup.spawner.push(move |stream| async move {
            let (mut reader, mut writer) = welcome(open(stream).await, &key).await;
            let (stream, message) = next_message(&mut reader).await.unwrap();
            assert!(matches!(
                message,
                ClientMessage::Query {
                    query: QueryRequest::Status,
                    ..
                }
            ));
            let id = request_id(&message);
            reply(
                &mut writer,
                stream,
                ServerMessage::QueryResult {
                    id,
                    result: QueryResponse::Status(status(PLANE)),
                },
            )
            .await;
            // The revoke commits and the Plane ends this session before the
            // reply reaches the client.
            let (_, message) = next_message(&mut reader).await.unwrap();
            if let ClientMessage::Command {
                command_id,
                command: CommandRequest::RevokePairedClient { client_id },
                ..
            } = message
            {
                *record.lock().unwrap() = Some((command_id, client_id));
            }
            Some(0)
        });
        let added = enrollment::add(&setup.bridge, "build-box", None, Instant::now())
            .await
            .unwrap();
        let plane_id = serde_json::to_value(&added).unwrap()["plane"]["planeId"]
            .as_str()
            .unwrap()
            .to_owned();
        let (binding, _) = setup.bridge.plane(Some(&plane_id)).unwrap();
        let id = setup
            .bridge
            .pairing
            .prepare(binding, CLIENT, Change::Revoke, true, Instant::now())
            .unwrap();
        // The one reconnect is refused: this computer is no longer paired.
        unauthorized_script(&setup.spawner);
        let receipt = execute_change(&setup.bridge, id).await.unwrap();
        assert_eq!(
            serde_json::to_value(&receipt).unwrap(),
            serde_json::json!({"kind": "applied_unverified", "change": "revoke"})
        );
        assert_eq!(*sent.lock().unwrap(), Some((id, CLIENT)));
        assert_eq!(setup.spawner.spawns(), 2, "exactly one reconnect attempt");
        let view = serde_json::to_value(setup.bridge.planes.view(binding.plane).unwrap()).unwrap();
        assert_eq!(view["connection"]["state"], "failed");
        assert_eq!(
            view["connection"]["error"]["code"],
            "connection.unauthorized"
        );
        // Sticky: nothing is spawned until the user acts, and the receipt
        // is replayed without resending.
        let (_, client) = setup.bridge.plane(Some(&plane_id)).unwrap();
        assert!(client.connect().await.is_err());
        assert!(matches!(
            execute_change(&setup.bridge, id).await.unwrap(),
            ClientChangeReceipt::AppliedUnverified { change: "revoke" }
        ));
        assert_eq!(setup.spawner.spawns(), 2);
    }

    #[tokio::test]
    async fn a_review_for_a_forgotten_plane_is_refused_without_sending() {
        use crate::jet::enrollment::{self, tests::setup};

        let setup = setup();
        let plane_id = enrollment::tests::pair(
            &setup,
            "build-box",
            crate::jet::planes::remote::tests::PLANE,
        )
        .await;
        let (binding, _) = setup.bridge.plane(Some(&plane_id)).unwrap();
        let id = setup
            .bridge
            .pairing
            .prepare(
                binding,
                Uuid::from_u128(5),
                Change::Disable,
                false,
                Instant::now(),
            )
            .unwrap();
        enrollment::forget(&setup.bridge, &plane_id).await.unwrap();
        let spawns = setup.spawner.spawns();
        let receipt = execute_change(&setup.bridge, id).await.unwrap();
        assert!(matches!(
            receipt,
            ClientChangeReceipt::Refused { ref error } if error.code == "plane.review_moved"
        ));
        assert_eq!(setup.spawner.spawns(), spawns, "nothing is sent");
    }

    #[test]
    fn self_changes_record_the_lost_access_on_the_plane() {
        let error = lost_access().with_plane(remote().plane.to_string());
        assert_eq!(error.category, "unauthorized");
        assert_eq!(error.code, "connection.unauthorized");
        assert!(!error.retryable);
    }
}
