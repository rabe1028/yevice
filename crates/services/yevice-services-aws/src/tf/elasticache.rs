use crate::services::elasticache::ElastiCacheSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct ElastiCacheTfAdapter;
impl TfAdapter for ElastiCacheTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &[
            "aws_elasticache_cluster",
            "aws_elasticache_replication_group",
        ]
    }
    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let node_type = raw
            .get_str("node_type")
            .unwrap_or("cache.t3.micro")
            .to_string();
        let num_nodes = if raw.resource_type.as_str() == "aws_elasticache_replication_group" {
            raw.get_f64("num_cache_clusters")
                .or_else(|| raw.get_f64("number_cache_clusters"))
                .unwrap_or(1.0)
        } else {
            raw.get_f64("num_cache_nodes").unwrap_or(1.0)
        };
        Ok(ResourceShell::new(
            "aws.elasticache",
            Provider::Aws,
            &ElastiCacheSpec {
                node_type,
                num_nodes,
            },
        ))
    }
}
