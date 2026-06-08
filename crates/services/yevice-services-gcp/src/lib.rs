//! GCP service plugin implementations for yevice.

pub mod pricing_adapter;
pub mod services;
pub mod tf;

pub use pricing_adapter::GcpPricingCatalog;

/// Register all GCP services and TF adapters.
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
