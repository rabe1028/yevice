# ADR-0004: Provider Implementation Pattern

Status: Accepted (2026-06-13). Refs #22.

## Context

`yevice-pricing` previously exposed an AWS-typed `PricingProvider` trait
(returning `Ec2Price`, `RdsPrice`, ...) from the shared crate. New providers
(GCP, future Azure) integrate via the provider-neutral `PriceCatalog` trait,
so the AWS trait at the common-crate level was misleading.

## Decision

Each provider crate (`yevice-services-<provider>`) is the **sole owner** of
its provider-specific types and traits. The common `yevice-pricing` crate
holds only neutral primitives.

### Mandatory components

Every provider crate MUST expose:

1. **`ProviderPlugin` impl** (`yevice_service_api::ProviderPlugin`) — wires
   services / CFN / TF adapters into the registries and hands out the
   pricing catalog. Example: `AwsPlugin`, `GcpPlugin`.
2. **`PriceCatalog` impl** (`yevice_pricing::catalog::PriceCatalog`) — the
   single provider-neutral pricing boundary. All cross-provider code (engine,
   CLI, IaC frontends) talks to providers only through this trait. Example:
   `AwsPricingCatalog`, `GcpPricingCatalog`.

### Optional components

3. **File-backed pricing registry** — for providers with a Bulk-Pricing API
   (AWS). Lives inside the provider crate. AWS currently has it
   (`FilePricingRegistry`, AWS-internal `PricingProvider` trait); GCP does
   not. Absence does not affect the public interface.
4. **`PricingMetadata` exposure** — when the file registry is present, the
   plugin may surface freshness/version metadata.
5. **Provider-internal traits** — e.g. AWS's `PricingProvider` (returns AWS
   `Ec2Price` etc.) is a private helper that lets two registry types share
   one `PriceCatalog` adapter. Such traits MUST stay inside the provider
   crate; do not put them in `yevice-pricing`.

### Crate layout (current examples)

- AWS: `crates/services/yevice-services-aws/src/{plugin.rs, pricing_adapter.rs, pricing_provider.rs}`
- GCP: `crates/services/yevice-services-gcp/src/{plugin.rs, pricing_adapter.rs}` (no file registry)

## Adding a new provider (e.g. Azure)

1. Create `crates/services/yevice-services-azure` mirroring the GCP layout.
2. Add Azure SKUs to `yevice-pricing/src/catalog.rs` if needed (Sku is a free-form string, so usually no change is required).
3. Implement `AzurePricingCatalog: PriceCatalog` — start with hardcoded prices, mirroring `GcpPricingCatalog`.
4. Implement `AzurePlugin: ProviderPlugin` and return `AzurePricingCatalog` from `pricing_catalog`.
5. Optional: add a file-backed registry + provider-internal trait if Azure exposes a Bulk-Pricing equivalent.
6. Register the new plugin in the CLI / engine entry point.

## Consequences

- `yevice-pricing` stops pretending to be cross-provider while exposing AWS types: provider-neutral access is `PriceCatalog`-only.
- AWS-shaped internals (`bulk_api`, `download`, `file_registry`, `model`, `registry`) stay in `yevice-pricing` to avoid duplicating the Bulk API parser/downloader; they are AWS-only by intent, and `yevice-services-aws` is their sole consumer. A future move into `yevice-services-aws` is possible but out of scope here.
