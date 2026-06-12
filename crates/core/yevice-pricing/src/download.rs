//! Downloading AWS bulk pricing data over HTTP.
//!
//! The AWS Price List Bulk API publishes one JSON document per service and
//! region. [`pricing_url`] builds the document URL and [`download_pricing`]
//! fetches it; callers decide where to store the bytes.

use std::time::Duration;

use thiserror::Error;

/// AWS services to download pricing for, as `(service_code, file_stem)` pairs.
pub const PRICING_SERVICES: &[(&str, &str)] = &[
    ("AmazonEC2", "ec2"),
    ("AWSLambda", "lambda"),
    ("AmazonRDS", "rds"),
    ("AmazonS3", "s3"),
    ("AmazonDynamoDB", "dynamodb"),
    ("AmazonECS", "ecs"),
    ("AmazonES", "opensearch"), // OpenSearch uses the old ES pricing code
    ("AmazonKinesis", "kinesis"),
    ("AWSQueueService", "sqs"),
    ("AmazonCloudWatch", "cloudwatch"),
];

/// Maximum accepted response body size (OOM guard for huge pricing files).
pub const MAX_PRICING_BODY_BYTES: u64 = 256 * 1024 * 1024; // 256 MiB

/// Per-request timeout for pricing downloads.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Errors raised while downloading pricing data.
#[derive(Debug, Error)]
pub enum DownloadError {
    /// The HTTP request itself failed (connection, status, timeout, ...).
    #[error("HTTP request failed")]
    Request(#[source] Box<ureq::Error>),

    /// The response body could not be read (or exceeded
    /// [`MAX_PRICING_BODY_BYTES`]).
    #[error("failed to read response body")]
    Body(#[source] Box<ureq::Error>),
}

/// Build the AWS Price List Bulk API URL for a service/region pair.
pub fn pricing_url(service_code: &str, region_code: &str) -> String {
    format!(
        "https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/{service_code}/current/{region_code}/index.json"
    )
}

/// Download a pricing document and return its raw bytes.
pub fn download_pricing(url: &str) -> Result<Vec<u8>, DownloadError> {
    let mut response = ureq::get(url)
        .config()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .call()
        .map_err(|e| DownloadError::Request(Box::new(e)))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_PRICING_BODY_BYTES)
        .read_to_vec()
        .map_err(|e| DownloadError::Body(Box::new(e)))?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pricing_url_embeds_service_and_region() {
        let url = pricing_url("AmazonEC2", "ap-northeast-1");
        assert_eq!(
            url,
            "https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AmazonEC2/current/ap-northeast-1/index.json"
        );
    }

    #[test]
    fn pricing_services_have_unique_file_stems() {
        let mut stems: Vec<&str> = PRICING_SERVICES.iter().map(|(_, stem)| *stem).collect();
        stems.sort_unstable();
        let len_before = stems.len();
        stems.dedup();
        assert_eq!(
            len_before,
            stems.len(),
            "duplicate file stems would clobber output files"
        );
    }
}
