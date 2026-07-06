//! Pricing registry backed by downloaded Bulk API JSON files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::bulk_api::{
    PricingEntry, find_entries, find_entries_by_family, first_price, parse_bulk_pricing_full,
};
use crate::error::PricingError;
use crate::model::*;

/// Metadata describing the source and freshness of a loaded pricing file.
#[derive(Debug, Clone)]
pub struct PricingMetadata {
    /// The service key used to identify this file (e.g. `"lambda"`, `"ec2"`).
    pub service_key: String,
    /// ISO 8601 publication date from the Bulk API JSON, if present.
    pub publication_date: Option<String>,
    /// Version string from the Bulk API JSON, if present.
    pub version: Option<String>,
    /// The region this pricing data was loaded for.
    pub region: String,
    /// Currency key detected from `pricePerUnit` (e.g. `"USD"`).
    /// Defaults to `"USD"` when no price dimension is found.
    pub currency: String,
}

/// Pricing registry that loads from downloaded JSON files.
pub struct FilePricingRegistry {
    pub region: String,
    #[allow(dead_code)]
    data_dir: PathBuf,
    /// Map from service key (e.g. `"lambda"`, `"ec2"`) to parsed pricing entries.
    services: HashMap<String, Vec<PricingEntry>>,
    /// Map from service key to file-level metadata extracted at load time.
    metadata: HashMap<String, PricingMetadata>,
}

impl FilePricingRegistry {
    pub fn load(region: impl Into<String>, data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        let region = region.into();

        let service_names = ["lambda", "ec2", "rds", "dynamodb", "kinesis"];

        let mut services: HashMap<String, Vec<PricingEntry>> = HashMap::new();
        let mut metadata: HashMap<String, PricingMetadata> = HashMap::new();
        for name in &service_names {
            if let Some((entries, meta)) = load_service(&data_dir, name, &region) {
                services.insert(name.to_string(), entries);
                metadata.insert(name.to_string(), meta);
            }
        }

        let loaded_count = services.len();
        tracing::info!(
            data_dir = %data_dir.display(),
            loaded_count,
            "loaded pricing data from files"
        );

        Self {
            region,
            data_dir,
            services,
            metadata,
        }
    }

    /// Returns metadata for `service_key`, or `None` if that file was not loaded.
    pub fn metadata(&self, service_key: &str) -> Option<&PricingMetadata> {
        self.metadata.get(service_key)
    }

    /// Returns metadata for all successfully loaded services.
    pub fn all_metadata(&self) -> Vec<&PricingMetadata> {
        self.metadata.values().collect()
    }

    /// Returns loaded entries for `service_key`, or `None` if that file was
    /// not loaded.
    fn entries(&self, service_key: &str) -> Option<&Vec<PricingEntry>> {
        self.services.get(service_key)
    }

    fn entries_required(&self, service_key: &str) -> Result<&Vec<PricingEntry>, PricingError> {
        self.entries(service_key)
            .ok_or_else(|| self.not_found(service_key))
    }

    fn not_found(&self, service: impl Into<String>) -> PricingError {
        PricingError::NotFound {
            service: service.into(),
            region: self.region.clone(),
        }
    }

    pub fn lambda_price(&self) -> Result<LambdaPrice, PricingError> {
        let entries = self.entries_required("lambda")?;
        let request_price = required_price(
            self,
            "lambda:AWS-Lambda-Requests",
            find_entries(entries, &[("group", "AWS-Lambda-Requests")]),
        )?;
        let gb_second_price = required_price(
            self,
            "lambda:AWS-Lambda-Duration",
            find_entries(entries, &[("group", "AWS-Lambda-Duration")]),
        )?;

        Ok(LambdaPrice {
            request_price,
            gb_second_price,
            free_tier_requests: 1_000_000.0,
            free_tier_gb_seconds: 400_000.0,
        })
    }

    pub fn ec2_price(&self, instance_type: &str) -> Result<Ec2Price, PricingError> {
        let entries = self.entries_required("ec2")?;
        let matches = find_entries(
            entries,
            &[
                ("instanceType", instance_type),
                ("operatingSystem", "Linux"),
                ("tenancy", "Shared"),
                ("preInstalledSw", "NA"),
                ("capacitystatus", "Used"),
            ],
        );
        Ok(Ec2Price {
            instance_type: instance_type.to_string(),
            hourly_price: required_price(self, format!("ec2:{instance_type}"), matches)?,
        })
    }

