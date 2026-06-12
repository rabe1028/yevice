//! Parser for AWS Bulk Pricing API JSON files.
//!
//! The JSON structure:
//! ```json
//! {
//!   "products": { "<sku>": { "sku": "...", "productFamily": "...", "attributes": { ... } } },
//!   "terms": { "OnDemand": { "<sku>": { "<sku>.<offerTermCode>": { "priceDimensions": { ... } } } } }
//! }
//! ```

use std::collections::HashMap;

use serde::Deserialize;

use crate::error::PricingError;

/// Top-level structure of the AWS Bulk Pricing JSON.
#[derive(Debug, Deserialize)]
pub struct BulkPricingFile {
    #[serde(rename = "offerCode")]
    pub offer_code: String,
    pub products: HashMap<String, BulkProduct>,
    pub terms: BulkTerms,
}

#[derive(Debug, Deserialize)]
pub struct BulkProduct {
    pub sku: String,
    #[serde(rename = "productFamily", default)]
    pub product_family: String,
    #[serde(default)]
    pub attributes: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct BulkTerms {
    #[serde(rename = "OnDemand", default)]
    pub on_demand: HashMap<String, HashMap<String, BulkOfferTerm>>,
}

#[derive(Debug, Deserialize)]
pub struct BulkOfferTerm {
    pub sku: String,
    #[serde(rename = "priceDimensions")]
    pub price_dimensions: HashMap<String, BulkPriceDimension>,
}

#[derive(Debug, Deserialize)]
pub struct BulkPriceDimension {
    #[serde(default)]
    pub description: String,
    #[serde(rename = "beginRange", default)]
    pub begin_range: String,
    #[serde(rename = "endRange", default)]
    pub end_range: String,
    #[serde(default)]
    pub unit: String,
    #[serde(rename = "pricePerUnit")]
    pub price_per_unit: HashMap<String, String>,
}

impl BulkPriceDimension {
    pub fn usd_price(&self) -> f64 {
        match self.price_per_unit.get("USD") {
            None => {
                tracing::warn!(
                    description = %self.description,
                    "no USD price for pricing dimension; defaulting to 0.0"
                );
                0.0
            }
            Some(s) => match s.parse::<f64>() {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        description = %self.description,
                        raw = %s,
                        error = %e,
                        "failed to parse USD price; defaulting to 0.0"
                    );
                    0.0
                }
            },
        }
    }
}

/// A simplified, lookup-friendly pricing entry extracted from bulk data.
#[derive(Debug, Clone)]
pub struct PricingEntry {
    pub sku: String,
    pub product_family: String,
    pub attributes: HashMap<String, String>,
    pub dimensions: Vec<PricingDimension>,
}

#[derive(Debug, Clone)]
pub struct PricingDimension {
    pub description: String,
    pub unit: String,
    pub price_usd: f64,
    pub begin_range: f64,
    pub end_range: Option<f64>,
}

/// Parse a Bulk Pricing JSON file and extract simplified pricing entries.
pub fn parse_bulk_pricing(json_data: &[u8]) -> Result<Vec<PricingEntry>, PricingError> {
    let file: BulkPricingFile =
        serde_json::from_slice(json_data).map_err(|e| PricingError::ParseError(e.to_string()))?;

    let mut entries = Vec::new();

    for (sku, product) in &file.products {
        let term_map = file.terms.on_demand.get(sku);
        let mut dimensions = Vec::new();

        if let Some(offers) = term_map {
            for offer in offers.values() {
                for dim in offer.price_dimensions.values() {
                    let begin: f64 = dim.begin_range.parse().unwrap_or_else(|e| {
                        tracing::warn!(
                            description = %dim.description,
                            raw = %dim.begin_range,
                            error = %e,
                            "failed to parse beginRange; defaulting to 0.0"
                        );
                        0.0
                    });
                    let end: Option<f64> = if dim.end_range == "Inf" {
                        None
                    } else {
                        dim.end_range.parse().ok()
                    };

                    dimensions.push(PricingDimension {
                        description: dim.description.clone(),
                        unit: dim.unit.clone(),
                        price_usd: dim.usd_price(),
                        begin_range: begin,
                        end_range: end,
                    });
                }
            }
        }

        entries.push(PricingEntry {
            sku: sku.clone(),
            product_family: product.product_family.clone(),
            attributes: product.attributes.clone(),
            dimensions,
        });
    }

    Ok(entries)
}

/// Lookup helper: find pricing entries matching attribute filters.
pub fn find_entries<'a>(
    entries: &'a [PricingEntry],
    filters: &[(&str, &str)],
) -> Vec<&'a PricingEntry> {
    entries
        .iter()
        .filter(|e| {
            filters
                .iter()
                .all(|(key, value)| e.attributes.get(*key).is_some_and(|v| v == *value))
        })
        .collect()
}

/// Lookup helper: find pricing entries matching both a `product_family` value
/// and a set of attribute filters.
///
/// `productFamily` is a top-level field on `PricingEntry` (not stored inside
/// `attributes`), so it cannot be matched by [`find_entries`] alone.
pub fn find_entries_by_family<'a>(
    entries: &'a [PricingEntry],
    product_family: &str,
    filters: &[(&str, &str)],
) -> Vec<&'a PricingEntry> {
    entries
        .iter()
        .filter(|e| {
            e.product_family == product_family
                && filters
                    .iter()
                    .all(|(key, value)| e.attributes.get(*key).is_some_and(|v| v == *value))
        })
        .collect()
}

/// Get the first non-zero USD price from an entry's dimensions.
pub fn first_price(entry: &PricingEntry) -> Option<f64> {
    entry
        .dimensions
        .iter()
        .map(|d| d.price_usd)
        .find(|&p| p > 0.0)
}
