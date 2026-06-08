use crate::services::elasticache::ElastiCacheSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct ElastiCacheCfnAdapter;
impl CfnAdapter for ElastiCacheCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &[
            "AWS::ElastiCache::CacheCluster",
            "AWS::ElastiCache::ReplicationGroup",
        ]
    }
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let spec = ElastiCacheSpec {
            node_type: raw
                .get_str("CacheNodeType")
                .unwrap_or("cache.t3.micro")
                .to_string(),
            num_nodes: raw.get_f64("NumCacheNodes").unwrap_or(1.0),
        };
        Ok(ResourceShell::new("aws.elasticache", Provider::Aws, &spec))
    }
}
