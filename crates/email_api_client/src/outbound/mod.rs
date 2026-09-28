//! Provider-specific outbound email API adapters.

/// Gmail outbound adapter implementation.
#[cfg(feature = "outbound-gmail")]
pub mod gmail;

/// Microsoft Graph read-only mail adapter implementation.
#[cfg(feature = "outbound-microsoft-graph")]
pub mod microsoft_graph;
