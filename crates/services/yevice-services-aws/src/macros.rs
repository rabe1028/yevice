//! Declarative service registration macros.
//!
//! `register_aws_services!` consumes a single table that lists every AWS
//! service alongside its CFN adapter and (optional) TF adapter. The macro
//! expands to the three `catalog.register / cfn.register / tf.register`
//! calls so that adding a new service requires editing exactly one row in
//! `lib.rs` instead of three separate blocks.
//!
//! ## Syntax
//!
//! Each row is `<ServicePath> => <kind>;` where `<kind>` is one of:
//!
//! - `cfn = <CfnAdapter>, tf = <TfAdapter>` — register service + CFN + TF.
//! - `cfn = <CfnAdapter>` — register service + CFN only (no TF adapter).
//! - `shared` — register only the service; piggyback on a CFN/TF adapter
//!   that an earlier row already registered. Required when one adapter
//!   produces multiple service ids (e.g. `EcsCfnAdapter` covers both
//!   `aws.ecs_fargate` and `aws.ecs_ec2`); double-registration of the
//!   same resource type panics inside the registry.
//!
//! ```ignore
//! register_aws_services! {
//!     catalog, cfn, tf;
//!
//!     services::lambda::LambdaService
//!         => cfn = cfn::lambda::LambdaCfnAdapter, tf = tf::lambda::LambdaTfAdapter;
//!     services::alb::AlbService => cfn = cfn::alb::AlbCfnAdapter;
//!     services::ecs_ec2::EcsEc2Service => shared;
//! }
//! ```

/// Expand a declarative AWS service table into registry calls.
///
/// See module-level docs for the syntax.
#[macro_export]
macro_rules! register_aws_services {
    // ------------------------------------------------------------------
    // Public entry point — delegates to the recursive `@munch` muncher.
    // ------------------------------------------------------------------
    (
        $catalog:expr, $cfn:expr, $tf:expr;
        $($rows:tt)*
    ) => {
        $crate::register_aws_services!(@munch ($catalog, $cfn, $tf); $($rows)*);
    };

    // No rows left → done.
    (@munch ($catalog:expr, $cfn:expr, $tf:expr); ) => {};

    // Row: service => cfn = <CfnAdapter>, tf = <TfAdapter>;
    (
        @munch ($catalog:expr, $cfn:expr, $tf:expr);
        $service:path => cfn = $cfn_adapter:path, tf = $tf_adapter:path;
        $($rest:tt)*
    ) => {
        $catalog.register($service);
        $cfn.register($cfn_adapter);
        $tf.register($tf_adapter);
        $crate::register_aws_services!(@munch ($catalog, $cfn, $tf); $($rest)*);
    };

    // Row: service => cfn = <CfnAdapter>;
    (
        @munch ($catalog:expr, $cfn:expr, $tf:expr);
        $service:path => cfn = $cfn_adapter:path;
        $($rest:tt)*
    ) => {
        $catalog.register($service);
        $cfn.register($cfn_adapter);
        $crate::register_aws_services!(@munch ($catalog, $cfn, $tf); $($rest)*);
    };

    // Row: service => shared;
    (
        @munch ($catalog:expr, $cfn:expr, $tf:expr);
        $service:path => shared;
        $($rest:tt)*
    ) => {
        $catalog.register($service);
        $crate::register_aws_services!(@munch ($catalog, $cfn, $tf); $($rest)*);
    };
}
