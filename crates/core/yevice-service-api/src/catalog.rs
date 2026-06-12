//! `ServiceCatalog` — the central registry for all service plugins.

use yevice_core::{
    bindings::{ConnectionRule, derive_bindings},
    capacity::{CapacityModel, QuotaProvider, Quotas},
    cost::ArchitectureCost,
    resource::Architecture,
    types::ArchitectureName,
};

use crate::{
    error::CostError,
    pricing_resolver::PriceCatalogResolver,
    service::{AnyService, Service, ServiceAdapter},
};

/// Registry of all service implementations. Built once at startup in the CLI.
///
/// Use [`ServiceCatalog::register`] to add implementations, then call
/// [`ServiceCatalog::build_cost_model`] or [`ServiceCatalog::build_capacity_models`].
#[derive(Default)]
pub struct ServiceCatalog {
    services: std::collections::HashMap<String, Box<dyn AnyService>>,
    connection_rules: Vec<Box<dyn ConnectionRule>>,
    quota_providers: Vec<Box<dyn QuotaProvider>>,
}

impl ServiceCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a typed service implementation.
    ///
    /// # Panics
    ///
    /// Panics if a service with the same `service_id` has already been registered.
    pub fn register<S: Service + 'static>(&mut self, service: S) {
        let id = service.id().to_string();
        assert!(
            !self.services.contains_key(&id),
            "duplicate service registration for service_id '{id}'"
        );
        self.services.insert(id, Box::new(ServiceAdapter(service)));
    }

    /// Register a single connection rule.
    pub fn register_connection_rule(&mut self, rule: Box<dyn ConnectionRule>) {
        self.connection_rules.push(rule);
    }

    /// Register multiple connection rules at once.
    pub fn register_connection_rules(&mut self, rules: Vec<Box<dyn ConnectionRule>>) {
        self.connection_rules.extend(rules);
    }

    /// Returns a sorted list of all registered service IDs.
    pub fn registered_service_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.services.keys().map(String::as_str).collect();
        ids.sort_unstable();
        ids
    }

    /// Return a slice of all registered connection rules.
    pub fn connection_rules(&self) -> &[Box<dyn ConnectionRule>] {
        &self.connection_rules
    }

    /// Register a quota provider.
    pub fn register_quota_provider(&mut self, p: Box<dyn QuotaProvider>) {
        self.quota_providers.push(p);
    }

    /// Merge default quotas from all registered providers for the given region.
    /// Later-registered providers win on key conflicts.
    pub fn default_quotas(&self, region: &str) -> Quotas {
        let mut merged = Quotas::default();
        for provider in &self.quota_providers {
            merged.merge_from(provider.default_quotas(region));
        }
        merged
    }

    /// Build a cost model for the given architecture.
    ///
    /// Resources whose service_id has no registered service are silently
    /// skipped (or cause an error if `strict` is `true`).
    ///
    /// The `pricing` resolver is called per-resource with the resource's
    /// provider. If no catalog is registered for that provider the resource is
    /// skipped (or an error is returned when `strict` is `true`).
    pub fn build_cost_model(
        &self,
        arch: &Architecture,
        pricing: &dyn PriceCatalogResolver,
        strict: bool,
    ) -> Result<ArchitectureCost, CostError> {
        let mut resource_costs = Vec::new();

        for resource in &arch.resources {
            let service_id = &resource.shell.service_id;
            if service_id == "other" {
                continue;
            }
            let Some(service) = self.services.get(service_id.as_str()) else {
                tracing::warn!(
                    service_id = service_id.as_str(),
                    resource_type = resource.resource_type.as_str(),
                    logical_id = %resource.logical_id,
                    "no service registered for service_id; resource silently skipped"
                );
                continue;
            };

            let Some(catalog) = pricing.resolve(resource.shell.provider) else {
                if strict {
                    return Err(CostError::NoPricingCatalog(resource.shell.provider));
                }
                tracing::warn!(
                    resource = %resource.logical_id,
                    provider = ?resource.shell.provider,
                    "no pricing catalog for provider; skipping"
                );
                continue;
            };

            match service.build_cost(
                &resource.logical_id,
                &resource.resource_type,
                &resource.shell,
                catalog,
            ) {
                Ok(cost) => resource_costs.push(cost),
                Err(e) => {
                    if strict {
                        return Err(e);
                    }
                    tracing::warn!(
                        resource = %resource.logical_id,
                        error = %e,
                        "failed to compute cost, skipping"
                    );
                }
            }
        }

        let bindings = derive_bindings(arch, &self.connection_rules);

        Ok(ArchitectureCost {
            name: ArchitectureName::new(&arch.name),
            resources: resource_costs,
            bindings,
            region: arch.region.clone(),
            topology: arch.topology(),
        })
    }

    /// Build capacity models for all resources in the architecture.
    pub fn build_capacity_models(
        &self,
        arch: &Architecture,
        quotas: &Quotas,
    ) -> Vec<CapacityModel> {
        let mut models = Vec::new();
        for resource in &arch.resources {
            let service_id = &resource.shell.service_id;
            if let Some(service) = self.services.get(service_id.as_str())
                && let Some(model) =
                    service.build_capacity(&resource.logical_id, &resource.shell, quotas)
            {
                models.push(model);
            }
        }
        models
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use yevice_core::{cost::ResourceCost, resource::Provider, types::LogicalId};
    use yevice_pricing::catalog::PriceCatalog;

    #[derive(Clone, Serialize, Deserialize)]
    struct DummySpec;

    struct DummyService;

    impl Service for DummyService {
        type Spec = DummySpec;

        fn id(&self) -> &'static str {
            "test.dummy"
        }

        fn provider(&self) -> Provider {
            Provider::Other
        }

        fn build_cost(
            &self,
            _id: &LogicalId,
            _resource_type: &yevice_core::types::ResourceType,
            _spec: &Self::Spec,
            _pricing: &dyn PriceCatalog,
        ) -> Result<ResourceCost, crate::CostError> {
            unimplemented!()
        }
    }

    #[test]
    #[should_panic(expected = "duplicate")]
    fn duplicate_service_registration_panics() {
        let mut catalog = ServiceCatalog::new();
        catalog.register(DummyService);
        catalog.register(DummyService);
    }
}
