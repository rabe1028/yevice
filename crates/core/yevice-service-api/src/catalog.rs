//! `ServiceCatalog` — the central registry for all service plugins.

use yevice_core::{
    bindings::{derive_bindings, ConnectionRule},
    capacity::{CapacityModel, QuotaProvider, Quotas},
    cost::ArchitectureCost,
    resource::Architecture,
    types::ArchitectureName,
};
use yevice_pricing::catalog::PriceCatalog;

use crate::{
    error::CostError,
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
    pub fn register<S: Service + 'static>(&mut self, service: S) {
        self.services
            .insert(service.id().to_string(), Box::new(ServiceAdapter(service)));
    }

    /// Register a single connection rule.
    pub fn register_connection_rule(&mut self, rule: Box<dyn ConnectionRule>) {
        self.connection_rules.push(rule);
    }

    /// Register multiple connection rules at once.
    pub fn register_connection_rules(&mut self, rules: Vec<Box<dyn ConnectionRule>>) {
        self.connection_rules.extend(rules);
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
    pub fn build_cost_model(
        &self,
        arch: &Architecture,
        pricing: &dyn PriceCatalog,
        strict: bool,
    ) -> Result<ArchitectureCost, CostError> {
        let mut resource_costs = Vec::new();

        for resource in &arch.resources {
            let service_id = &resource.shell.service_id;
            if service_id == "other" {
                continue;
            }
            let Some(service) = self.services.get(service_id.as_str()) else {
                tracing::debug!(
                    service_id = service_id.as_str(),
                    "no cost model registered, skipping"
                );
                continue;
            };

            match service.build_cost(
                &resource.logical_id,
                &resource.resource_type,
                &resource.shell,
                pricing,
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