    pub fn rds_price(&self, instance_type: &str, engine: &str) -> Result<RdsPrice, PricingError> {
        let entries = self.entries_required("rds")?;
        let matches = match engine {
            "mysql" | "mariadb" => find_entries(
                entries,
                &[("instanceType", instance_type), ("databaseEngine", "MySQL")],
            ),
            "postgres" => find_entries(
                entries,
                &[
                    ("instanceType", instance_type),
                    ("databaseEngine", "PostgreSQL"),
                ],
            ),
            "aurora-mysql" => find_entries(
                entries,
                &[
                    ("instanceType", instance_type),
                    ("databaseEngine", "Aurora MySQL"),
                ],
            ),
            "aurora-postgresql" => find_entries(
                entries,
                &[
                    ("instanceType", instance_type),
                    ("databaseEngine", "Aurora PostgreSQL"),
                ],
            ),
            "sqlserver-se" | "sqlserver" => find_entries(
                entries,
                &[
                    ("instanceType", instance_type),
                    ("databaseEngine", "SQL Server"),
                    ("databaseEdition", "Standard"),
                ],
            ),
            _ => find_entries(
                entries,
                &[("instanceType", instance_type), ("databaseEngine", engine)],
            ),
        };

        Ok(RdsPrice {
            instance_type: instance_type.to_string(),
            hourly_price: required_price(self, format!("rds:{engine}:{instance_type}"), matches)?,
            storage_price_per_gb: 0.138, // gp2 default; Bulk parser support is still pending.
        })
    }

    /// RDS gp3 storage price per GB-month.
    ///
    /// Looks for a `Database Storage` entry with `volumeType = "General Purpose-GP3"` in
    /// the downloaded `rds.json` file.
    pub fn rds_gp3_storage_price(&self) -> Result<f64, PricingError> {
        let entries = self.entries_required("rds")?;
        required_price(
            self,
            "rds:gp3_storage",
            find_entries_by_family(
                entries,
                "Database Storage",
                &[
                    ("volumeType", "General Purpose-GP3"),
                    ("deploymentOption", "Single-AZ"),
                ],
            ),
        )
    }

    /// RDS gp3 excess IOPS price per IOPS-month.
    ///
    /// Looks for a `System Operation` entry with `group = "RDS-GP3-IOPS"` in the downloaded
    /// `rds.json` file.
    pub fn rds_gp3_iops_price(&self) -> Result<f64, PricingError> {
        let entries = self.entries_required("rds")?;
        required_price(
            self,
            "rds:gp3_iops",
            find_entries(
                entries,
                &[("group", "RDS-GP3-IOPS"), ("deploymentOption", "Single-AZ")],
            ),
        )
    }

    pub fn dynamodb_price(&self) -> Result<DynamoDbPrice, PricingError> {
        let entries = self.entries_required("dynamodb")?;
        Ok(DynamoDbPrice {
            write_request_price: dynamodb_required_price(
                self,
                entries,
                "dynamodb:pay_per_request_write",
                "DDB-WriteUnits",
                "PayPerRequestThroughput",
            )?,
            read_request_price: dynamodb_required_price(
                self,
                entries,
                "dynamodb:pay_per_request_read",
                "DDB-ReadUnits",
                "PayPerRequestThroughput",
            )?,
            wcu_hour_price: dynamodb_required_price(
                self,
                entries,
                "dynamodb:provisioned_wcu_hour",
                "DDB-WriteUnits",
                "CommittedThroughput",
            )?,
            rcu_hour_price: dynamodb_required_price(
                self,
                entries,
                "dynamodb:provisioned_rcu_hour",
                "DDB-ReadUnits",
                "CommittedThroughput",
            )?,
            storage_price_per_gb: dynamodb_storage_price(self, entries)?,
            free_tier_wru: 25_000.0,
            free_tier_rru: 25_000.0,
            free_tier_storage_gb: 25.0,
        })
    }

