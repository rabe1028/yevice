use yevice_core::cost::Tier;
use yevice_pricing::{PriceRecord, Sku, gcp_hardcoded_pricing, registry::PricingRegistry};

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-12,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn sku_display_and_price_record_helpers_preserve_values() {
    let sku = Sku::dynamic("aws.lambda.requests");
    assert_eq!(sku.as_str(), "aws.lambda.requests");
    assert_eq!(sku.to_string(), "aws.lambda.requests");

    let flat = PriceRecord::flat(0.42);
    assert_close(flat.as_flat().unwrap(), 0.42);

    let tiers = vec![
        Tier {
            upper_limit: Some(1_000.0),
            unit_price: 0.10,
        },
        Tier {
            upper_limit: None,
            unit_price: 0.08,
        },
    ];
    let tiered = PriceRecord::tiered(tiers.clone());
    assert_eq!(tiered.as_tiered().unwrap(), tiers.as_slice());
}

#[test]
fn hardcoded_ec2_prices_cover_all_supported_instance_arms() {
    let registry = PricingRegistry::new("ap-northeast-1");

    for (instance_type, hourly_price) in [
        ("t3.nano", 0.0068),
        ("t3.small", 0.0272),
        ("t3.medium", 0.0544),
        ("t3.large", 0.1088),
        ("t3.xlarge", 0.2176),
        ("t3.2xlarge", 0.4352),
        ("m5.large", 0.124),
        ("m5.xlarge", 0.248),
        ("m5.2xlarge", 0.496),
        ("m5.4xlarge", 0.992),
        ("m6i.large", 0.124),
        ("m6i.xlarge", 0.248),
        ("m6i.2xlarge", 0.496),
        ("c5.large", 0.107),
        ("c5.xlarge", 0.214),
        ("c5.2xlarge", 0.428),
        ("r5.large", 0.152),
        ("r5.xlarge", 0.304),
        ("r5.2xlarge", 0.608),
    ] {
        let price = registry.ec2_price(instance_type).unwrap();
        assert_eq!(price.instance_type, instance_type);
        assert_close(price.hourly_price, hourly_price);
    }
}

#[test]
fn hardcoded_rds_prices_cover_remaining_engine_and_size_combinations() {
    let registry = PricingRegistry::new("ap-northeast-1");

    for ((engine, instance_type), hourly_price) in [
        (("mysql", "db.t3.small"), 0.052),
        (("mariadb", "db.t3.medium"), 0.104),
        (("mysql", "db.r5.large"), 0.290),
        (("mariadb", "db.r5.xlarge"), 0.580),
        (("postgres", "db.t3.micro"), 0.026),
        (("postgres", "db.t3.small"), 0.052),
        (("postgres", "db.t3.medium"), 0.104),
        (("postgres", "db.r5.large"), 0.290),
        (("postgres", "db.r5.xlarge"), 0.580),
        (("aurora-mysql", "db.r5.large"), 0.350),
        (("aurora-postgresql", "db.r5.xlarge"), 0.700),
    ] {
        let price = registry.rds_price(instance_type, engine).unwrap();
        assert_eq!(price.instance_type, instance_type);
        assert_close(price.hourly_price, hourly_price);
        assert_close(price.storage_price_per_gb, 0.138);
    }
}

#[test]
fn hardcoded_elasticache_prices_cover_all_supported_node_types() {
    let registry = PricingRegistry::new("ap-northeast-1");

    for (node_type, hourly_price) in [
        ("cache.t3.micro", 0.021),
        ("cache.t3.small", 0.042),
        ("cache.t3.medium", 0.084),
        ("cache.r6g.large", 0.202),
        ("cache.r6g.xlarge", 0.404),
        ("cache.m6g.large", 0.178),
    ] {
        let price = registry.elasticache_price(node_type).unwrap();
        assert_eq!(price.node_type, node_type);
        assert_close(price.hourly_price, hourly_price);
    }
}

#[test]
fn hardcoded_msk_prices_cover_all_supported_broker_types() {
    let registry = PricingRegistry::new("ap-northeast-1");

    for (broker_type, hourly_price) in [
        ("kafka.t3.small", 0.0456),
        ("kafka.m5.large", 0.213),
        ("kafka.m5.xlarge", 0.425),
        ("kafka.m5.2xlarge", 0.850),
    ] {
        let price = registry.msk_broker_price(broker_type).unwrap();
        assert_close(price.hourly_price, hourly_price);
        assert_close(price.storage_gb_month_price, 0.114);
    }
}

