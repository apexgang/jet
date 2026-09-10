//! Craft disable controls, introduced in Jet protocol minor 27.
/// How an interactive disable handles already accepted Runs.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CraftDisableMode {
	/// Block new Runs and let pinned Runs finish.
	Wait,
	/// Stop the Craft while leaving its Harnesses under their helpers.
	Force,
}