    pub fn kinesis_price(&self) -> Result<KinesisPrice, PricingError> {
        let entries = self.entries_required("kinesis")?;
        Ok(KinesisPrice {
            shard_hour_price: kinesis_price_by_attrs(
                self,
                entries,
                "Provisioned shard hour",
                &[
                    ("operation", "shardHourStorage"),
                    ("group", "Provisioned shard hour"),
                ],
            )?,
            put_payload_unit_price: kinesis_price_by_attrs(
                self,
                entries,
                "PUT payload units",
                &[("operation", "PutRequest")],
            )?,
            on_demand_ingestion_price_per_gb: kinesis_price_by_attrs(
                self,
                entries,
                "On-Demand data ingestion",
                &[("operation", "OnDemandDataIngested"), ("group", "OnDemand")],
            )?,
            on_demand_retrieval_price_per_gb: kinesis_price_by_attrs(
                self,
                entries,
                "On-Demand GetRecords retrieval",
                &[
                    ("operation", "OnDemandDataRetrieval"),
                    ("group", "OnDemand"),
                ],
            )?,
            on_demand_stream_hour_price: kinesis_price_by_attrs(
                self,
                entries,
                "On-Demand stream hour",
                &[("operation", "OnDemandStreamHr"), ("group", "OnDemand")],
            )?,
        })
    }
}

fn kinesis_price_by_attrs(
    registry: &FilePricingRegistry,
    entries: &[PricingEntry],
    label: &str,
    filters: &[(&str, &str)],
) -> Result<f64, PricingError> {
    required_price(
        registry,
        format!("kinesis:{label}"),
        find_entries_by_family(entries, "Kinesis Streams", filters),
    )
}

fn dynamodb_required_price(
    registry: &FilePricingRegistry,
    entries: &[PricingEntry],
    label: &str,
    group: &str,
    operation: &str,
) -> Result<f64, PricingError> {
    required_price(
        registry,
        label,
        entries.iter().filter(|entry| {
            entry.attributes.get("group").is_some_and(|v| v == group)
                && entry
                    .attributes
                    .get("operation")
                    .is_some_and(|v| v == operation)
                && is_dynamodb_standard_table_class(entry)
        }),
    )
}

fn dynamodb_storage_price(
    registry: &FilePricingRegistry,
    entries: &[PricingEntry],
) -> Result<f64, PricingError> {
    required_price(
        registry,
        "dynamodb:storage",
        entries.iter().filter(|entry| {
            entry.product_family == "Database Storage"
                && is_dynamodb_standard_table_class(entry)
                && entry
                    .attributes
                    .get("usagetype")
                    .is_some_and(|usage| usage.ends_with("-TimedStorage-ByteHrs"))
        }),
    )
}

fn is_dynamodb_standard_table_class(entry: &PricingEntry) -> bool {
    if let Some(table_class) = entry.attributes.get("tableClass") {
        return matches!(
            table_class.as_str(),
            "Standard" | "DynamoDB Standard" | "DynamoDB Standard table class"
        );
    }

    let has_ia_usage = entry
        .attributes
        .get("usagetype")
        .is_some_and(|usage| usage.contains("-IA-"));
    let has_ia_description = entry.dimensions.iter().any(|dim| {
        dim.description.contains("Standard-IA")
            || dim.description.contains("Standard Infrequent Access")
    });

    !has_ia_usage && !has_ia_description
}

fn required_price<'a>(
    registry: &FilePricingRegistry,
    service: impl Into<String>,
    matches: impl IntoIterator<Item = &'a PricingEntry>,
) -> Result<f64, PricingError> {
    matches
        .into_iter()
        .find_map(first_price)
        .ok_or_else(|| registry.not_found(service))
}

