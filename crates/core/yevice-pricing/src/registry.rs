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
            request_price: 0.0000002,        // $0.20 per 1M requests
            gb_second_price: 0.0000166667,   // per GB-second
            free_tier_requests: 1_000_000.0, // 1M free requests/month
            free_tier_gb_seconds: 400_000.0, // 400K GB-seconds/month
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
            write_request_price: 0.000000715, // per WRU ($1.4269 per million)
            read_request_price: 0.000000143,  // per RRU ($0.2854 per million)
            // Provisioned
            wcu_hour_price: 0.000742, // $0.000742 per WCU-hour ($0.5417 per WCU-month)
            rcu_hour_price: 0.0001484, // $0.0001484 per RCU-hour ($0.1083 per RCU-month)
            // Common
            storage_price_per_gb: 0.285, // per GB-month
            free_tier_wru: 25_000.0,     // 25K WCU (equivalent)
            free_tier_rru: 25_000.0,     // 25K RCU (equivalent)
            free_tier_storage_gb: 25.0,  // 25 GB
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

    /// Kinesis Data Streams pricing for ap-northeast-1.
    pub fn kinesis_price(&self) -> KinesisPrice {
        KinesisPrice {
            // Provisioned mode
            shard_hour_price: 0.0195,          // per shard hour
            put_payload_unit_price: 0.0000002, // per 25KB payload unit
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
    /// Reference: https://aws.amazon.com/jp/quicksight/pricing/
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
}
