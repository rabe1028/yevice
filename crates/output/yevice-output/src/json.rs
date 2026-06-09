//! JSON renderer — serializes `cost.topology` to a pretty-printed JSON string.

use yevice_core::cost::ArchitectureCost;

use crate::ArchitectureRenderer;
use crate::error::RenderError;

/// Renders the topology from an [`ArchitectureCost`] as pretty-printed JSON.
///
/// The output is the verbatim serde serialization of [`yevice_core::topology::Topology`],
/// which can be round-tripped back via `serde_json::from_str::<Topology>`.
pub struct JsonRenderer;

impl ArchitectureRenderer for JsonRenderer {
    fn format_name(&self) -> &'static str {
        "json"
    }

    fn render(&self, cost: &ArchitectureCost) -> Result<String, RenderError> {
        let json = serde_json::to_string_pretty(&cost.topology)?;
        Ok(json)
    }
}
