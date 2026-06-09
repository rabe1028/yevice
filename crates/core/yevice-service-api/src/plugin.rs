//! Provider plugin trait for uniform service and pricing registration.
//!
//! Each cloud-provider crate implements [`ProviderPlugin`] once. The CLI wires
//! up all providers by iterating over a `Vec<Box<dyn ProviderPlugin>>` rather
//! than calling provider-specific free functions, so adding a new provider
//! requires only adding a plugin to the list.

use yevice_core::resource::Provider;
use yevice_pricing::catalog::PriceCatalog;

use crate::{CfnAdapterRegistry, ServiceCatalog, TfAdapterRegistry};

/// Bundles the registries a provider plugin writes into.
pub struct Registration<'a> {
    /// The service catalog to register services and connection rules into.
    pub catalog: &'a mut ServiceCatalog,
    /// The CloudFormation adapter registry.
    pub cfn_adapters: &'a mut CfnAdapterRegistry,
    /// The Terraform adapter registry.
    pub tf_adapters: &'a mut TfAdapterRegistry,
}

/// A cloud provider plugin: registers its services/adapters/rules/quotas and
/// builds its pricing catalog.
///
/// Implemented once per provider crate so that adding a provider does not
/// require touching CLI wiring.
pub trait ProviderPlugin: Send + Sync {
    /// The cloud provider this plugin is responsible for.
    fn provider(&self) -> Provider;

    /// Register all services, adapters, connection rules, and quota providers
    /// into the supplied registries.
    fn register(&self, reg: &mut Registration<'_>);

    /// Build this provider's pricing catalog for the given region.
    ///
    /// `list_price` controls whether promotional free-tier allowances should be
    /// zeroed out (AWS-specific; ignored by providers that do not have a
    /// concept of a free tier in catalog lookups).
    fn pricing_catalog(&self, region: &str, list_price: bool) -> Box<dyn PriceCatalog>;
}
