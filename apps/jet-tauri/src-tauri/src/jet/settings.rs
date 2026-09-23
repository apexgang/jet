//! Plane-addressed settings. Every settings command resolves its Plane here,
//! before any I/O, so later slices bind snapshots and reviews to one Plane.

use super::{client::PlaneClient, errors::PublicError, planes, planes::PlaneBinding, JetBridge};

/// A canonical Plane handle is `local` or a 36-byte UUID.
const MAX_PLANE_ID_BYTES: usize = 36;

/// The Plane a settings target, snapshot or review belongs to, and the Plane
/// identity known when it was resolved. Follow-up commands act only through
/// the Plane that issued them.
pub(crate) type PlaneKey = PlaneBinding;

/// Resolves a webview Plane handle to a registered Plane. An oversized,
/// non-canonical or unregistered handle is `plane.unknown`; nothing connects.
pub(crate) fn plane_client(
    bridge: &JetBridge,
    plane_id: &str,
) -> Result<(PlaneKey, PlaneClient), PublicError> {
    // ASVS 2.2.1: bounded before the registry sees it.
    if plane_id.len() > MAX_PLANE_ID_BYTES {
        return Err(planes::unknown_plane());
    }
    bridge.plane(Some(plane_id))
}

#[cfg(test)]
mod tests {
    use super::plane_client;
    use crate::jet::{enrollment::tests::setup, planes::PlaneId};
    use uuid::Uuid;

    #[test]
    fn only_registered_canonical_planes_resolve() {
        let setup = setup();
        let (key, _) = plane_client(&setup.bridge, "local").unwrap();
        assert_eq!(key.plane, PlaneId::Local);

        let unregistered = Uuid::from_u128(7).to_string();
        let oversized = format!("{unregistered}0");
        for invalid in [
            "",
            "LOCAL",
            "../local",
            unregistered.as_str(),
            &unregistered.to_uppercase(),
            oversized.as_str(),
        ] {
            let error = plane_client(&setup.bridge, invalid).err().unwrap();
            assert_eq!(error.code, "plane.unknown", "{invalid}");
        }
    }
}