#[test]
fn hardcoded_opensearch_service_prices_cover_all_supported_instance_types() {
    let registry = PricingRegistry::new("ap-northeast-1");

    for (instance_type, hourly_price) in [
        ("t3.small.search", 0.036),
        ("t3.medium.search", 0.073),
        ("m5.large.search", 0.182),
        ("m5.xlarge.search", 0.365),
        ("r5.large.search", 0.250),
        ("r5.xlarge.search", 0.501),
    ] {
        let price = registry.opensearch_service_price(instance_type).unwrap();
        assert_close(price.instance_hour_price, hourly_price);
        assert_close(price.gp2_storage_gb_month_price, 0.135);
    }
}

#[test]
fn hardcoded_documentdb_prices_and_storage_cover_all_supported_types() {
    let registry = PricingRegistry::new("ap-northeast-1");

    for (instance_type, hourly_price) in [
        ("db.t3.medium", 0.076),
        ("db.r5.large", 0.277),
        ("db.r5.xlarge", 0.554),
        ("db.r5.2xlarge", 1.108),
        ("db.r6g.large", 0.264),
        ("db.r6g.xlarge", 0.528),
    ] {
        let price = registry.documentdb_price(instance_type).unwrap();
        assert_close(price.instance_hour_price, hourly_price);
        assert_close(price.storage_gb_month_price, 0.110);
    }

    assert_close(registry.documentdb_storage_price(), 0.110);
}

#[test]
fn hardcoded_redshift_prices_cover_all_supported_node_types() {
    let registry = PricingRegistry::new("ap-northeast-1");

    for (node_type, hourly_price) in [
        ("dc2.large", 0.314),
        ("dc2.8xlarge", 5.024),
        ("ra3.xlplus", 1.086),
        ("ra3.4xlarge", 3.496),
        ("ra3.16xlarge", 13.985),
    ] {
        let price = registry.redshift_price(node_type).unwrap();
        assert_close(price.node_hour_price, hourly_price);
    }
}

#[test]
fn gcp_hardcoded_pricing_respects_region_specific_sql_and_storage_bands() {
    for (region, sql_vcpu, sql_ram, sql_ssd, standard_storage, nearline_storage, archive_storage) in [
        ("us-east1", 0.0413, 0.0070, 0.170, 0.020, 0.013, 0.0025),
        ("europe-west4", 0.0513, 0.0087, 0.187, 0.020, 0.013, 0.0025),
        ("europe-west2", 0.0534, 0.0090, 0.187, 0.023, 0.016, 0.0025),
        ("asia-east1", 0.0488, 0.0083, 0.187, 0.020, 0.013, 0.0025),
        (
            "asia-northeast1",
            0.0559,
            0.0095,
            0.221,
            0.023,
            0.016,
            0.0025,
        ),
        (
            "asia-southeast1",
            0.0526,
            0.0089,
            0.187,
            0.023,
            0.016,
            0.0025,
        ),
        ("asia-south1", 0.0465, 0.0079, 0.187, 0.023, 0.016, 0.0025),
        (
            "australia-southeast1",
            0.0659,
            0.0112,
            0.221,
            0.023,
            0.016,
            0.0025,
        ),
        (
            "southamerica-east1",
            0.0695,
            0.0118,
            0.221,
            0.035,
            0.020,
            0.0025,
        ),
        ("me-central1", 0.0605, 0.0103, 0.221, 0.026, 0.019, 0.0028),
        ("me-west1", 0.0559, 0.0095, 0.221, 0.026, 0.019, 0.0028),
    ] {
        let pricing = gcp_hardcoded_pricing(region);
        assert_eq!(pricing.region, region);
        assert_close(pricing.cloud_run_request_per_million, 0.40);
        assert_close(pricing.cloud_function_invocation_per_million, 0.40);
        assert_close(pricing.pubsub_data_gb, 0.04);
        assert_close(pricing.bigquery_query_per_tb, 5.0);
        assert_close(pricing.cloud_sql_vcpu_hour, sql_vcpu);
        assert_close(pricing.cloud_sql_ram_gb_hour, sql_ram);
        assert_close(pricing.cloud_sql_ssd_gb_month, sql_ssd);
        assert_close(pricing.cloud_storage_standard_gb_month, standard_storage);
        assert_close(pricing.cloud_storage_nearline_gb_month, nearline_storage);
        assert_close(pricing.cloud_storage_archive_gb_month, archive_storage);
    }
}
