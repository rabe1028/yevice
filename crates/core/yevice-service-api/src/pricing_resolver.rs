//! Per-provider pricing catalog resolver.
//!
//! [`PriceCatalogResolver`] decouples the cost-model loop from any single
//! pricing catalog so that multi-provider architectures (e.g. AWS + GCP
//! resources in the same input) can be priced correctly.

use std::collections::HashMap;

use yevice_core::resource::Provider;
use yevice_pricing::catalog::PriceCatalog;

/// Selects the appropriate [`PriceCatalog`] for a given cloud provider.
///
/// Used by [`crate::ServiceCatalog::build_cost_model`] to dispatch per-resource,
/// enabling mixed-provider architectures.
pub trait PriceCatalogResolver: Send + Sync {
    /// Return the catalog to use for `provider`, or `None` if no catalog is
    /// registered for it.
    fn resolve(&self, provider: Provider) -> Option<&dyn PriceCatalog>;
}

/// A [`PriceCatalogResolver`] backed by a per-provider map of catalogs.
///
/// Build one with [`MultiProviderCatalog::with`] chaining or
/// [`MultiProviderCatalog::insert`], then pass a reference to
/// [`crate::ServiceCatalog::build_cost_model`].
#[derive(Default)]
pub struct MultiProviderCatalog {
    catalogs: HashMap<Provider, Box<dyn PriceCatalog>>,
}

impl MultiProviderCatalog {
    /// Create an empty resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a catalog for `provider` and return `self` for chaining.
    #[must_use]
    pub fn with(mut self, provider: Provider, catalog: Box<dyn PriceCatalog>) -> Self {
        self.catalogs.insert(provider, catalog);
        self
    }

    /// Register a catalog for `provider` in place.
    pub fn insert(&mut self, provider: Provider, catalog: Box<dyn PriceCatalog>) {
        self.catalogs.insert(provider, catalog);
    }

    /// Convenience constructor: a resolver with a single provider registered.
    pub fn single(provider: Provider, catalog: Box<dyn PriceCatalog>) -> Self {
        Self::new().with(provider, catalog)
    }
}

impl PriceCatalogResolver for MultiProviderCatalog {
    fn resolve(&self, provider: Provider) -> Option<&dyn PriceCatalog> {
        self.catalogs
            .get(&provider)
            .map(std::convert::AsRef::as_ref)
    }
}
