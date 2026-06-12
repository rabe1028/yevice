//! Registry and pricing-resolver assembly from injected provider plugins.
//!
//! The engine never hardcodes a provider list: callers pass the plugins they
//! want (e.g. AWS + GCP + Cloudflare) and these helpers fan them out into the
//! service catalog, IaC adapter registries, and per-provider pricing resolver.

use std::collections::HashMap;

use yevice_core::resource::{Architecture, Provider};
use yevice_service_api::{
    CfnAdapterRegistry, MultiProviderCatalog, ProviderPlugin, Registration, ServiceCatalog,
    TfAdapterRegistry,
};

/// The registries that provider plugins populate during registration.
#[derive(Default)]
pub struct Registries {
    /// Service catalog (cost/capacity model builders and connection rules).
    pub catalog: ServiceCatalog,
    /// CloudFormation resource-type → adapter registry.
    pub cfn_adapters: CfnAdapterRegistry,
    /// Terraform resource-type → adapter registry.
    pub tf_adapters: TfAdapterRegistry,
}

/// Build all registries by letting each plugin register its services,
/// adapters, connection rules, and quota providers.
pub fn build_registries(plugins: &[Box<dyn ProviderPlugin>]) -> Registries {
    let mut registries = Registries::default();
    for plugin in plugins {
        let mut reg = Registration {
            catalog: &mut registries.catalog,
            cfn_adapters: &mut registries.cfn_adapters,
            tf_adapters: &mut registries.tf_adapters,
        };
        plugin.register(&mut reg);
    }
    registries
}

/// Build a per-provider pricing resolver from the providers present in `arch`.
///
/// Iterates over the injected provider plugins and, for each provider that
/// appears in the architecture, inserts the plugin's pricing catalog into the
/// resolver. The `Provider::Other` variant has no corresponding plugin and is
/// handled separately with a [`yevice_pricing::NoopCatalog`].
///
/// `provider_regions` allows overriding the region used for a specific
/// provider's pricing catalog. Providers not present in the map fall back to
/// `default_region`.
pub fn build_pricing_resolver(
    plugins: &[Box<dyn ProviderPlugin>],
    arch: &Architecture,
    default_region: &str,
    provider_regions: &HashMap<Provider, String>,
    list_price: bool,
) -> MultiProviderCatalog {
    let mut resolver = MultiProviderCatalog::new();

    for plugin in plugins {
        if arch.has_provider(plugin.provider()) {
            let region = provider_regions
                .get(&plugin.provider())
                .map_or(default_region, String::as_str);
            resolver.insert(
                plugin.provider(),
                plugin.pricing_catalog(region, list_price),
            );
        }
    }

    // Provider::Other has no dedicated plugin; use a no-op catalog.
    if arch.has_provider(Provider::Other) {
        resolver.insert(Provider::Other, Box::new(yevice_pricing::NoopCatalog));
    }

    resolver
}

#[cfg(test)]
mod tests {
    use super::*;
    use yevice_core::resource::{Resource, ResourceShell};
    use yevice_core::types::{LogicalId, Region, ResourceType};
    use yevice_pricing::NoopCatalog;
    use yevice_pricing::catalog::PriceCatalog;
    use yevice_service_api::PriceCatalogResolver;

    /// A provider plugin stub that records the region it was asked to price.
    struct StubPlugin {
        provider: Provider,
    }

    impl ProviderPlugin for StubPlugin {
        fn provider(&self) -> Provider {
            self.provider
        }

        fn register(&self, _reg: &mut Registration<'_>) {}

        fn pricing_catalog(&self, _region: &str, _list_price: bool) -> Box<dyn PriceCatalog> {
            Box::new(NoopCatalog)
        }
    }

    fn arch_with(provider: Provider) -> Architecture {
        let shell = ResourceShell::new("stub.service", provider, &serde_json::json!({}));
        Architecture {
            name: "test-arch".to_string(),
            region: Region::new("ap-northeast-1"),
            resources: vec![Resource {
                logical_id: LogicalId::new("MyService"),
                resource_type: ResourceType::new("stub_resource"),
                shell,
                group: None,
            }],
            connections: Vec::new(),
        }
    }

    #[test]
    fn pricing_resolver_only_includes_providers_present_in_architecture() {
        let plugins: Vec<Box<dyn ProviderPlugin>> = vec![
            Box::new(StubPlugin {
                provider: Provider::Aws,
            }),
            Box::new(StubPlugin {
                provider: Provider::Gcp,
            }),
        ];
        let arch = arch_with(Provider::Gcp);

        let mut provider_regions: HashMap<Provider, String> = HashMap::new();
        provider_regions.insert(Provider::Gcp, "asia-northeast1".to_string());

        let resolver =
            build_pricing_resolver(&plugins, &arch, "ap-northeast-1", &provider_regions, false);
        assert!(
            resolver.resolve(Provider::Gcp).is_some(),
            "provider present in the architecture must get a catalog"
        );
        assert!(
            resolver.resolve(Provider::Aws).is_none(),
            "provider absent from the architecture must not get a catalog"
        );
    }

    #[test]
    fn pricing_resolver_assigns_noop_catalog_to_other_provider() {
        let plugins: Vec<Box<dyn ProviderPlugin>> = vec![];
        let arch = arch_with(Provider::Other);

        let resolver =
            build_pricing_resolver(&plugins, &arch, "ap-northeast-1", &HashMap::new(), false);
        assert!(
            resolver.resolve(Provider::Other).is_some(),
            "Provider::Other must fall back to a no-op catalog"
        );
    }
}
