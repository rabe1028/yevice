use crate::error::PricingError;
use crate::model::*;

/// Pricing registry that provides pricing data for AWS services.
///
/// For MVP, this uses hardcoded ap-northeast-1 pricing data.
/// In the future, this will be populated from AWS Bulk Pricing API data.
pub struct PricingRegistry {
    pub region: String,
}

impl PricingRegistry {
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            region: region.into(),
        }
    }

    /// Look up EC2 on-demand pricing for a given instance type.
    pub fn ec2_price(&self, instance_type: &str) -> Result<Ec2Price, PricingError> {
        // ap-northeast-1 on-demand Linux pricing (USD/hr)
        let hourly = match instance_type {
            "t3.nano" => 0.0068,
            "t3.micro" => 0.0136,
            "t3.small" => 0.0272,
            "t3.medium" => 0.0544,
            "t3.large" => 0.1088,
            "t3.xlarge" => 0.2176,
            "t3.2xlarge" => 0.4352,
            "m5.large" => 0.124,
            "m5.xlarge" => 0.248,
            "m5.2xlarge" => 0.496,
            "m5.4xlarge" => 0.992,
            "m6i.large" => 0.124,
            "m6i.xlarge" => 0.248,
            "m6i.2xlarge" => 0.496,
            "m6a.large" => 0.1116,
            "m6a.xlarge" => 0.2232,
            "m6a.2xlarge" => 0.4464,
            "c5.large" => 0.107,
            "c5.xlarge" => 0.214,
            "c5.2xlarge" => 0.428,
            "r5.large" => 0.152,
            "r5.xlarge" => 0.304,
            "r5.2xlarge" => 0.608,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("EC2:{instance_type}"),
                    region: self.region.clone(),
                });
            }
        };

        Ok(Ec2Price {
            instance_type: instance_type.to_string(),
            hourly_price: hourly,
        })
    }

    /// EC2 on-demand Windows pricing (USD/hr, ap-northeast-1).
    /// Windows = Linux base + license uplift (~$0.046/vCPU-hr).
    pub fn ec2_windows_hourly_price(&self, instance_type: &str) -> Result<f64, PricingError> {
        Ok(match instance_type {
            "m5.large" => 0.216,
            "m5.xlarge" => 0.432,
            "m5.2xlarge" => 0.864,
            "m6a.large" => 0.2036,
            "m6a.xlarge" => 0.4072,
            "m6a.2xlarge" => 0.8144,
            "m6i.large" => 0.216,
            "m6i.xlarge" => 0.432,
            "t3.medium" => 0.1464,
            "t3.large" => 0.2008,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("EC2:Windows:{instance_type}"),
                    region: self.region.clone(),
                });
            }
        })
    }

    /// AWS Lambda HTTP response streaming processed-bytes price per GB
    /// (ap-northeast-1). Verified $0.008/GiB via the AWS Price List API.
    pub fn lambda_http_stream_gb_price(&self) -> f64 {
        0.008
    }

    /// Lambda pricing for ap-northeast-1.
    pub fn lambda_price(&self) -> LambdaPrice {
        LambdaPrice {
            request_price: Self::LAMBDA_REQUEST_PRICE, // $0.20 per 1M requests
            gb_second_price: Self::LAMBDA_GB_SECOND_PRICE, // per GB-second
            free_tier_requests: 1_000_000.0,           // 1M free requests/month
            free_tier_gb_seconds: 400_000.0,           // 400K GB-seconds/month
        }
    }

    /// RDS on-demand pricing for a given instance type.
    pub fn rds_price(&self, instance_type: &str, engine: &str) -> Result<RdsPrice, PricingError> {
        // ap-northeast-1 on-demand pricing (simplified)
        let hourly = match (engine, instance_type) {
            ("mysql" | "mariadb", "db.t3.micro") => 0.026,
            ("mysql" | "mariadb", "db.t3.small") => 0.052,
            ("mysql" | "mariadb", "db.t3.medium") => 0.104,
            // db.m6i.large single-AZ; derived from AWS CDP ec-container
            // reference (Multi-AZ 730h = $343.10 -> single-AZ $0.235/h).
            ("mysql" | "mariadb", "db.m6i.large") => 0.235,
            ("mysql" | "mariadb", "db.r5.large") => 0.290,
            ("mysql" | "mariadb", "db.r5.xlarge") => 0.580,
            ("postgres", "db.t3.micro") => 0.026,
            ("postgres", "db.t3.small") => 0.052,
            ("postgres", "db.t3.medium") => 0.104,
            ("postgres", "db.m6i.large") => 0.235,
            ("postgres", "db.r5.large") => 0.290,
            ("postgres", "db.r5.xlarge") => 0.580,
            ("aurora-mysql" | "aurora-postgresql", "db.r5.large") => 0.350,
            ("aurora-mysql" | "aurora-postgresql", "db.r5.xlarge") => 0.700,
            // SQL Server Standard Edition (License Included), Single-AZ,
            // ap-northeast-1. Multi-AZ doubles at the service layer (az_mult).
            // Other editions (ee/ex/web) intentionally have no rate here.
            ("sqlserver-se", "db.r5.large") => 1.050,
            ("sqlserver-se", "db.r5.xlarge") => 2.100,
            ("sqlserver-se", "db.t3.medium") => 0.342,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("RDS:{engine}:{instance_type}"),
                    region: self.region.clone(),
                });
            }
        };

        // gp2 storage price
        let storage_price = 0.138; // per GB-month

        Ok(RdsPrice {
            instance_type: instance_type.to_string(),
            hourly_price: hourly,
            storage_price_per_gb: storage_price,
        })
    }

    /// RDS gp3 storage price per GB-month (ap-northeast-1).
    pub fn rds_gp3_storage_price(&self) -> f64 {
        Self::RDS_GP3_STORAGE_PRICE
    }

    /// RDS gp3 storage price per GB-month fallback constant (ap-northeast-1).
    pub(crate) const RDS_GP3_STORAGE_PRICE: f64 = 0.1216;

    /// RDS gp3 excess IOPS price per IOPS-month (ap-northeast-1).
    pub fn rds_gp3_iops_price(&self) -> f64 {
        Self::RDS_GP3_IOPS_PRICE
    }

    /// RDS gp3 excess IOPS price fallback constant (ap-northeast-1).
    pub(crate) const RDS_GP3_IOPS_PRICE: f64 = 0.008;

    /// S3 Standard pricing for ap-northeast-1.
    pub fn s3_price(&self) -> S3Price {
        S3Price {
            storage_tiers: vec![
                S3StorageTier {
                    upper_limit_gb: Some(50_000.0), // First 50 TB
                    price_per_gb: 0.025,
                },
                S3StorageTier {
                    upper_limit_gb: Some(500_000.0), // Next 450 TB
                    price_per_gb: 0.024,
                },
                S3StorageTier {
                    upper_limit_gb: None, // Over 500 TB
                    price_per_gb: 0.023,
                },
            ],
            put_request_price: 0.0000047,  // per request
            get_request_price: 0.00000037, // per request
        }
    }

    /// `DynamoDB` on-demand pricing for ap-northeast-1.
    pub fn dynamodb_price(&self) -> DynamoDbPrice {
        DynamoDbPrice {
            // On-Demand (PAY_PER_REQUEST)
            write_request_price: Self::DYNAMODB_WRITE_REQUEST_PRICE, // per WRU ($1.4269 per million)
            read_request_price: Self::DYNAMODB_READ_REQUEST_PRICE, // per RRU ($0.2854 per million)
            // Provisioned
            wcu_hour_price: 0.000742, // $0.000742 per WCU-hour ($0.5417 per WCU-month)
            rcu_hour_price: 0.0001484, // $0.0001484 per RCU-hour ($0.1083 per RCU-month)
            // Common
            storage_price_per_gb: Self::DYNAMODB_STORAGE_PRICE, // per GB-month
            free_tier_wru: 25_000.0,                            // 25K WCU (equivalent)
            free_tier_rru: 25_000.0,                            // 25K RCU (equivalent)
            free_tier_storage_gb: 25.0,                         // 25 GB
        }
    }

    /// ECS Fargate pricing for ap-northeast-1.
    pub fn fargate_price(&self) -> FargatePrice {
        FargatePrice {
            vcpu_hour_price: 0.05056,      // per vCPU per hour
            memory_gb_hour_price: 0.00553, // per GB per hour
        }
    }

    /// `OpenSearch` Serverless pricing for ap-northeast-1.
    pub fn opensearch_serverless_price(&self) -> OpenSearchServerlessPrice {
        OpenSearchServerlessPrice {
            ocu_hour_price: 0.334,       // per OCU per hour
            storage_price_per_gb: 0.026, // per GB-month
        }
    }

    // ---------------------------------------------------------------------------
    // Canonical fallback constants shared with FilePricingRegistry.
    // These are the single source of truth; file_registry.rs references these
    // when a downloaded pricing file loads successfully but the expected group
    // attribute is missing or yields no price.
    // ---------------------------------------------------------------------------

    /// Lambda: per-request price fallback (matches `lambda_price`).
    pub(crate) const LAMBDA_REQUEST_PRICE: f64 = 0.0000002;
    /// Lambda: per-GB-second price fallback (matches `lambda_price`).
    pub(crate) const LAMBDA_GB_SECOND_PRICE: f64 = 0.0000166667;

    /// DynamoDB on-demand: per-WRU price fallback (matches `dynamodb_price`).
    pub(crate) const DYNAMODB_WRITE_REQUEST_PRICE: f64 = 0.000000715;
    /// DynamoDB on-demand: per-RRU price fallback (matches `dynamodb_price`).
    pub(crate) const DYNAMODB_READ_REQUEST_PRICE: f64 = 0.000000143;
    /// DynamoDB: storage per-GB-month price fallback (matches `dynamodb_price`).
    pub(crate) const DYNAMODB_STORAGE_PRICE: f64 = 0.285;

    /// Kinesis: shard-hour price fallback (matches `kinesis_price`).
    pub(crate) const KINESIS_SHARD_HOUR_PRICE: f64 = 0.0195;
    /// Kinesis: PUT-payload-unit price fallback (matches `kinesis_price`).
    pub(crate) const KINESIS_PUT_PAYLOAD_UNIT_PRICE: f64 = 0.0000002;

    /// Kinesis Data Streams pricing for ap-northeast-1.
    pub fn kinesis_price(&self) -> KinesisPrice {
        KinesisPrice {
            // Provisioned mode
            shard_hour_price: Self::KINESIS_SHARD_HOUR_PRICE,
            put_payload_unit_price: Self::KINESIS_PUT_PAYLOAD_UNIT_PRICE,
            // On-Demand mode
            on_demand_ingestion_price_per_gb: 0.098, // $0.098 per GB
            on_demand_retrieval_price_per_gb: 0.034, // $0.034 per GB
            on_demand_stream_hour_price: 0.052,      // $0.052 per stream-hour
        }
    }

    /// SQS pricing for ap-northeast-1.
    pub fn sqs_price(&self) -> SqsPrice {
        SqsPrice {
            standard_request_price: 0.0000004, // $0.40 per million
            fifo_request_price: 0.0000005,     // $0.50 per million
            free_tier_requests: 1_000_000.0,   // 1M free/month
        }
    }

    /// `CloudWatch` custom metric price per metric per month (ap-northeast-1).
    /// First 10,000 metrics are $0.30 each; used by Container Insights.
    pub fn cloudwatch_custom_metric_month_price(&self) -> f64 {
        0.30
    }

    /// `CloudWatch` Logs pricing for ap-northeast-1.
    pub fn cloudwatch_logs_price(&self) -> CloudWatchLogsPrice {
        CloudWatchLogsPrice {
            ingestion_price_per_gb: 0.76,
            storage_price_per_gb: 0.033,
            free_tier_ingestion_gb: 5.0,
            free_tier_storage_gb: 5.0,
        }
    }

    /// API Gateway pricing for ap-northeast-1.
    pub fn api_gateway_price(&self) -> ApiGatewayPrice {
        ApiGatewayPrice {
            rest_api_request_price: 0.00000435, // $4.35 per million
            http_api_request_price: 0.00000120, // $1.20 per million
            free_tier_requests: 1_000_000.0,    // 1M free/month (first 12 months)
        }
    }

    /// NAT Gateway pricing for ap-northeast-1.
    pub fn nat_gateway_price(&self) -> NatGatewayPrice {
        NatGatewayPrice {
            hourly_price: 0.062,                 // $0.062 per hour
            data_processing_price_per_gb: 0.062, // $0.062 per GB
        }
    }

    /// `CloudFront` pricing for ap-northeast-1 (Japan edge).
    pub fn cloudfront_price(&self) -> CloudFrontPrice {
        CloudFrontPrice {
            request_price_per_10k: 0.0120,       // $0.0120 per 10K HTTPS requests
            data_transfer_price_per_gb: 0.114,   // first 10TB
            free_tier_data_transfer_gb: 1_000.0, // 1TB free/month
        }
    }

    /// `ElastiCache` pricing for ap-northeast-1.
    pub fn elasticache_price(&self, node_type: &str) -> Result<ElastiCachePrice, PricingError> {
        let hourly = match node_type {
            "cache.t3.micro" => 0.021,
            "cache.t3.small" => 0.042,
            "cache.t3.medium" => 0.084,
            "cache.r6g.large" => 0.202,
            "cache.r6g.xlarge" => 0.404,
            // Derived from AWS CDP ec-container ref (2 nodes x 730h = $383.98).
            "cache.r7g.large" => 0.263,
            "cache.m6g.large" => 0.178,
            "cache.m5.large" => 0.191, // 6.38 GiB (ap-northeast-1)
            "cache.m5.xlarge" => 0.382,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("ElastiCache:{node_type}"),
                    region: self.region.clone(),
                });
            }
        };
        Ok(ElastiCachePrice {
            node_type: node_type.to_string(),
            hourly_price: hourly,
        })
    }

    /// Step Functions pricing for ap-northeast-1.
    pub fn step_functions_price(&self) -> StepFunctionsPrice {
        StepFunctionsPrice {
            standard_transition_price: 0.000025, // $0.025 per 1K transitions
            express_request_price: 0.000001,     // per request
            express_duration_price_per_gb_second: 0.00001667,
            free_tier_transitions: 4_000.0, // 4K free/month
        }
    }

    /// `EventBridge` Scheduler pricing for ap-northeast-1.
    pub fn eventbridge_scheduler_price(&self) -> EventBridgeSchedulerPrice {
        EventBridgeSchedulerPrice {
            invocation_price: 0.00000106,        // ~$1.06 per million
            free_tier_invocations: 14_000_000.0, // 14M free/month
        }
    }

    /// Internet egress data transfer pricing for ap-northeast-1.
    pub fn data_transfer_price(&self) -> DataTransferPrice {
        DataTransferPrice {
            egress_tiers: vec![
                DataTransferTier {
                    upper_limit_gb: Some(1.0),
                    price_per_gb: 0.0,
                },
                DataTransferTier {
                    upper_limit_gb: Some(10_240.0),
                    price_per_gb: 0.114,
                },
                DataTransferTier {
                    upper_limit_gb: Some(51_200.0),
                    price_per_gb: 0.089,
                },
                DataTransferTier {
                    upper_limit_gb: Some(153_600.0),
                    price_per_gb: 0.086,
                },
                DataTransferTier {
                    upper_limit_gb: None,
                    price_per_gb: 0.084,
                },
            ],
        }
    }

    /// AWS Batch pricing for ap-northeast-1.
    pub fn batch_price(&self) -> BatchPrice {
        BatchPrice {
            fargate_vcpu_hour_price: 0.05056,
            fargate_memory_gb_hour_price: 0.00553,
            fargate_ephemeral_storage_gb_hour_price: 0.000111,
            fargate_ephemeral_free_gb: 20.0,
            ebs_gp3_gb_month_price: 0.096,
            ebs_gp3_iops_month_price: 0.006,
            ebs_gp3_iops_free: 3000.0,
            ebs_gp3_throughput_mibps_month_price: 0.048,
            ebs_gp3_throughput_free_mibps: 125.0,
        }
    }

    pub fn alb_price(&self) -> AlbPrice {
        AlbPrice {
            alb_hour_price: 0.0243,
            lcu_hour_price: 0.008,
        }
    }

    pub fn sns_price(&self) -> SnsPrice {
        SnsPrice {
            delivery_price_per_million: 0.50,
            free_tier_deliveries: 1_000_000.0,
        }
    }

    pub fn eks_price(&self) -> EksPrice {
        EksPrice {
            cluster_hour_price: 0.10,
        }
    }

    pub fn firehose_price(&self) -> FirehosePrice {
        FirehosePrice {
            ingestion_price_per_gb: 0.031,
        }
    }

    pub fn secrets_manager_price(&self) -> SecretsManagerPrice {
        SecretsManagerPrice {
            secret_month_price: 0.40,
            api_call_price_per_10k: 0.05,
        }
    }

    pub fn waf_price(&self) -> WafPrice {
        WafPrice {
            web_acl_month_price: 5.0,
            rule_month_price: 1.0,
            request_price_per_million: 0.60,
        }
    }

    pub fn efs_price(&self) -> EfsPrice {
        EfsPrice {
            standard_gb_month_price: 0.36,
            ia_gb_month_price: 0.0272,
            ia_access_price_per_gb: 0.01,
        }
    }

    pub fn eventbridge_price(&self) -> EventBridgePrice {
        EventBridgePrice {
            custom_event_price_per_million: 1.0,
        }
    }

    pub fn athena_price(&self) -> AthenaPrice {
        AthenaPrice {
            scan_price_per_tb: 5.0,
        }
    }

    /// Amazon Bedrock on-demand input token price per 1,000 tokens
    /// (Claude 3 / 3.5 Sonnet, ap-northeast-1; uniform across regions).
    pub fn bedrock_input_token_price_per_1k(&self) -> f64 {
        0.003
    }

    /// Amazon Bedrock on-demand output token price per 1,000 tokens
    /// (Claude 3 / 3.5 Sonnet, ap-northeast-1; uniform across regions).
    pub fn bedrock_output_token_price_per_1k(&self) -> f64 {
        0.015
    }

    pub fn ecr_price(&self) -> EcrPrice {
        EcrPrice {
            private_storage_gb_month: 0.10,
        }
    }

    pub fn appsync_price(&self) -> AppSyncPrice {
        AppSyncPrice {
            operation_price_per_million: 4.0,
            free_tier_operations: 250_000.0,
        }
    }

    pub fn cognito_price(&self) -> CognitoPrice {
        CognitoPrice {
            free_tier_mau: 50_000.0,
            tier1_price: 0.0055,  // per MAU up to 100K
            tier2_price: 0.0046,  // per MAU 100K–1M
            tier3_price: 0.00325, // per MAU over 1M
        }
    }

    pub fn route53_price(&self) -> Route53Price {
        Route53Price {
            hosted_zone_month_price: 0.50,
            query_price_per_million: 0.40,
        }
    }

    pub fn glue_price(&self) -> GluePrice {
        GluePrice {
            standard_dpu_hour_price: 0.44,
            flex_dpu_hour_price: 0.29,
        }
    }

    pub fn msk_broker_price(&self, broker_type: &str) -> Result<MskBrokerPrice, PricingError> {
        let hourly = match broker_type {
            "kafka.t3.small" => 0.0456,
            "kafka.m5.large" => 0.213,
            "kafka.m5.xlarge" => 0.425,
            "kafka.m5.2xlarge" => 0.850,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("MSK:{broker_type}"),
                    region: self.region.clone(),
                });
            }
        };
        Ok(MskBrokerPrice {
            hourly_price: hourly,
            storage_gb_month_price: 0.114,
        })
    }

    pub fn opensearch_service_price(
        &self,
        instance_type: &str,
    ) -> Result<OpenSearchServicePrice, PricingError> {
        let hourly = match instance_type {
            "t3.small.search" => 0.036,
            "t3.medium.search" => 0.073,
            "m5.large.search" => 0.182,
            "m5.xlarge.search" => 0.365,
            "r5.large.search" => 0.250,
            "r5.xlarge.search" => 0.501,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("OpenSearch:{instance_type}"),
                    region: self.region.clone(),
                });
            }
        };
        Ok(OpenSearchServicePrice {
            instance_hour_price: hourly,
            gp2_storage_gb_month_price: 0.135,
        })
    }

    pub fn documentdb_price(&self, instance_type: &str) -> Result<DocumentDbPrice, PricingError> {
        let hourly = match instance_type {
            "db.t3.medium" => 0.076,
            "db.r5.large" => 0.277,
            "db.r5.xlarge" => 0.554,
            "db.r5.2xlarge" => 1.108,
            "db.r6g.large" => 0.264,
            "db.r6g.xlarge" => 0.528,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("DocumentDB:{instance_type}"),
                    region: self.region.clone(),
                });
            }
        };
        Ok(DocumentDbPrice {
            instance_hour_price: hourly,
            storage_gb_month_price: Self::DOCUMENTDB_STORAGE_GB_MONTH_PRICE,
        })
    }

    pub fn documentdb_storage_price(&self) -> f64 {
        Self::DOCUMENTDB_STORAGE_GB_MONTH_PRICE
    }

    const DOCUMENTDB_STORAGE_GB_MONTH_PRICE: f64 = 0.110;

    /// Standalone Amazon EBS volume price per GB-month by volume type (ap-northeast-1).
    pub fn ebs_gb_month_price(&self, volume_type: &str) -> Result<f64, PricingError> {
        Ok(match volume_type {
            "gp3" => 0.096,
            "gp2" => 0.12,
            "st1" => 0.054,
            "sc1" => 0.018,
            "io1" | "io2" => 0.142,
            "standard" | "magnetic" => 0.10,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("EBS:{volume_type}"),
                    region: self.region.clone(),
                });
            }
        })
    }

    /// Amazon EBS snapshot storage price per GB-month (ap-northeast-1).
    pub fn ebs_snapshot_gb_month_price(&self) -> f64 {
        0.05
    }

    /// Site-to-Site VPN connection price per hour (ap-northeast-1).
    pub fn site_to_site_vpn_connection_hour_price(&self) -> f64 {
        0.048
    }

    /// `Redshift` managed/cluster storage price per GB-month (ap-northeast-1).
    pub fn redshift_storage_gb_month_price(&self) -> f64 {
        0.0261
    }

    /// `Redshift` Spectrum scan price per TB scanned (ap-northeast-1).
    pub fn redshift_spectrum_tb_scan_price(&self) -> f64 {
        5.00
    }

    pub fn redshift_price(&self, node_type: &str) -> Result<RedshiftPrice, PricingError> {
        let hourly = match node_type {
            "dc2.large" => 0.314,
            "dc2.8xlarge" => 5.024,
            "ra3.xlplus" => 1.086,
            "ra3.4xlarge" => 3.496,
            "ra3.16xlarge" => 13.985,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("Redshift:{node_type}"),
                    region: self.region.clone(),
                });
            }
        };
        Ok(RedshiftPrice {
            node_hour_price: hourly,
        })
    }

    /// `Lightsail` pricing for ap-northeast-1 (Japan).
    pub fn lightsail_price(&self) -> LightsailPrice {
        // nano_2_0 bundle: $3.43/month (512MB RAM, 1 vCPU, 20GB SSD, 1TB data transfer)
        // EBS disk: $0.10/GB/month (same as EC2 EBS gp2/gp3)
        LightsailPrice {
            instance_bundle_month_price: 3.43,
            disk_gb_month_price: 0.10,
        }
    }

    /// `Lightsail` Linux/Unix instance bundle price per month by bundle id
    /// (ap-northeast-1). The classic plan ladder; unknown bundles are
    /// unsupported (so a template's plan is never silently priced as nano).
    pub fn lightsail_bundle_month_price(&self, bundle_id: &str) -> Result<f64, PricingError> {
        Ok(match bundle_id {
            "nano_2_0" => 3.43,
            "micro_2_0" => 5.0,
            "small_2_0" => 10.0,
            "medium_2_0" => 20.0,
            "large_2_0" => 40.0,
            "xlarge_2_0" => 80.0,
            "2xlarge_2_0" => 160.0,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("Lightsail:bundle:{bundle_id}"),
                    region: self.region.clone(),
                });
            }
        })
    }

    /// `QuickSight` pricing for ap-northeast-1 (Japan).
    /// Reference: <https://aws.amazon.com/jp/quicksight/pricing/>
    pub fn quicksight_price(&self) -> QuickSightPrice {
        QuickSightPrice {
            // Creator: $24/user/month (monthly), $18/user/month (annual)
            creator_month_price: 24.0,
            creator_annual_month_price: 18.0,
            // Viewer on-demand: $0.30/session, max $5.00/user/month
            viewer_session_price: 0.30,
            viewer_max_month_price: 5.00,
            // SPICE capacity: $0.38/GB/month (first 10GB free per creator)
            spice_gb_month_price: 0.38,
            free_spice_gb: 10.0,
        }
    }
    // ----- aws.kendra -----
    /// Amazon Kendra Developer Edition index price per hour (ap-northeast-1).
    /// Billed continuously; multiply by 730h/mo at the service layer.
    pub fn kendra_index_hour_price(&self, edition: &str) -> Result<f64, PricingError> {
        Ok(match edition {
            "DEVELOPER_EDITION" => 1.125,
            "ENTERPRISE_EDITION" => 1.40,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("Kendra:{edition}"),
                    region: self.region.clone(),
                });
            }
        })
    }

    /// Amazon Kendra connector document-scan price per document (ap-northeast-1).
    /// $1.00 per 1M documents scanned = $0.000001/document.
    pub fn kendra_connector_scan_document_price(&self) -> f64 {
        0.000_001
    }

    /// Amazon Kendra connector sync (scan) compute price per hour (ap-northeast-1).
    pub fn kendra_connector_scan_hour_price(&self) -> f64 {
        0.35
    }

    // ----- aws.transcribe -----
    /// Amazon Transcribe standard batch transcription price per minute
    /// (ap-northeast-1, Tier 1: first 250K minutes/month).
    pub fn transcribe_standard_batch_price_per_minute(&self) -> f64 {
        0.024
    }

    // ----- aws.fsx_windows -----
    /// Amazon FSx for Windows File Server storage price per GB-month
    /// (ap-northeast-1), by storage type (`ssd`/`hdd`) and deployment option
    /// (`single_az`/`multi_az`).
    pub fn fsx_windows_storage_gb_month_price(
        &self,
        storage_type: &str,
        deployment: &str,
    ) -> Result<f64, PricingError> {
        Ok(match (storage_type, deployment) {
            ("ssd", "single_az") => 0.156,
            ("ssd", "multi_az") => 0.276,
            ("hdd", "single_az") => 0.016,
            ("hdd", "multi_az") => 0.030,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("FSxWindows:storage:{storage_type}:{deployment}"),
                    region: self.region.clone(),
                });
            }
        })
    }

    /// Amazon FSx for Windows File Server throughput capacity price per
    /// MBps-month (ap-northeast-1), by deployment option.
    pub fn fsx_windows_throughput_mbps_month_price(
        &self,
        deployment: &str,
    ) -> Result<f64, PricingError> {
        Ok(match deployment {
            "single_az" => 2.53,
            "multi_az" => 5.175,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("FSxWindows:throughput:{deployment}"),
                    region: self.region.clone(),
                });
            }
        })
    }

    /// Amazon FSx for Windows File Server backup storage price per GB-month
    /// (ap-northeast-1). Identical for Single-AZ and Multi-AZ.
    pub fn fsx_windows_backup_gb_month_price(&self) -> f64 {
        0.05
    }

    // ----- aws.directory_service -----
    /// AWS Directory Service for Microsoft Active Directory (AWS Managed
    /// Microsoft AD) price per domain-controller-hour, by edition
    /// (ap-northeast-1). Source: AWS Price List API (AWSDirectoryService,
    /// region ap-northeast-1) — usagetype `APN1-Std-MicrosoftAD-DC-Usage`
    /// (Standard) and `APN1-MicrosoftAD-DC-Usage` (Enterprise).
    pub fn directory_service_dc_hour_price(&self, edition: &str) -> Result<f64, PricingError> {
        Ok(match edition {
            "Standard" => 0.073,
            "Enterprise" => 0.2225,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("DirectoryService:MicrosoftAD:{edition}"),
                    region: self.region.clone(),
                });
            }
        })
    }

    // ----- aws.cloudwatch -----
    /// `CloudWatch` standard-resolution alarm price per alarm per month (ap-northeast-1).
    pub fn cloudwatch_alarm_month_price(&self) -> f64 {
        0.10
    }

    // ----- aws.guardduty -----
    /// Amazon GuardDuty pricing for ap-northeast-1 (Tokyo).
    /// Source: AWS Price List API, offer `AmazonGuardDuty`, region
    /// `ap-northeast-1` (published 2026-01-20).
    pub fn guardduty_price(&self) -> GuardDutyPrice {
        GuardDutyPrice {
            // $0.00000472 per CloudTrail event ($4.72 per 1M events).
            cloudtrail_event_price: 0.000_004_72,
            // VPC Flow Logs + DNS query logs, per GB-month (volume tiered).
            flowlog_dns_gb_tiers: vec![
                GuardDutyTier {
                    upper_limit_gb: Some(500.0), // first 500 GB
                    price_per_gb: 1.18,
                },
                GuardDutyTier {
                    upper_limit_gb: Some(2_500.0), // next 2,000 GB
                    price_per_gb: 0.59,
                },
                GuardDutyTier {
                    upper_limit_gb: Some(10_000.0), // next 7,500 GB
                    price_per_gb: 0.29,
                },
                GuardDutyTier {
                    upper_limit_gb: None, // over 10,000 GB
                    price_per_gb: 0.17,
                },
            ],
        }
    }

    // ----- aws.cloudtrail -----
    /// AWS CloudTrail data-event delivery price per 100,000 events (ap-northeast-1).
    /// Management-event delivery: first copy per region is free; additional
    /// copies are $2.00 per 100,000 events (uniform across regions).
    pub fn cloudtrail_data_event_price_per_100k(&self) -> f64 {
        0.10
    }

    /// AWS CloudTrail additional management-event copy price per 100,000 events.
    pub fn cloudtrail_management_event_copy_price_per_100k(&self) -> f64 {
        2.00
    }

    // ----- aws.backup -----
    /// AWS Backup warm (backup) storage price per GB-month by protected-resource
    /// engine (ap-northeast-1). Source: AWS Bulk Pricing API (AWSBackup,
    /// `APN1-WarmStorage-ByteHrs-*`) and the AWS Backup pricing page. EBS warm
    /// storage is billed at the EBS snapshot rate ($0.05/GB-mo); RDS at the RDS
    /// backup-storage rate ($0.095/GB-mo); Aurora at $0.024/GB-mo; EFS $0.06;
    /// DynamoDB $0.114.
    pub fn backup_warm_storage_gb_month_price(&self, engine: &str) -> Result<f64, PricingError> {
        Ok(match engine {
            "ebs" => 0.05,
            "efs" => 0.06,
            "rds" => 0.095,
            "aurora" => 0.024,
            "dynamodb" => 0.114,
            _ => {
                return Err(PricingError::NotFound {
                    service: format!("Backup:{engine}"),
                    region: self.region.clone(),
                });
            }
        })
    }

    /// Standalone inter-region data transfer price per GB (ap-northeast-1 ->
    /// e.g. ap-northeast-3 Osaka). Source: AWS data transfer pricing.
    pub fn data_transfer_inter_region_price_per_gb(&self) -> f64 {
        0.09
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ec2_price_lookup() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("t3.micro").unwrap();
        assert_eq!(price.hourly_price, 0.0136);
    }

    #[test]
    fn test_ec2_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.ec2_price("z99.metal").is_err());
    }

    #[test]
    fn test_lambda_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.lambda_price();
        assert_eq!(price.free_tier_requests, 1_000_000.0);
    }

    #[test]
    fn test_rds_price_lookup() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.rds_price("db.t3.micro", "mysql").unwrap();
        assert_eq!(price.hourly_price, 0.026);
    }

    #[test]
    fn test_data_transfer_price() {
        let registry = PricingRegistry::new("ap-northeast-1");
        let price = registry.data_transfer_price();
        assert!(!price.egress_tiers.is_empty());
        // First GB is free
        assert_eq!(price.egress_tiers[0].price_per_gb, 0.0);
    }

    // -----------------------------------------------------------------------
    // Tests for EC2 Windows pricing (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_ec2_windows_price_m5_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ec2_windows_hourly_price("m5.large").unwrap(), 0.216);
    }

    #[test]
    fn test_ec2_windows_price_m5_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ec2_windows_hourly_price("m5.xlarge").unwrap(), 0.432);
    }

    #[test]
    fn test_ec2_windows_price_m5_2xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ec2_windows_hourly_price("m5.2xlarge").unwrap(), 0.864);
    }

    #[test]
    fn test_ec2_windows_price_m6a_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ec2_windows_hourly_price("m6a.large").unwrap(), 0.2036);
    }

    #[test]
    fn test_ec2_windows_price_m6a_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ec2_windows_hourly_price("m6a.xlarge").unwrap(), 0.4072);
    }

    #[test]
    fn test_ec2_windows_price_m6a_2xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ec2_windows_hourly_price("m6a.2xlarge").unwrap(), 0.8144);
    }

    #[test]
    fn test_ec2_windows_price_m6i_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ec2_windows_hourly_price("m6i.large").unwrap(), 0.216);
    }

    #[test]
    fn test_ec2_windows_price_m6i_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ec2_windows_hourly_price("m6i.xlarge").unwrap(), 0.432);
    }

    #[test]
    fn test_ec2_windows_price_t3_medium() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ec2_windows_hourly_price("t3.medium").unwrap(), 0.1464);
    }

    #[test]
    fn test_ec2_windows_price_t3_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ec2_windows_hourly_price("t3.large").unwrap(), 0.2008);
    }

    #[test]
    fn test_ec2_windows_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.ec2_windows_hourly_price("unknown.instance").is_err());
    }

    // -----------------------------------------------------------------------
    // Tests for Lambda streaming price (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_lambda_http_stream_gb_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.lambda_http_stream_gb_price(), 0.008);
    }

    // -----------------------------------------------------------------------
    // Tests for RDS pricing (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_rds_price_mysql_db_m6i_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.rds_price("db.m6i.large", "mysql").unwrap();
        assert_eq!(price.hourly_price, 0.235);
    }

    #[test]
    fn test_rds_price_postgres_db_m6i_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.rds_price("db.m6i.large", "postgres").unwrap();
        assert_eq!(price.hourly_price, 0.235);
    }

    #[test]
    fn test_rds_price_sqlserver_se_db_r5_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.rds_price("db.r5.large", "sqlserver-se").unwrap();
        assert_eq!(price.hourly_price, 1.050);
    }

    #[test]
    fn test_rds_price_sqlserver_se_db_r5_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.rds_price("db.r5.xlarge", "sqlserver-se").unwrap();
        assert_eq!(price.hourly_price, 2.100);
    }

    #[test]
    fn test_rds_price_sqlserver_se_db_t3_medium() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.rds_price("db.t3.medium", "sqlserver-se").unwrap();
        assert_eq!(price.hourly_price, 0.342);
    }

    #[test]
    fn test_rds_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.rds_price("db.unknown", "unknown").is_err());
    }

    // -----------------------------------------------------------------------
    // Tests for CloudWatch custom metric price (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_cloudwatch_custom_metric_month_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.cloudwatch_custom_metric_month_price(), 0.30);
    }

    // -----------------------------------------------------------------------
    // Tests for ElastiCache pricing (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_elasticache_price_cache_r7g_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.elasticache_price("cache.r7g.large").unwrap();
        assert_eq!(price.hourly_price, 0.263);
    }

    #[test]
    fn test_elasticache_price_cache_m5_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.elasticache_price("cache.m5.large").unwrap();
        assert_eq!(price.hourly_price, 0.191);
    }

    #[test]
    fn test_elasticache_price_cache_m5_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.elasticache_price("cache.m5.xlarge").unwrap();
        assert_eq!(price.hourly_price, 0.382);
    }

    #[test]
    fn test_elasticache_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.elasticache_price("cache.unknown").is_err());
    }

    // -----------------------------------------------------------------------
    // Tests for Bedrock token prices (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_bedrock_input_token_price_per_1k() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.bedrock_input_token_price_per_1k(), 0.003);
    }

    #[test]
    fn test_bedrock_output_token_price_per_1k() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.bedrock_output_token_price_per_1k(), 0.015);
    }

    // -----------------------------------------------------------------------
    // Tests for EBS pricing (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_ebs_gb_month_price_gp3() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ebs_gb_month_price("gp3").unwrap(), 0.096);
    }

    #[test]
    fn test_ebs_gb_month_price_gp2() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ebs_gb_month_price("gp2").unwrap(), 0.12);
    }

    #[test]
    fn test_ebs_gb_month_price_st1() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ebs_gb_month_price("st1").unwrap(), 0.054);
    }

    #[test]
    fn test_ebs_gb_month_price_sc1() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ebs_gb_month_price("sc1").unwrap(), 0.018);
    }

    #[test]
    fn test_ebs_gb_month_price_io1() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ebs_gb_month_price("io1").unwrap(), 0.142);
    }

    #[test]
    fn test_ebs_gb_month_price_io2() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ebs_gb_month_price("io2").unwrap(), 0.142);
    }

    #[test]
    fn test_ebs_gb_month_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.ebs_gb_month_price("unknown").is_err());
    }

    // -----------------------------------------------------------------------
    // Tests for EC2 pricing (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_ec2_price_m6a_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("m6a.large").unwrap();
        assert_eq!(price.hourly_price, 0.1116);
    }

    #[test]
    fn test_ec2_price_m6a_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("m6a.xlarge").unwrap();
        assert_eq!(price.hourly_price, 0.2232);
    }

    #[test]
    fn test_ec2_price_m6a_2xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("m6a.2xlarge").unwrap();
        assert_eq!(price.hourly_price, 0.4464);
    }

    // -----------------------------------------------------------------------
    // Tests for MSK broker pricing (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_msk_broker_price_kafka_t3_small() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.msk_broker_price("kafka.t3.small").unwrap();
        assert_eq!(price.hourly_price, 0.0456);
    }

    #[test]
    fn test_msk_broker_price_kafka_m5_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.msk_broker_price("kafka.m5.large").unwrap();
        assert_eq!(price.hourly_price, 0.213);
    }

    #[test]
    fn test_msk_broker_price_kafka_m5_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.msk_broker_price("kafka.m5.xlarge").unwrap();
        assert_eq!(price.hourly_price, 0.425);
    }

    #[test]
    fn test_msk_broker_price_kafka_m5_2xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.msk_broker_price("kafka.m5.2xlarge").unwrap();
        assert_eq!(price.hourly_price, 0.850);
    }

    #[test]
    fn test_msk_broker_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.msk_broker_price("kafka.unknown").is_err());
    }

    // -----------------------------------------------------------------------
    // Tests for OpenSearch Service pricing (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_opensearch_service_price_t3_small() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.opensearch_service_price("t3.small.search").unwrap();
        assert_eq!(price.instance_hour_price, 0.036);
    }

    #[test]
    fn test_opensearch_service_price_t3_medium() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.opensearch_service_price("t3.medium.search").unwrap();
        assert_eq!(price.instance_hour_price, 0.073);
    }

    #[test]
    fn test_opensearch_service_price_m5_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.opensearch_service_price("m5.large.search").unwrap();
        assert_eq!(price.instance_hour_price, 0.182);
    }

    #[test]
    fn test_opensearch_service_price_m5_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.opensearch_service_price("m5.xlarge.search").unwrap();
        assert_eq!(price.instance_hour_price, 0.365);
    }

    #[test]
    fn test_opensearch_service_price_r5_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.opensearch_service_price("r5.large.search").unwrap();
        assert_eq!(price.instance_hour_price, 0.250);
    }

    #[test]
    fn test_opensearch_service_price_r5_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.opensearch_service_price("r5.xlarge.search").unwrap();
        assert_eq!(price.instance_hour_price, 0.501);
    }

    #[test]
    fn test_opensearch_service_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.opensearch_service_price("unknown").is_err());
    }

    // -----------------------------------------------------------------------
    // Tests for DocumentDB pricing (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_documentdb_price_db_t3_medium() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.documentdb_price("db.t3.medium").unwrap();
        assert_eq!(price.instance_hour_price, 0.076);
    }

    #[test]
    fn test_documentdb_price_db_r5_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.documentdb_price("db.r5.large").unwrap();
        assert_eq!(price.instance_hour_price, 0.277);
    }

    #[test]
    fn test_documentdb_price_db_r5_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.documentdb_price("db.r5.xlarge").unwrap();
        assert_eq!(price.instance_hour_price, 0.554);
    }

    #[test]
    fn test_documentdb_price_db_r5_2xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.documentdb_price("db.r5.2xlarge").unwrap();
        assert_eq!(price.instance_hour_price, 1.108);
    }

    #[test]
    fn test_documentdb_price_db_r6g_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.documentdb_price("db.r6g.large").unwrap();
        assert_eq!(price.instance_hour_price, 0.264);
    }

    #[test]
    fn test_documentdb_price_db_r6g_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.documentdb_price("db.r6g.xlarge").unwrap();
        assert_eq!(price.instance_hour_price, 0.528);
    }

    #[test]
    fn test_documentdb_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.documentdb_price("unknown").is_err());
    }

    // -----------------------------------------------------------------------
    // Tests for Redshift pricing (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_redshift_price_dc2_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.redshift_price("dc2.large").unwrap();
        assert_eq!(price.node_hour_price, 0.314);
    }

    #[test]
    fn test_redshift_price_dc2_8xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.redshift_price("dc2.8xlarge").unwrap();
        assert_eq!(price.node_hour_price, 5.024);
    }

    #[test]
    fn test_redshift_price_ra3_xlplus() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.redshift_price("ra3.xlplus").unwrap();
        assert_eq!(price.node_hour_price, 1.086);
    }

    #[test]
    fn test_redshift_price_ra3_4xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.redshift_price("ra3.4xlarge").unwrap();
        assert_eq!(price.node_hour_price, 3.496);
    }

    #[test]
    fn test_redshift_price_ra3_16xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.redshift_price("ra3.16xlarge").unwrap();
        assert_eq!(price.node_hour_price, 13.985);
    }

    #[test]
    fn test_redshift_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.redshift_price("unknown").is_err());
    }

    // -----------------------------------------------------------------------
    // Tests for Lightsail bundle pricing (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_lightsail_bundle_nano_2_0() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.lightsail_bundle_month_price("nano_2_0").unwrap(), 3.43);
    }

    #[test]
    fn test_lightsail_bundle_micro_2_0() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.lightsail_bundle_month_price("micro_2_0").unwrap(), 5.0);
    }

    #[test]
    fn test_lightsail_bundle_small_2_0() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.lightsail_bundle_month_price("small_2_0").unwrap(), 10.0);
    }

    #[test]
    fn test_lightsail_bundle_medium_2_0() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.lightsail_bundle_month_price("medium_2_0").unwrap(),
            20.0
        );
    }

    #[test]
    fn test_lightsail_bundle_large_2_0() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.lightsail_bundle_month_price("large_2_0").unwrap(), 40.0);
    }

    #[test]
    fn test_lightsail_bundle_xlarge_2_0() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.lightsail_bundle_month_price("xlarge_2_0").unwrap(),
            80.0
        );
    }

    #[test]
    fn test_lightsail_bundle_2xlarge_2_0() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.lightsail_bundle_month_price("2xlarge_2_0").unwrap(),
            160.0
        );
    }

    #[test]
    fn test_lightsail_bundle_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.lightsail_bundle_month_price("unknown").is_err());
    }

    // -----------------------------------------------------------------------
    // Tests for Lightsail price (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_lightsail_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.lightsail_price();
        assert_eq!(price.instance_bundle_month_price, 3.43);
        assert_eq!(price.disk_gb_month_price, 0.10);
    }

    // -----------------------------------------------------------------------
    // Tests for various constant-price methods (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_rds_gp3_storage_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.rds_gp3_storage_price(), 0.1216);
    }

    #[test]
    fn test_rds_gp3_iops_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.rds_gp3_iops_price(), 0.008);
    }

    #[test]
    fn test_ebs_snapshot_gb_month_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.ebs_snapshot_gb_month_price(), 0.05);
    }

    #[test]
    fn test_site_to_site_vpn_connection_hour_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.site_to_site_vpn_connection_hour_price(), 0.048);
    }

    #[test]
    fn test_redshift_storage_gb_month_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.redshift_storage_gb_month_price(), 0.0261);
    }

    #[test]
    fn test_redshift_spectrum_tb_scan_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.redshift_spectrum_tb_scan_price(), 5.00);
    }

    #[test]
    fn test_documentdb_storage_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.documentdb_storage_price(), 0.110);
    }

    // -----------------------------------------------------------------------
    // Tests for various service prices (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_sqs_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.sqs_price();
        assert_eq!(price.standard_request_price, 0.0000004);
        assert_eq!(price.fifo_request_price, 0.0000005);
    }

    #[test]
    fn test_cloudwatch_logs_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.cloudwatch_logs_price();
        assert_eq!(price.ingestion_price_per_gb, 0.76);
        assert_eq!(price.storage_price_per_gb, 0.033);
    }

    #[test]
    fn test_api_gateway_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.api_gateway_price();
        assert_eq!(price.rest_api_request_price, 0.00000435);
        assert_eq!(price.http_api_request_price, 0.00000120);
    }

    #[test]
    fn test_nat_gateway_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.nat_gateway_price();
        assert_eq!(price.hourly_price, 0.062);
        assert_eq!(price.data_processing_price_per_gb, 0.062);
    }

    #[test]
    fn test_cloudfront_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.cloudfront_price();
        assert_eq!(price.request_price_per_10k, 0.0120);
        assert_eq!(price.data_transfer_price_per_gb, 0.114);
    }

    #[test]
    fn test_step_functions_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.step_functions_price();
        assert_eq!(price.standard_transition_price, 0.000025);
    }

    #[test]
    fn test_eventbridge_scheduler_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.eventbridge_scheduler_price();
        assert_eq!(price.invocation_price, 0.00000106);
    }

    #[test]
    fn test_batch_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.batch_price();
        assert_eq!(price.fargate_vcpu_hour_price, 0.05056);
        assert_eq!(price.fargate_memory_gb_hour_price, 0.00553);
    }

    #[test]
    fn test_alb_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.alb_price();
        assert_eq!(price.alb_hour_price, 0.0243);
        assert_eq!(price.lcu_hour_price, 0.008);
    }

    #[test]
    fn test_sns_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.sns_price();
        assert_eq!(price.delivery_price_per_million, 0.50);
    }

    #[test]
    fn test_eks_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.eks_price();
        assert_eq!(price.cluster_hour_price, 0.10);
    }

    #[test]
    fn test_firehose_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.firehose_price();
        assert_eq!(price.ingestion_price_per_gb, 0.031);
    }

    #[test]
    fn test_secrets_manager_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.secrets_manager_price();
        assert_eq!(price.secret_month_price, 0.40);
    }

    #[test]
    fn test_waf_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.waf_price();
        assert_eq!(price.web_acl_month_price, 5.0);
    }

    #[test]
    fn test_efs_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.efs_price();
        assert_eq!(price.standard_gb_month_price, 0.36);
    }

    #[test]
    fn test_eventbridge_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.eventbridge_price();
        assert_eq!(price.custom_event_price_per_million, 1.0);
    }

    #[test]
    fn test_athena_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.athena_price();
        assert_eq!(price.scan_price_per_tb, 5.0);
    }

    #[test]
    fn test_ecr_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ecr_price();
        assert_eq!(price.private_storage_gb_month, 0.10);
    }

    #[test]
    fn test_appsync_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.appsync_price();
        assert_eq!(price.operation_price_per_million, 4.0);
    }

    #[test]
    fn test_cognito_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.cognito_price();
        assert_eq!(price.free_tier_mau, 50_000.0);
        assert_eq!(price.tier1_price, 0.0055);
    }

    #[test]
    fn test_route53_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.route53_price();
        assert_eq!(price.hosted_zone_month_price, 0.50);
    }

    #[test]
    fn test_glue_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.glue_price();
        assert_eq!(price.standard_dpu_hour_price, 0.44);
        assert_eq!(price.flex_dpu_hour_price, 0.29);
    }

    // -----------------------------------------------------------------------
    // Tests for Kinesis pricing (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_kinesis_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.kinesis_price();
        assert_eq!(price.shard_hour_price, 0.0195);
        assert_eq!(price.put_payload_unit_price, 0.0000002);
        assert_eq!(price.on_demand_ingestion_price_per_gb, 0.098);
    }

    // -----------------------------------------------------------------------
    // Tests for Fargate pricing (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_fargate_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.fargate_price();
        assert_eq!(price.vcpu_hour_price, 0.05056);
        assert_eq!(price.memory_gb_hour_price, 0.00553);
    }

    // -----------------------------------------------------------------------
    // Tests for OpenSearch Serverless pricing (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_opensearch_serverless_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.opensearch_serverless_price();
        assert_eq!(price.ocu_hour_price, 0.334);
        assert_eq!(price.storage_price_per_gb, 0.026);
    }

    // -----------------------------------------------------------------------
    // Tests for DynamoDB pricing (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_dynamodb_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.dynamodb_price();
        assert_eq!(price.write_request_price, 0.000000715);
        assert_eq!(price.read_request_price, 0.000000143);
        assert_eq!(price.storage_price_per_gb, 0.285);
    }

    // -----------------------------------------------------------------------
    // Tests for S3 pricing (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_s3_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.s3_price();
        assert_eq!(price.put_request_price, 0.0000047);
        assert_eq!(price.get_request_price, 0.00000037);
        assert_eq!(price.storage_tiers[0].price_per_gb, 0.025);
    }

    // -----------------------------------------------------------------------
    // Tests for Lambda pricing (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_lambda_price_values() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.lambda_price();
        assert_eq!(price.request_price, 0.0000002);
        assert_eq!(price.gb_second_price, 0.0000166667);
        assert_eq!(price.free_tier_requests, 1_000_000.0);
        assert_eq!(price.free_tier_gb_seconds, 400_000.0);
    }

    // -----------------------------------------------------------------------
    // Tests for RDS pricing with various engines (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_rds_price_aurora_mysql() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.rds_price("db.r5.large", "aurora-mysql").unwrap();
        assert_eq!(price.hourly_price, 0.350);
    }

    #[test]
    fn test_rds_price_aurora_postgresql() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.rds_price("db.r5.xlarge", "aurora-postgresql").unwrap();
        assert_eq!(price.hourly_price, 0.700);
    }

    // -----------------------------------------------------------------------
    // Tests for EC2 pricing with various instance types (catches match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_ec2_price_t3_nano() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("t3.nano").unwrap();
        assert_eq!(price.hourly_price, 0.0068);
    }

    #[test]
    fn test_ec2_price_t3_small() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("t3.small").unwrap();
        assert_eq!(price.hourly_price, 0.0272);
    }

    #[test]
    fn test_ec2_price_t3_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("t3.large").unwrap();
        assert_eq!(price.hourly_price, 0.1088);
    }

    #[test]
    fn test_ec2_price_m5_4xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("m5.4xlarge").unwrap();
        assert_eq!(price.hourly_price, 0.992);
    }

    #[test]
    fn test_ec2_price_c5_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("c5.large").unwrap();
        assert_eq!(price.hourly_price, 0.107);
    }

    #[test]
    fn test_ec2_price_c5_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("c5.xlarge").unwrap();
        assert_eq!(price.hourly_price, 0.214);
    }

    #[test]
    fn test_ec2_price_c5_2xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("c5.2xlarge").unwrap();
        assert_eq!(price.hourly_price, 0.428);
    }

    #[test]
    fn test_ec2_price_r5_large() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("r5.large").unwrap();
        assert_eq!(price.hourly_price, 0.152);
    }

    #[test]
    fn test_ec2_price_r5_xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("r5.xlarge").unwrap();
        assert_eq!(price.hourly_price, 0.304);
    }

    #[test]
    fn test_ec2_price_r5_2xlarge() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.ec2_price("r5.2xlarge").unwrap();
        assert_eq!(price.hourly_price, 0.608);
    }

    // -----------------------------------------------------------------------
    // Tests for Kendra pricing (catches return value and match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_kendra_index_hour_price_developer_edition() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.kendra_index_hour_price("DEVELOPER_EDITION").unwrap(),
            1.125
        );
    }

    #[test]
    fn test_kendra_index_hour_price_enterprise_edition() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.kendra_index_hour_price("ENTERPRISE_EDITION").unwrap(),
            1.40
        );
    }

    #[test]
    fn test_kendra_index_hour_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.kendra_index_hour_price("UNKNOWN_EDITION").is_err());
    }

    #[test]
    fn test_kendra_connector_scan_document_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.kendra_connector_scan_document_price(), 0.000_001);
    }

    #[test]
    fn test_kendra_connector_scan_hour_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.kendra_connector_scan_hour_price(), 0.35);
    }

    // -----------------------------------------------------------------------
    // Tests for FSx for Windows pricing (catches return value and match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_fsx_windows_storage_gb_month_price_ssd_single_az() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.fsx_windows_storage_gb_month_price("ssd", "single_az")
                .unwrap(),
            0.156
        );
    }

    #[test]
    fn test_fsx_windows_storage_gb_month_price_ssd_multi_az() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.fsx_windows_storage_gb_month_price("ssd", "multi_az")
                .unwrap(),
            0.276
        );
    }

    #[test]
    fn test_fsx_windows_storage_gb_month_price_hdd_single_az() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.fsx_windows_storage_gb_month_price("hdd", "single_az")
                .unwrap(),
            0.016
        );
    }

    #[test]
    fn test_fsx_windows_storage_gb_month_price_hdd_multi_az() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.fsx_windows_storage_gb_month_price("hdd", "multi_az")
                .unwrap(),
            0.030
        );
    }

    #[test]
    fn test_fsx_windows_storage_gb_month_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(
            reg.fsx_windows_storage_gb_month_price("unknown", "single_az")
                .is_err()
        );
    }

    #[test]
    fn test_fsx_windows_throughput_mbps_month_price_single_az() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.fsx_windows_throughput_mbps_month_price("single_az")
                .unwrap(),
            2.53
        );
    }

    #[test]
    fn test_fsx_windows_throughput_mbps_month_price_multi_az() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.fsx_windows_throughput_mbps_month_price("multi_az")
                .unwrap(),
            5.175
        );
    }

    #[test]
    fn test_fsx_windows_throughput_mbps_month_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(
            reg.fsx_windows_throughput_mbps_month_price("unknown")
                .is_err()
        );
    }

    #[test]
    fn test_fsx_windows_backup_gb_month_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.fsx_windows_backup_gb_month_price(), 0.05);
    }

    // -----------------------------------------------------------------------
    // Tests for Directory Service pricing (catches return value and match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_directory_service_dc_hour_price_standard() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.directory_service_dc_hour_price("Standard").unwrap(),
            0.073
        );
    }

    #[test]
    fn test_directory_service_dc_hour_price_enterprise() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.directory_service_dc_hour_price("Enterprise").unwrap(),
            0.2225
        );
    }

    #[test]
    fn test_directory_service_dc_hour_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.directory_service_dc_hour_price("Unknown").is_err());
    }

    // -----------------------------------------------------------------------
    // Tests for CloudWatch alarm price (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_cloudwatch_alarm_month_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.cloudwatch_alarm_month_price(), 0.10);
    }

    // -----------------------------------------------------------------------
    // Tests for CloudTrail pricing (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_cloudtrail_data_event_price_per_100k() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.cloudtrail_data_event_price_per_100k(), 0.10);
    }

    #[test]
    fn test_cloudtrail_management_event_copy_price_per_100k() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.cloudtrail_management_event_copy_price_per_100k(), 2.00);
    }

    // -----------------------------------------------------------------------
    // Tests for Backup warm storage pricing (catches return value and match arm deletion mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_backup_warm_storage_gb_month_price_ebs() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.backup_warm_storage_gb_month_price("ebs").unwrap(), 0.05);
    }

    #[test]
    fn test_backup_warm_storage_gb_month_price_efs() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.backup_warm_storage_gb_month_price("efs").unwrap(), 0.06);
    }

    #[test]
    fn test_backup_warm_storage_gb_month_price_rds() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.backup_warm_storage_gb_month_price("rds").unwrap(),
            0.095
        );
    }

    #[test]
    fn test_backup_warm_storage_gb_month_price_aurora() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.backup_warm_storage_gb_month_price("aurora").unwrap(),
            0.024
        );
    }

    #[test]
    fn test_backup_warm_storage_gb_month_price_dynamodb() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(
            reg.backup_warm_storage_gb_month_price("dynamodb").unwrap(),
            0.114
        );
    }

    #[test]
    fn test_backup_warm_storage_gb_month_price_not_found() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert!(reg.backup_warm_storage_gb_month_price("unknown").is_err());
    }

    // -----------------------------------------------------------------------
    // Tests for inter-region data transfer price (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_data_transfer_inter_region_price_per_gb() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.data_transfer_inter_region_price_per_gb(), 0.09);
    }

    // -----------------------------------------------------------------------
    // Tests for GuardDuty pricing (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_guardduty_cloudtrail_event_price() {
        let reg = PricingRegistry::new("ap-northeast-1");
        let price = reg.guardduty_price();
        assert_eq!(price.cloudtrail_event_price, 0.000_004_72);
        assert_eq!(price.flowlog_dns_gb_tiers.len(), 4);
        assert_eq!(price.flowlog_dns_gb_tiers[0].price_per_gb, 1.18);
    }

    // -----------------------------------------------------------------------
    // Tests for transcribe standard batch price (catches return value mutants)
    // -----------------------------------------------------------------------

    #[test]
    fn test_transcribe_standard_batch_price_per_minute() {
        let reg = PricingRegistry::new("ap-northeast-1");
        assert_eq!(reg.transcribe_standard_batch_price_per_minute(), 0.024);
    }
}
