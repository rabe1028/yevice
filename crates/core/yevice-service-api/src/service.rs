use serde::{Serialize, de::DeserializeOwned};
use yevice_core::{
    capacity::{CapacityModel, RegionQuotas},
    cost::ResourceCost,
    resource::{Provider, ResourceShell},
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::PriceCatalog;

use crate::error::CostError;

/// A service plugin. One impl per cloud service type.
///
/// `Spec` is the strongly-typed configuration for this service (e.g. `LambdaSpec`).
/// It must be serializable so it can be stored in `ResourceShell` and recovered.
pub trait Service: Send + Sync {
    type Spec: Clone + Serialize + DeserializeOwned + Send + Sync + 'static;

    /// Unique service identifier, e.g. `"aws.lambda"`, `"gcp.cloud_run"`.
    fn id(&self) -> &'static str;

    fn provider(&self) -> Provider;

    fn build_cost(
        &self,
        id: &LogicalId,
        resource_type: &ResourceType,
        spec: &Self::Spec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError>;

    fn build_capacity(
        &self,
        _id: &LogicalId,
        _spec: &Self::Spec,
        _quotas: &RegionQuotas,
    ) -> Option<CapacityModel> {
        None
    }
}

/// Type-erased version of `Service` used inside `ServiceCatalog`.
pub trait AnyService: Send + Sync {
    fn id(&self) -> &str;
    fn provider(&self) -> Provider;

    fn build_cost(
        &self,
        id: &LogicalId,
        resource_type: &ResourceType,
        shell: &ResourceShell,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError>;

    fn build_capacity(
        &self,
        id: &LogicalId,
        shell: &ResourceShell,
        quotas: &RegionQuotas,
    ) -> Option<CapacityModel>;
}

/// Wraps a typed `Service` impl and adapts it to `AnyService`.
pub struct ServiceAdapter<S>(pub S);

impl<S: Service + 'static> AnyService for ServiceAdapter<S> {
    fn id(&self) -> &str {
        self.0.id()
    }

    fn provider(&self) -> Provider {
        self.0.provider()
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        resource_type: &ResourceType,
        shell: &ResourceShell,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let spec: S::Spec = shell.decode().map_err(|e| CostError::SpecDeserialize {
            service_id: self.0.id().to_string(),
            cause: e.to_string(),
        })?;
        self.0.build_cost(id, resource_type, &spec, pricing)
    }

    fn build_capacity(
        &self,
        id: &LogicalId,
        shell: &ResourceShell,
        quotas: &RegionQuotas,
    ) -> Option<CapacityModel> {
        let spec: S::Spec = shell.decode().ok()?;
        self.0.build_capacity(id, &spec, quotas)
    }
}
