//! Load simulation: cost over time with varying hourly load patterns.
//!
//! A [`SimulationProfile`] describes base usage parameters, an hourly load
//! multiplier pattern, and the set of variables that scale with load.
//! [`simulate_architecture`] evaluates a cost model across all 24 hours and
//! returns the per-hour and aggregated monthly costs as plain data — rendering
//! is left to the caller.
//!
//! Load profile format:
//! ```yaml
//! base_params:
//!   IngestFunction_avg_duration_ms: 200
//!   ...
//! hourly_pattern:
//!   - hour: 0
//!     multiplier: 0.1
//!   - hour: 9
//!     multiplier: 1.0
//!   - hour: 12
//!     multiplier: 0.8
//!   - hour: 18
//!     multiplier: 1.5  # peak
//!   - hour: 22
//!     multiplier: 0.3
//! scaled_variables:
//!   - DataStream_put_records
//!   - IngestFunction_requests
//! days_per_month: 30
//! ```

use std::collections::HashMap;

use serde_yaml_ng::Value;
use thiserror::Error;

use crate::cost::ArchitectureCost;
use crate::error::CoreError;
use crate::evaluate::{Params, evaluate_architecture};
use crate::io::{ParamValueError, params_from_yaml_map};
use crate::types::VariableName;

/// Errors raised while parsing a simulation profile or running a simulation.
#[derive(Debug, Error)]
pub enum SimulateError {
    /// The profile file is not valid YAML.
    #[error("failed to parse profile")]
    Yaml(#[source] serde_yaml_ng::Error),

    /// The top-level YAML document is not a mapping.
    #[error("profile must be a mapping")]
    NotAMapping,

    /// The profile has no `base_params` key.
    #[error("profile must have base_params")]
    MissingBaseParams,

    /// `base_params` is not a mapping of variable names to values.
    #[error("failed to parse base_params")]
    InvalidBaseParams(#[source] serde_yaml_ng::Error),

    /// A `base_params` entry holds a value that cannot be read as a number.
    #[error(transparent)]
    BaseParamValue(#[from] ParamValueError),

    /// The profile has no `hourly_pattern` array.
    #[error("profile must have hourly_pattern array")]
    MissingHourlyPattern,

    /// An `hourly_pattern` entry is not a mapping.
    #[error("hourly entry must be a mapping")]
    HourlyEntryNotAMapping,

    /// An `hourly_pattern` entry has no `hour` key.
    #[error("hourly entry must have hour")]
    HourlyEntryMissingHour,

    /// An `hourly_pattern` entry has no `multiplier` key.
    #[error("hourly entry must have multiplier")]
    HourlyEntryMissingMultiplier,

    /// Evaluating the architecture at `base_params` failed.
    #[error("failed to evaluate base cost for architecture '{arch}'")]
    BaseEvaluation {
        /// Architecture name.
        arch: String,
        #[source]
        source: CoreError,
    },

    /// Evaluating the architecture at a specific hour failed.
    #[error(
        "failed to evaluate '{arch}' at hour {hour} in simulation (check that the load \
         profile's base_params provides every variable the cost model references)"
    )]
    HourEvaluation {
        /// Architecture name.
        arch: String,
        /// Hour of day (0–23) at which evaluation failed.
        hour: u32,
        #[source]
        source: CoreError,
    },
}

/// A load profile for cost simulation over a 24-hour day.
#[derive(Debug)]
pub struct SimulationProfile {
    /// Baseline usage parameters (monthly values).
    pub base_params: Params,
    /// `(hour, multiplier)` change points, sorted by hour. Each multiplier
    /// applies from its hour until the next change point.
    pub hourly_pattern: Vec<(u32, f64)>,
    /// Variables that scale with the hourly load multiplier.
    pub scaled_variables: Vec<VariableName>,
    /// Number of days per month used to convert monthly values to hourly.
    pub days_per_month: f64,
}

impl SimulationProfile {
    /// Parse a simulation profile from YAML text.
    ///
    /// `base_params` supports both flat (`Foo_requests: 100`) and hierarchical
    /// (`Foo: { requests: 100 }`) keys, matching the usage-params file format.
    pub fn from_yaml_str(content: &str) -> Result<Self, SimulateError> {
        let raw: Value = serde_yaml_ng::from_str(content).map_err(SimulateError::Yaml)?;
        let map = raw.as_mapping().ok_or(SimulateError::NotAMapping)?;

        // Load base_params
        let base_params_val = map
            .get(Value::String("base_params".into()))
            .ok_or(SimulateError::MissingBaseParams)?;
        let base_map: HashMap<String, Value> = serde_yaml_ng::from_value(base_params_val.clone())
            .map_err(SimulateError::InvalidBaseParams)?;

        let base_params = params_from_yaml_map(base_map, "profile base_param")?;

        // Load hourly_pattern
        let pattern_val = map
            .get(Value::String("hourly_pattern".into()))
            .and_then(|v| v.as_sequence())
            .ok_or(SimulateError::MissingHourlyPattern)?;

        let mut hourly_pattern: Vec<(u32, f64)> = Vec::new();
        for entry in pattern_val {
            let entry_map = entry
                .as_mapping()
                .ok_or(SimulateError::HourlyEntryNotAMapping)?;
            let hour = entry_map
                .get(Value::String("hour".into()))
                .and_then(Value::as_u64)
                .ok_or(SimulateError::HourlyEntryMissingHour)? as u32;
            let multiplier = entry_map
                .get(Value::String("multiplier".into()))
                .and_then(Value::as_f64)
                .ok_or(SimulateError::HourlyEntryMissingMultiplier)?;
            hourly_pattern.push((hour, multiplier));
        }
        hourly_pattern.sort_by_key(|(h, _)| *h);

        // Load scaled_variables
        let scaled = map
            .get(Value::String("scaled_variables".into()))
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(VariableName::new))
                    .collect()
            })
            .unwrap_or_default();

        // Days per month
        let days = map
            .get(Value::String("days_per_month".into()))
            .and_then(Value::as_f64)
            .unwrap_or(30.0);

        Ok(Self {
            base_params,
            hourly_pattern,
            scaled_variables: scaled,
            days_per_month: days,
        })
    }

    /// The load multiplier in effect at `hour` (the last change point at or
    /// before that hour; falls back to the first entry, or `1.0` when the
    /// pattern is empty).
    pub fn multiplier_at(&self, hour: u32) -> f64 {
        // Find the last defined multiplier at or before this hour
        let mut result = self.hourly_pattern.first().map_or(1.0, |(_, m)| *m);
        for (h, m) in &self.hourly_pattern {
            if *h <= hour {
                result = *m;
            }
        }
        result
    }
}