fn load_service(
    data_dir: &Path,
    name: &str,
    region: &str,
) -> Option<(Vec<PricingEntry>, PricingMetadata)> {
    let path = data_dir.join(format!("{name}.json"));
    match std::fs::read(&path) {
        Ok(data) => match parse_bulk_pricing_full(&data) {
            Ok((entries, bulk_meta)) => {
                tracing::debug!(service = name, entries = entries.len(), "loaded pricing");
                tracing::info!(
                    service = name,
                    publication_date = bulk_meta.publication_date.as_deref().unwrap_or("unknown"),
                    version = bulk_meta.version.as_deref().unwrap_or("unknown"),
                    "loaded pricing file metadata"
                );
                let meta = PricingMetadata {
                    service_key: name.to_string(),
                    publication_date: bulk_meta.publication_date,
                    version: bulk_meta.version,
                    region: region.to_string(),
                    currency: bulk_meta.currency,
                };
                Some((entries, meta))
            }
            Err(e) => {
                tracing::warn!(service = name, error = %e, "failed to parse pricing file");
                None
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(service = name, "pricing file not found; skipping");
            None
        }
        Err(e) => {
            tracing::warn!(service = name, error = %e, "failed to read pricing file");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bulk_api::PricingDimension;

    /// Build a minimal Bulk Pricing JSON fixture with a single SKU whose
    /// `group` attribute is deliberately set to `WRONG_GROUP_NAME`.  When the
    /// registry looks up the real group name, `find_entries` returns empty,
    /// which must surface as `PricingError::NotFound`.
    fn minimal_bulk_json_with_wrong_group(offer_code: &str) -> String {
        format!(
            r#"{{
              "offerCode": "{offer_code}",
              "products": {{
                "SKU001": {{
                  "sku": "SKU001",
                  "productFamily": "Compute",
                  "attributes": {{"group": "WRONG_GROUP_NAME"}}
                }}
              }},
              "terms": {{
                "OnDemand": {{
                  "SKU001": {{
                    "SKU001.JRTCKXETXF": {{
                      "sku": "SKU001",
                      "priceDimensions": {{
                        "SKU001.JRTCKXETXF.6YS6EN2CT7": {{
                          "description": "per request",
                          "beginRange": "0",
                          "endRange": "Inf",
                          "unit": "Requests",
                          "pricePerUnit": {{"USD": "0.000001"}}
                        }}
                      }}
                    }}
                  }}
                }}
              }}
            }}"#
        )
    }

    fn minimal_kinesis_bulk_json() -> &'static str {
        r#"{
          "offerCode": "AmazonKinesis",
          "products": {
            "SHARD": {
              "sku": "SHARD",
              "productFamily": "Kinesis Streams",
              "attributes": {
                "group": "Provisioned shard hour",
                "operation": "shardHourStorage",
                "usagetype": "APN1-Storage-ShardHour"
              }
            },
            "PUT": {
              "sku": "PUT",
              "productFamily": "Kinesis Streams",
              "attributes": {
                "group": "Payload Units",
                "operation": "PutRequest",
                "usagetype": "APN1-PutRequestPayloadUnits"
              }
            },
            "ODIN": {
              "sku": "ODIN",
              "productFamily": "Kinesis Streams",
              "attributes": {
                "group": "OnDemand",
                "operation": "OnDemandDataIngested",
                "usagetype": "APN1-OnDemand-BilledIncomingBytes"
              }
            },
            "ODOUT": {
              "sku": "ODOUT",
              "productFamily": "Kinesis Streams",
              "attributes": {
                "group": "OnDemand",
                "operation": "OnDemandDataRetrieval",
                "usagetype": "APN1-OnDemand-BilledOutgoingBytes"
              }
            },
            "ODHR": {
              "sku": "ODHR",
              "productFamily": "Kinesis Streams",
              "attributes": {
                "group": "OnDemand",
                "operation": "OnDemandStreamHr",
                "usagetype": "APN1-OnDemand-StreamHour"
              }
            },
            "ADV": {
              "sku": "ADV",
              "productFamily": "Kinesis Streams",
              "attributes": {
                "group": "OnDemand Advantage",
                "operation": "AdvantageDataRetrieval",
                "usagetype": "APN1-Advantage-BilledOutgoingBytes"
              }
            }
          },
          "terms": {
            "OnDemand": {
              "SHARD": {
                "SHARD.JRTCKXETXF": {
                  "sku": "SHARD",
                  "priceDimensions": {
                    "SHARD.RATE": {
                      "description": "provisioned shard hour",
                      "beginRange": "0",
                      "endRange": "Inf",
                      "unit": "ShardHour",
                      "pricePerUnit": {"USD": "0.0195000000"}
                    }
                  }
                }
              },
              "PUT": {
                "PUT.JRTCKXETXF": {
                  "sku": "PUT",
                  "priceDimensions": {
                    "PUT.RATE": {
                      "description": "PUT payload units",
                      "beginRange": "0",
                      "endRange": "Inf",
                      "unit": "PutRequest",
                      "pricePerUnit": {"USD": "0.0000000215"}
                    }
                  }
                }
              },
              "ODIN": {
                "ODIN.JRTCKXETXF": {
                  "sku": "ODIN",
                  "priceDimensions": {
                    "ODIN.RATE": {
                      "description": "on-demand data written",
                      "beginRange": "0",
                      "endRange": "Inf",
                      "unit": "GB",
                      "pricePerUnit": {"USD": "0.1040000000"}
                    }
                  }
                }
              },
              "ODOUT": {
                "ODOUT.JRTCKXETXF": {
                  "sku": "ODOUT",
                  "priceDimensions": {
                    "ODOUT.RATE": {
                      "description": "on-demand GetRecords data read",
                      "beginRange": "0",
                      "endRange": "Inf",
                      "unit": "GB",
                      "pricePerUnit": {"USD": "0.0520000000"}
                    }
                  }
                }
              },
              "ODHR": {
                "ODHR.JRTCKXETXF": {
                  "sku": "ODHR",
                  "priceDimensions": {
                    "ODHR.RATE": {
                      "description": "on-demand stream hour",
                      "beginRange": "0",
                      "endRange": "Inf",
                      "unit": "StreamHr",
                      "pricePerUnit": {"USD": "0.0520000000"}
                    }
                  }
                }
              },
              "ADV": {
                "ADV.JRTCKXETXF": {
                  "sku": "ADV",
                  "priceDimensions": {
                    "ADV.RATE": {
                      "description": "advantage retrieval must not be selected",
                      "beginRange": "0",
                      "endRange": "Inf",
                      "unit": "GB",
                      "pricePerUnit": {"USD": "0.0208000000"}
                    }
                  }
                }
              }
            }
          }
        }"#
    }

    /// Create a uniquely-named temporary directory (no external crate needed).
    fn make_temp_dir(suffix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "yevice_pricing_test_{}_{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn assert_not_found(result: Result<impl std::fmt::Debug, PricingError>, service: &str) {
        match result {
            Err(PricingError::NotFound {
                service: actual, ..
            }) => assert_eq!(actual, service),
            other => panic!("expected NotFound for {service}, got {other:?}"),
        }
    }

    fn test_registry() -> FilePricingRegistry {
        FilePricingRegistry {
            region: "ap-northeast-1".to_string(),
            data_dir: std::path::PathBuf::new(),
            services: std::collections::HashMap::new(),
            metadata: std::collections::HashMap::new(),
        }
    }

    fn pricing_entry(product_family: &str, usagetype: &str, price: f64) -> PricingEntry {
        PricingEntry {
            sku: format!("{product_family}:{usagetype}"),
            product_family: product_family.to_string(),
            attributes: std::collections::HashMap::from([(
                "usagetype".to_string(),
                usagetype.to_string(),
            )]),
            dimensions: vec![PricingDimension {
                description: "test price".to_string(),
                unit: "GB-Mo".to_string(),
                price_usd: price,
                begin_range: 0.0,
                end_range: None,
            }],
        }
    }

    fn dynamodb_entry(group: &str, operation: &str, usagetype: &str, price: f64) -> PricingEntry {
        PricingEntry {
            sku: format!("{group}:{operation}:{usagetype}"),
            product_family: "Amazon DynamoDB".to_string(),
            attributes: std::collections::HashMap::from([
                ("group".to_string(), group.to_string()),
                ("operation".to_string(), operation.to_string()),
                ("usagetype".to_string(), usagetype.to_string()),
            ]),
            dimensions: vec![PricingDimension {
                description: format!("{usagetype} test price"),
                unit: "Requests".to_string(),
                price_usd: price,
                begin_range: 0.0,
                end_range: None,
            }],
        }
    }

    #[test]
    fn load_skips_service_files_without_implemented_file_backed_lookups() {
        let dir = make_temp_dir("unsupported_service_files");
        std::fs::write(
            dir.join("s3.json"),
            minimal_bulk_json_with_wrong_group("AmazonS3"),
        )
        .unwrap();

        let reg = FilePricingRegistry::load("ap-northeast-1", &dir);

        assert!(reg.metadata("s3").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When a downloaded Lambda pricing file loads successfully but has no
    /// `AWS-Lambda-Requests` group, lookup must fail instead of returning a
    /// hardcoded price.
    #[test]
    fn lambda_missing_group_returns_not_found() {
        let dir = make_temp_dir("lambda");
        std::fs::write(
            dir.join("lambda.json"),
            minimal_bulk_json_with_wrong_group("AmazonLambda"),
        )
        .unwrap();

        let reg = FilePricingRegistry::load("ap-northeast-1", &dir);
        assert_not_found(reg.lambda_price(), "lambda:AWS-Lambda-Requests");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When a downloaded DynamoDB pricing file loads successfully but has no
    /// expected pricing groups, lookup must fail instead of returning hardcoded
    /// prices.
    #[test]
    fn dynamodb_missing_group_returns_not_found() {
        let dir = make_temp_dir("dynamodb");
        std::fs::write(
            dir.join("dynamodb.json"),
            minimal_bulk_json_with_wrong_group("AmazonDynamoDB"),
        )
        .unwrap();

        let reg = FilePricingRegistry::load("ap-northeast-1", &dir);
        assert_not_found(reg.dynamodb_price(), "dynamodb:pay_per_request_write");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dynamodb_required_price_prefers_standard_table_class_over_standard_ia() {
        let registry = test_registry();
        let entries = vec![
            dynamodb_entry(
                "DDB-WriteUnits",
                "PayPerRequestThroughput",
                "APN1-IA-WriteRequestUnits",
                999.0,
            ),
            dynamodb_entry(
                "DDB-WriteUnits",
                "PayPerRequestThroughput",
                "APN1-WriteRequestUnits",
                0.00000125,
            ),
        ];

        let price = dynamodb_required_price(
            &registry,
            &entries,
            "dynamodb:pay_per_request_write",
            "DDB-WriteUnits",
            "PayPerRequestThroughput",
        )
        .unwrap();

        assert_eq!(price, 0.00000125);
    }

    #[test]
    fn dynamodb_standard_table_class_rejects_infrequent_access_description() {
        let mut entry = dynamodb_entry(
            "DDB-ReadUnits",
            "PayPerRequestThroughput",
            "APN1-ReadRequestUnits",
            999.0,
        );
        entry.dimensions[0].description =
            "DynamoDB Standard Infrequent Access read request".to_string();

        assert!(!is_dynamodb_standard_table_class(&entry));
    }

    #[test]
    fn dynamodb_storage_price_requires_storage_family_and_timed_storage_usage() {
        let registry = test_registry();
        let entries = vec![
            pricing_entry("Database Storage", "APN1-IA-TimedStorage-ByteHrs", 1.10),
            pricing_entry("Wrong Family", "APN1-TimedStorage-ByteHrs", 999.0),
            pricing_entry("Database Storage", "APN1-NotStorage-ByteHrs", 888.0),
            pricing_entry("Database Storage", "APN1-TimedStorage-ByteHrs", 0.275),
        ];

        let price = dynamodb_storage_price(&registry, &entries).unwrap();

        assert_eq!(price, 0.275);
    }

    /// When a downloaded Kinesis pricing file loads successfully but has no
    /// matching Kinesis Streams entries, lookup must fail instead of returning
    /// hardcoded prices.
    #[test]
    fn kinesis_missing_group_returns_not_found() {
        let dir = make_temp_dir("kinesis");
        std::fs::write(
            dir.join("kinesis.json"),
            minimal_bulk_json_with_wrong_group("AmazonKinesis"),
        )
        .unwrap();

        let reg = FilePricingRegistry::load("ap-northeast-1", &dir);
        assert_not_found(reg.kinesis_price(), "kinesis:Provisioned shard hour");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kinesis_price_reads_current_bulk_api_attributes() {
        let dir = make_temp_dir("kinesis_current");
        std::fs::write(dir.join("kinesis.json"), minimal_kinesis_bulk_json()).unwrap();

        let reg = FilePricingRegistry::load("ap-northeast-1", &dir);
        let price = reg.kinesis_price().unwrap();

        assert_eq!(price.shard_hour_price, 0.0195);
        assert_eq!(price.put_payload_unit_price, 0.0000000215);
        assert_eq!(price.on_demand_ingestion_price_per_gb, 0.104);
        assert_eq!(price.on_demand_retrieval_price_per_gb, 0.052);
        assert_eq!(price.on_demand_stream_hour_price, 0.052);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Missing gp3 entries in a loaded `rds.json` must fail instead of falling
    /// back to hardcoded Tokyo constants.
    #[test]
    fn rds_gp3_missing_entries_return_not_found() {
        let dir = make_temp_dir("rds_gp3_missing");
        // Write an rds.json with no gp3 entries so the lookup path returns None.
        std::fs::write(
            dir.join("rds.json"),
            minimal_bulk_json_with_wrong_group("AmazonRDS"),
        )
        .unwrap();

        let reg = FilePricingRegistry::load("us-east-1", &dir);

        assert_not_found(reg.rds_gp3_storage_price(), "rds:gp3_storage");
        assert_not_found(reg.rds_gp3_iops_price(), "rds:gp3_iops");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_service_file_returns_not_found() {
        let dir = make_temp_dir("missing_file");
        let reg = FilePricingRegistry::load("us-east-1", &dir);

        assert_not_found(reg.lambda_price(), "lambda");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
