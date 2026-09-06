//! Discovery of Harness-native Conversation identities outside Jet
//! (ADR-0010).
//!
//! Jet reads the identities a supported Harness makes available and never
//! guesses at the processes it cannot see into. A discovery is observed,
//! like a Capability observation, and stored nowhere: what it reports is
//! the machine as it is now, and an import records only what the user
//! chose to register from it.

use std::future::Future;
use std::pin::Pin;

use crate::import::DiscoveredConversation;

/// Observes the Harness-native Conversation identities present on this
/// Plane outside Jet's management.
///
/// `jetd` asks whenever a client lists external Conversations and again
/// before an import commits, so an identity is registered only while the
/// Plane can still see it. An implementation answers from the machine it
/// runs on, must not poll on its own, and must not change Plane state. It
/// reports a live process as cooperating only when the Harness advertises
/// a structured endpoint Jet can drive; a process it can see only through
/// a terminal stays external.
///
/// The returned future is boxed because the core chooses its discovery at
/// run time: a Plane observes its own machine, while a test answers with a
/// fixed observation.
pub trait ConversationDiscovery: std::fmt::Debug + Send + Sync {
	/// Observes the Plane once.
	fn discover(
		&self,
	) -> Pin<Box<dyn Future<Output = Vec<DiscoveredConversation>> + Send + '_>>;
}

/// The discovery this Plane runs with until its Crafts report identities.
///
/// Reading a Harness's native identities is the work of the Craft that
/// adapts it (ADR-0007), and no bundled Craft does so yet; until the Craft
/// issues land, the Plane honestly reports none rather than reading
/// another program's files on its own.
#[derive(Debug)]
pub(crate) struct SystemConversationDiscovery;

impl ConversationDiscovery for SystemConversationDiscovery {
	fn discover(
		&self,
	) -> Pin<Box<dyn Future<Output = Vec<DiscoveredConversation>> + Send + '_>>
	{
		Box::pin(async { Vec::new() })
	}
}
