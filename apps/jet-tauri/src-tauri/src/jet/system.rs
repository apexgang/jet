//! System health and safe repairs (Wave 3.3 §4.4). This slice provides the
//! one repair the main window's Plane-health notice offers: "Free disposable
//! space", a single bounded Artifact collection pass on one Plane.
use std::{collections::HashSet, sync::Mutex};

use jet_protocol::ARTIFACTS_MINOR;
use serde::Serialize;
use tauri::State;

use super::{errors::PublicError, planes::PlaneId, JetBridge};

#[derive(Default)]
pub(crate) struct SystemState {
    collect_in_flight: Mutex<HashSet<PlaneId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CollectView {
    removed: u32,
}

/// Holds one Plane's slot in an in-flight set and releases it on every exit
/// path, including a dropped IPC future.
struct InFlight<'a> {
    set: &'a Mutex<HashSet<PlaneId>>,
    plane: PlaneId,
}

impl<'a> InFlight<'a> {
    fn enter(
        set: &'a Mutex<HashSet<PlaneId>>,
        plane: PlaneId,
        busy: PublicError,
    ) -> Result<Self, PublicError> {
        let mut planes = set.lock().map_err(|_| PublicError::internal())?;
        if !planes.insert(plane) {
            return Err(busy);
        }
        Ok(Self { set, plane })
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Ok(mut planes) = self.set.lock() {
            planes.remove(&self.plane);
        }
    }
}

/// Runs one bounded collection of disposable Artifact storage on a Plane.
/// It is available under disk pressure (`docs/disk-pressure.md`) and
/// carries no Command ID, so a retry is simply another pass.
#[tauri::command]
pub(crate) async fn collect_disposable_storage(
    bridge: State<'_, JetBridge>,
    plane_id: String,
) -> Result<CollectView, PublicError> {
    collect(&bridge, &plane_id).await
}

async fn collect(bridge: &JetBridge, plane_id: &str) -> Result<CollectView, PublicError> {
    let (binding, client) = bridge.plane(Some(plane_id))?;
    let _flight = InFlight::enter(
        &bridge.system.collect_in_flight,
        binding.plane,
        PublicError::conflict(
            "storage.collect_busy",
            "Jet is already freeing disposable space on this Plane.",
        ),
    )
    .map_err(|error| error.with_plane(binding.plane.to_string()))?;
    async {
        let removed = client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?
            .collect_artifacts()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        bridge
            .planes
            .observe_success(binding.plane, ARTIFACTS_MINOR);
        Ok(CollectView { removed })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use uuid::Uuid;

    use super::collect;
    use crate::jet::{
        keystore::{
            tests::{CountingStore, Fail},
            IdentityKeys,
        },
        planes::{spawner::fake::FakeSpawner, PlaneId},
        JetBridge,
    };

    fn bridge(directory: &std::path::Path) -> JetBridge {
        JetBridge::for_test(
            directory,
            Uuid::from_u128(1),
            Arc::new(FakeSpawner::default()),
            Arc::new(IdentityKeys::new(Arc::new(CountingStore::new(
                Fail::Nothing,
            )))),
        )
    }

    #[tokio::test]
    async fn collection_is_single_flight_per_plane_and_releases_its_slot() {
        let directory = tempfile::tempdir().unwrap();
        let bridge = bridge(directory.path());

        bridge
            .system
            .collect_in_flight
            .lock()
            .unwrap()
            .insert(PlaneId::Local);
        let busy = collect(&bridge, "local").await.unwrap_err();
        assert_eq!(busy.code, "storage.collect_busy");
        assert_eq!(busy.category, "conflict");
        assert_eq!(busy.plane_id.as_deref(), Some("local"));
        bridge.system.collect_in_flight.lock().unwrap().clear();

        // The local socket does not exist: the pass fails offline, names its
        // Plane, and leaves no slot behind.
        let offline = collect(&bridge, "local").await.unwrap_err();
        assert_eq!(offline.category, "offline");
        assert_eq!(offline.plane_id.as_deref(), Some("local"));
        assert!(bridge.system.collect_in_flight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn collection_refuses_an_unknown_plane_before_any_io() {
        let directory = tempfile::tempdir().unwrap();
        let bridge = bridge(directory.path());
        let unknown = collect(&bridge, "../jetd.sock").await.unwrap_err();
        assert_eq!(unknown.category, "invalid_input");
        assert!(bridge.system.collect_in_flight.lock().unwrap().is_empty());
    }
}