/// Simulation result for a single architecture.
#[derive(Debug)]
pub struct ArchSimulation {
    /// Architecture name.
    pub name: String,
    /// Aggregated monthly cost across all 24 hourly slices.
    pub total_monthly_cost: f64,
    /// `(hour, monthly-equivalent cost at that hour's load rate)` for each hour.
    pub hourly_costs: Vec<(u32, f64)>,
    /// Per-resource `(label, monthly_cost)` evaluated at `base_params`.
    /// Empty unless `with_base_breakdown` was requested.
    pub base_resource_costs: Vec<(String, f64)>,
}

/// Simulate one architecture's cost over a 24-hour load pattern.
///
/// For each hour, the profile's `scaled_variables` are converted from monthly
/// to hourly values and multiplied by the hour's load multiplier; the cost
/// model is then evaluated at that rate. The total monthly cost aggregates the
/// 24 hourly slices.
///
/// When `with_base_breakdown` is `true`, the architecture is additionally
/// evaluated once at the unscaled `base_params` to produce a per-resource
/// breakdown.
pub fn simulate_architecture(
    arch: &ArchitectureCost,
    profile: &SimulationProfile,
    with_base_breakdown: bool,
) -> Result<ArchSimulation, SimulateError> {
    let arch_name = arch.name.to_string();
    let mut total_monthly = 0.0;
    let mut hourly_costs = Vec::new();

    // Evaluate at base_params once for the resource breakdown display.
    let base_resource_costs = if with_base_breakdown {
        let result = evaluate_architecture(arch, &profile.base_params).map_err(|e| {
            SimulateError::BaseEvaluation {
                arch: arch_name.clone(),
                source: e,
            }
        })?;
        result
            .resources
            .into_iter()
            .map(|r| (r.label, r.monthly_cost))
            .collect()
    } else {
        Vec::new()
    };

    for hour in 0..24 {
        let multiplier = profile.multiplier_at(hour);
        let mut params = profile.base_params.clone();

        // Scale designated variables by the hourly multiplier
        for var_name in &profile.scaled_variables {
            if let Some(base_val) = params.get(var_name).copied() {
                // Convert monthly value to hourly, apply multiplier
                let hourly_val = base_val / (24.0 * profile.days_per_month) * multiplier;
                params.insert(var_name.clone(), hourly_val);
            }
        }

        // Evaluate cost for this hour's load (as monthly equivalent at this rate)
        match evaluate_architecture(arch, &params) {
            Ok(result) => {
                // Each of the 24 hourly slices contributes 1/24 of its
                // monthly-rate cost, independent of days_per_month.
                let hour_cost = result.total_monthly_cost / 24.0;
                total_monthly += hour_cost;
                hourly_costs.push((hour, result.total_monthly_cost));
            }
            Err(e) => {
                return Err(SimulateError::HourEvaluation {
                    arch: arch_name,
                    hour,
                    source: e,
                });
            }
        }
    }

    Ok(ArchSimulation {
        name: arch_name,
        total_monthly_cost: total_monthly,
        hourly_costs,
        base_resource_costs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE_YAML: &str = "\
base_params:
  Fn_requests: 7200
  Fn:
    duration_ms: 200
hourly_pattern:
  - hour: 0
    multiplier: 0.5
  - hour: 12
    multiplier: 2.0
scaled_variables:
  - Fn_requests
days_per_month: 30
";

    #[test]
    fn parses_profile_with_flat_and_hierarchical_base_params() {
        let profile = SimulationProfile::from_yaml_str(PROFILE_YAML).unwrap();
        assert_eq!(
            profile.base_params.get(&VariableName::new("Fn_requests")),
            Some(&7200.0)
        );
        assert_eq!(
            profile
                .base_params
                .get(&VariableName::new("Fn_duration_ms")),
            Some(&200.0)
        );
        assert_eq!(profile.hourly_pattern, vec![(0, 0.5), (12, 2.0)]);
        assert_eq!(
            profile.scaled_variables,
            vec![VariableName::new("Fn_requests")]
        );
        assert_eq!(profile.days_per_month, 30.0);
    }

    #[test]
    fn multiplier_at_uses_last_change_point_at_or_before_hour() {
        let profile = SimulationProfile::from_yaml_str(PROFILE_YAML).unwrap();
        assert_eq!(profile.multiplier_at(0), 0.5);
        assert_eq!(profile.multiplier_at(11), 0.5);
        assert_eq!(profile.multiplier_at(12), 2.0);
        assert_eq!(profile.multiplier_at(23), 2.0);
    }

    #[test]
    fn multiplier_defaults_to_one_for_empty_pattern() {
        let profile = SimulationProfile {
            base_params: Params::default(),
            hourly_pattern: vec![],
            scaled_variables: vec![],
            days_per_month: 30.0,
        };
        assert_eq!(profile.multiplier_at(5), 1.0);
    }

    #[test]
    fn days_per_month_defaults_to_thirty() {
        let yaml = "\
base_params:
  X: 1
hourly_pattern:
  - hour: 0
    multiplier: 1.0
";
        let profile = SimulationProfile::from_yaml_str(yaml).unwrap();
        assert_eq!(profile.days_per_month, 30.0);
        assert!(profile.scaled_variables.is_empty());
    }

    #[test]
    fn rejects_profile_without_base_params() {
        let err = SimulationProfile::from_yaml_str("hourly_pattern: []\n").unwrap_err();
        assert!(matches!(err, SimulateError::MissingBaseParams));
    }

    #[test]
    fn rejects_profile_without_hourly_pattern() {
        let err = SimulationProfile::from_yaml_str("base_params:\n  X: 1\n").unwrap_err();
        assert!(matches!(err, SimulateError::MissingHourlyPattern));
    }

    #[test]
    fn simulates_empty_architecture_to_zero_cost() {
        let arch: ArchitectureCost = serde_json::from_value(serde_json::json!({
            "name": "empty",
            "resources": [],
            "region": "ap-northeast-1",
            "topology": { "nodes": [], "connections": [] }
        }))
        .unwrap();
        let profile = SimulationProfile::from_yaml_str(PROFILE_YAML).unwrap();

        let sim = simulate_architecture(&arch, &profile, true).unwrap();
        assert_eq!(sim.name, "empty");
        assert_eq!(sim.total_monthly_cost, 0.0);
        assert_eq!(sim.hourly_costs.len(), 24);
        assert!(sim.base_resource_costs.is_empty());
    }
}
