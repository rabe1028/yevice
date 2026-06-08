use std::collections::HashMap;

use yevice_pricing::bulk_api::{
    BulkPriceDimension, PricingDimension, PricingEntry, find_entries, first_price,
    parse_bulk_pricing,
};

fn attributes(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn entry(sku: &str, attrs: &[(&str, &str)], prices: &[f64]) -> PricingEntry {
    PricingEntry {
        sku: sku.to_string(),
        product_family: "Compute".to_string(),
        attributes: attributes(attrs),
        dimensions: prices
            .iter()
            .enumerate()
            .map(|(index, price_usd)| PricingDimension {
                description: format!("dimension-{index}"),
                unit: "Hrs".to_string(),
                price_usd: *price_usd,
                begin_range: index as f64,
                end_range: None,
            })
            .collect(),
    }
}

#[test]
fn usd_price_parses_usd_and_defaults_to_zero_for_missing_or_invalid_values() {
    let valid = BulkPriceDimension {
        description: String::new(),
        begin_range: String::new(),
        end_range: String::new(),
        unit: String::new(),
        price_per_unit: HashMap::from([("USD".to_string(), "1.25".to_string())]),
    };
    assert_eq!(valid.usd_price(), 1.25);

    let missing = BulkPriceDimension {
        description: String::new(),
        begin_range: String::new(),
        end_range: String::new(),
        unit: String::new(),
        price_per_unit: HashMap::from([("JPY".to_string(), "10".to_string())]),
    };
    assert_eq!(missing.usd_price(), 0.0);

    let invalid = BulkPriceDimension {
        description: String::new(),
        begin_range: String::new(),
        end_range: String::new(),
        unit: String::new(),
        price_per_unit: HashMap::from([("USD".to_string(), "not-a-number".to_string())]),
    };
    assert_eq!(invalid.usd_price(), 0.0);
}

#[test]
fn parse_bulk_pricing_extracts_dimensions_and_keeps_products_without_terms() {
    let json = br#"{
        "offerCode": "ExampleOffer",
        "products": {
            "SKU-LAMBDA": {
                "sku": "SKU-LAMBDA",
                "productFamily": "Serverless",
                "attributes": {
                    "servicecode": "AWSLambda",
                    "group": "AWS-Lambda-Duration"
                }
            },
            "SKU-EMPTY": {
                "sku": "SKU-EMPTY",
                "productFamily": "Storage",
                "attributes": {
                    "servicecode": "AmazonS3"
                }
            }
        },
        "terms": {
            "OnDemand": {
                "SKU-LAMBDA": {
                    "SKU-LAMBDA.JRTCKXETXF": {
                        "sku": "SKU-LAMBDA",
                        "priceDimensions": {
                            "REQUESTS": {
                                "description": "Requests",
                                "beginRange": "0",
                                "endRange": "Inf",
                                "unit": "Requests",
                                "pricePerUnit": {"USD": "0.0000002"}
                            },
                            "DURATION": {
                                "description": "Duration",
                                "beginRange": "0",
                                "endRange": "128",
                                "unit": "GB-Second",
                                "pricePerUnit": {"USD": "0.0000999"}
                            }
                        }
                    }
                }
            }
        }
    }"#;

    let entries = parse_bulk_pricing(json).unwrap();
    assert_eq!(entries.len(), 2);

    let lambda = entries
        .iter()
        .find(|entry| entry.sku == "SKU-LAMBDA")
        .unwrap();
    assert_eq!(lambda.product_family, "Serverless");
    assert_eq!(
        lambda.attributes.get("servicecode").map(String::as_str),
        Some("AWSLambda")
    );
    assert_eq!(lambda.dimensions.len(), 2);

    let duration = lambda
        .dimensions
        .iter()
        .find(|dimension| dimension.description == "Duration")
        .unwrap();
    assert_eq!(duration.price_usd, 0.0000999);
    assert_eq!(duration.begin_range, 0.0);
    assert_eq!(duration.end_range, Some(128.0));

    let requests = lambda
        .dimensions
        .iter()
        .find(|dimension| dimension.description == "Requests")
        .unwrap();
    assert_eq!(requests.price_usd, 0.0000002);
    assert_eq!(requests.begin_range, 0.0);
    assert_eq!(requests.end_range, None);

    let empty = entries
        .iter()
        .find(|entry| entry.sku == "SKU-EMPTY")
        .unwrap();
    assert_eq!(empty.product_family, "Storage");
    assert!(empty.dimensions.is_empty());
}

#[test]
fn find_entries_requires_all_filters_to_match() {
    let entries = vec![
        entry(
            "lambda-requests",
            &[("service", "lambda"), ("group", "requests")],
            &[0.2],
        ),
        entry(
            "lambda-duration",
            &[("service", "lambda"), ("group", "duration")],
            &[0.3],
        ),
        entry(
            "s3-storage",
            &[("service", "s3"), ("group", "storage")],
            &[0.4],
        ),
    ];

    let matches = find_entries(&entries, &[("service", "lambda"), ("group", "duration")]);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].sku, "lambda-duration");

    let missing = find_entries(&entries, &[("service", "lambda"), ("group", "storage")]);
    assert!(missing.is_empty());
}

#[test]
fn first_price_skips_non_positive_values_and_returns_none_when_absent() {
    let positive = entry(
        "kinesis",
        &[("service", "kinesis")],
        &[0.0, -0.5, 0.75, 1.2],
    );
    assert_eq!(first_price(&positive), Some(0.75));

    let none = entry("free-tier", &[("service", "misc")], &[0.0, -0.5]);
    assert_eq!(first_price(&none), None);
}
