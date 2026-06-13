//! GCP service plugin implementations for yevice.
//!
//! Follows the standard provider pattern (see
//! `docs/adr/0004-provider-implementation-pattern.md`):
//! - **mandatory**: `ProviderPlugin` (via [`GcpPlugin`]) + `PriceCatalog`
//!   (via [`GcpPricingCatalog`])
//! - **not implemented yet**: file-backed pricing registry (GCP has no Bulk
//!   Pricing API equivalent wired up), CFN adapters (GCP uses Terraform /
//!   Deployment Manager, not CloudFormation)

pub mod plugin;
pub mod pricing_adapter;
pub mod services;
pub mod tf;

pub use plugin::GcpPlugin;
pub use pricing_adapter::GcpPricingCatalog;

/// Register all GCP services and TF adapters.
///
/// Mirrors `yevice_services_aws::register` but takes no CFN adapter registry
/// — GCP has no CloudFormation surface.
pub fn register(
    catalog: &mut yevice_service_api::ServiceCatalog,
    tf: &mut yevice_service_api::TfAdapterRegistry,
) {
    catalog.register(services::cloud_function::GcpCloudFunctionService);
    catalog.register(services::cloud_run::GcpCloudRunService);
    catalog.register(services::bigquery::GcpBigQueryService);
    catalog.register(services::cloud_storage::GcpCloudStorageService);
    catalog.register(services::pubsub::GcpPubSubService);
    catalog.register(services::cloud_sql::GcpCloudSqlService);

    tf.register(tf::cloud_function::GcpCloudFunctionTfAdapter);
    tf.register(tf::cloud_run::GcpCloudRunTfAdapter);
    tf.register(tf::bigquery::GcpBigQueryTfAdapter);
    tf.register(tf::cloud_storage::GcpCloudStorageTfAdapter);
    tf.register(tf::pubsub::GcpPubSubTfAdapter);
    tf.register(tf::cloud_sql::GcpCloudSqlTfAdapter);
}
