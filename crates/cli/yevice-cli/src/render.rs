//! Table-rendering helpers extracted from the command implementations.
//!
//! Each function constructs a [`comfy_table::Table`] from structured data and
//! returns it ready for printing.  The calling command function calls
//! `println!("{table}")` — no formatting logic lives outside this module.

use std::collections::BTreeMap;

use comfy_table::{Cell, Color, Table, presets::UTF8_FULL};
use yevice_core::Money;
use yevice_core::capacity::{Severity, Violation};
use yevice_core::evaluate::ArchitectureResult;
use yevice_core::fx::{RateDate, StaticRates, convert_to};
use yevice_core::simulate::ArchSimulation;

/// Render a [`Money`] amount with its declared currency code suffix.
///
/// USD uses the historical `$<value>` glyph for backward-compatible output;
/// other currencies render as `<value> <CODE>` (e.g. `1000.00 JPY`). This is
/// the only place in the CLI that decides between symbol vs. ISO code, so
/// updating the convention later only touches this function.
fn fmt_money(m: &Money) -> String {
    fmt_amount(m.value, &m.currency)
}

fn fmt_money_4(m: &Money) -> String {
    fmt_amount_4(m.value, &m.currency)
}

fn fmt_amount(value: f64, currency: &str) -> String {
    if currency == "USD" {
        format!("${value:.2}")
    } else {
        format!("{value:.2} {currency}")
    }
}

fn fmt_amount_4(value: f64, currency: &str) -> String {
    if currency == "USD" {
        format!("${value:.4}")
    } else {
        format!("{value:.4} {currency}")
    }
}

/// Pick the single-currency code most representative of the result for the
/// header label. Returns `"USD"` for empty/multi-currency models so the
/// header stays stable across mixed evaluations (the per-currency
/// breakdown printed underneath the table conveys the full picture).
fn header_currency(result: &ArchitectureResult) -> &str {
    if let Some(money) = &result.display_total {
        return money.currency.as_str();
    }
    if result.totals_by_currency.len() == 1 {
        return result.totals_by_currency.keys().next().unwrap();
    }
    "USD"
}

// ---------------------------------------------------------------------------
// eval
// ---------------------------------------------------------------------------

/// Convert a single [`Money`] value to `target_currency` using `rates`.
///
/// Returns `None` when the rate is missing so callers can fall back to a
/// placeholder rather than mixing currencies silently.
fn try_convert_money(
    money: &Money,
    target: &str,
    rates: &StaticRates,
    at: RateDate,
) -> Option<Money> {
    let mut single: BTreeMap<String, f64> = BTreeMap::new();
    single.insert(money.currency.clone(), money.value);
    convert_to(&single, target, rates, at).ok()
}

/// Build the per-resource cost table for `eval --breakdown`.
///
/// Each resource appears as a coloured row followed by indented component rows.
/// A green TOTAL row is appended at the bottom.
///
/// When `display_currency` is `Some((rates, target, at))`, per-resource costs
/// are converted to `target`; rows that cannot be converted show `"n/a"`.
pub(crate) fn render_eval_breakdown_table(
    result: &ArchitectureResult,
    display_currency: Option<(&StaticRates, &str, RateDate)>,
) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    let header_ccy = header_currency(result).to_string();
    table.set_header(vec![
        "Resource / Component".to_string(),
        format!("Monthly Cost ({header_ccy})"),
    ]);

    for r in &result.resources {
        let cost_str = if let Some((rates, target, at)) = display_currency {
            match try_convert_money(&r.monthly_cost, target, rates, at) {
                Some(m) => fmt_money(&m),
                None => "n/a".to_string(),
            }
        } else {
            fmt_money(&r.monthly_cost)
        };
        table.add_row(vec![
            Cell::new(&r.label).fg(Color::Cyan),
            Cell::new(cost_str).fg(Color::Cyan),
        ]);
        for (name, cost) in &r.component_costs {
            let comp_str = if let Some((rates, target, at)) = display_currency {
                match try_convert_money(cost, target, rates, at) {
                    Some(m) => fmt_money_4(&m),
                    None => "n/a".to_string(),
                }
            } else {
                fmt_money_4(cost)
            };
            table.add_row(vec![Cell::new(format!("  └─ {name}")), Cell::new(comp_str)]);
        }
    }

    // When --display-currency was applied, show the FX-converted total.
    // Single-currency results show the native total. Mixed currencies
    // without --display-currency render `mixed (see breakdown)` so the
    // table never folds heterogeneous numbers; the caller prints the
    // per-currency breakdown.
    let total_cell = if let Some(money) = &result.display_total {
        fmt_money(money)
    } else if result.totals_by_currency.len() > 1 {
        "mixed (see breakdown)".to_string()
    } else {
        fmt_amount(result.naive_total(), &header_ccy)
    };
    table.add_row(vec![
        Cell::new("TOTAL").fg(Color::Green),
        Cell::new(total_cell).fg(Color::Green),
    ]);

    table
}

/// Build the simple (no-breakdown) per-resource cost table for `eval`.
///
/// Each resource appears on one row with label, type, and monthly cost.
/// A green TOTAL row is appended at the bottom.
///
/// When `display_currency` is `Some((rates, target, at))`, per-resource costs
/// are converted to `target`; rows that cannot be converted show `"n/a"`.
pub(crate) fn render_eval_table(
    result: &ArchitectureResult,
    display_currency: Option<(&StaticRates, &str, RateDate)>,
) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    let header_ccy = header_currency(result).to_string();
    table.set_header(vec![
        "Resource".to_string(),
        "Type".to_string(),
        format!("Monthly Cost ({header_ccy})"),
    ]);

    for r in &result.resources {
        let cost_str = if let Some((rates, target, at)) = display_currency {
            match try_convert_money(&r.monthly_cost, target, rates, at) {
                Some(m) => fmt_money(&m),
                None => "n/a".to_string(),
            }
        } else {
            fmt_money(&r.monthly_cost)
        };
        table.add_row(vec![
            Cell::new(&r.label),
            Cell::new(&r.resource_type),
            Cell::new(cost_str),
        ]);
    }

    let total_cell = if let Some(money) = &result.display_total {
        fmt_money(money)
    } else if result.totals_by_currency.len() > 1 {
        "mixed (see breakdown)".to_string()
    } else {
        fmt_amount(result.naive_total(), &header_ccy)
    };
    table.add_row(vec![
        Cell::new("TOTAL").fg(Color::Green),
        Cell::new(""),
        Cell::new(total_cell).fg(Color::Green),
    ]);

    table
}

// ---------------------------------------------------------------------------
// compare
// ---------------------------------------------------------------------------

/// Build the multi-architecture comparison summary table for `compare`.
///
/// When `breakdown` is `true`, component-level rows are inserted after each
/// resource row.  A difference row is added when exactly two architectures are
/// compared.
pub(crate) fn render_compare_table(results: &[ArchitectureResult], breakdown: bool) -> Table {
    let mut summary = Table::new();
    summary.load_preset(UTF8_FULL);

    let mut header = vec![Cell::new("Architecture")];
    for r in results {
        header.push(Cell::new(&r.name));
    }
    summary.set_header(header);

    // Total row — prefer the FX-converted display_total when present,
    // otherwise show the native single-currency total. Mixed-currency
    // results without --display-currency render `mixed (see breakdown)`
    // so the table never folds incompatible numbers into a single cell;
    // the caller is responsible for printing the per-currency breakdown.
    let mut total_row = vec![Cell::new("Total Monthly Cost").fg(Color::Green)];
    for r in results {
        let cell = if let Some(money) = &r.display_total {
            fmt_money(money)
        } else if r.totals_by_currency.len() > 1 {
            "mixed (see breakdown)".to_string()
        } else {
            fmt_amount(r.naive_total(), header_currency(r))
        };
        total_row.push(Cell::new(cell));
    }
    summary.add_row(total_row);

    // Collect all unique resource labels across architectures.
    let mut all_labels: Vec<String> = Vec::new();
    for r in results {
        for res in &r.resources {
            if !all_labels.contains(&res.label) {
                all_labels.push(res.label.clone());
            }
        }
    }

    for label in &all_labels {
        let mut row = vec![Cell::new(label)];
        for r in results {
            let cost = r
                .resources
                .iter()
                .find(|res| &res.label == label)
                .map_or_else(|| "-".to_string(), |res| fmt_money(&res.monthly_cost));
            row.push(Cell::new(cost));
        }
        summary.add_row(row);

        if breakdown {
            let mut all_component_names: Vec<String> = Vec::new();
            for r in results {
                if let Some(res) = r.resources.iter().find(|res| &res.label == label) {
                    for (name, _) in &res.component_costs {
                        if !all_component_names.contains(name) {
                            all_component_names.push(name.clone());
                        }
                    }
                }
            }
            for comp_name in &all_component_names {
                let mut comp_row = vec![Cell::new(format!("  └─ {comp_name}"))];
                for r in results {
                    let comp_cost = r
                        .resources
                        .iter()
                        .find(|res| &res.label == label)
                        .and_then(|res| res.component_costs.iter().find(|(n, _)| n == comp_name))
                        .map_or_else(|| "-".to_string(), |(_, v)| fmt_money_4(v));
                    comp_row.push(Cell::new(comp_cost));
                }
                summary.add_row(comp_row);
            }
        }
    }

    // Difference row (only when exactly 2 architectures are compared, and
    // only when both totals are commensurate — i.e. both have a converted
    // `display_total` in the same currency, or both are single-currency in
    // the same native currency). Mixed currencies without `--display-currency`
    // get an "n/a (mixed)" row instead of a misleading scalar diff.
    if results.len() == 2 {
        let comparable: Option<(f64, String)> =
            match (&results[0].display_total, &results[1].display_total) {
                (Some(a), Some(b)) if a.currency == b.currency => {
                    Some((b.value - a.value, b.currency.clone()))
                }
                _ if results[0].totals_by_currency.len() == 1
                    && results[1].totals_by_currency.len() == 1
                    && results[0].totals_by_currency.keys().next()
                        == results[1].totals_by_currency.keys().next() =>
                {
                    let ccy = header_currency(&results[1]).to_string();
                    Some((results[1].naive_total() - results[0].naive_total(), ccy))
                }
                _ => None,
            };
        if let Some((diff, ccy)) = comparable {
            let diff_str = if diff >= 0.0 {
                format!("+{}", fmt_amount(diff, &ccy))
            } else {
                format!("-{}", fmt_amount(diff.abs(), &ccy))
            };
            let color = if diff > 0.0 { Color::Red } else { Color::Green };
            summary.add_row(vec![
                Cell::new("Difference"),
                Cell::new("-"),
                Cell::new(diff_str).fg(color),
            ]);
        } else {
            summary.add_row(vec![
                Cell::new("Difference"),
                Cell::new("-"),
                Cell::new("n/a (currency mismatch)").fg(Color::Yellow),
            ]);
        }
    }

    summary
}

// ---------------------------------------------------------------------------
// sensitivity
// ---------------------------------------------------------------------------

/// One row of data for the sensitivity sweep table.
pub(crate) enum SensitivityRow {
    Ok {
        value: f64,
        /// Total monthly cost for this step.  When `--display-currency` was
        /// applied, this is the FX-converted amount; otherwise it is the
        /// native single-currency total (for mixed-currency models it is
        /// `None` so the table can show `"mixed"` instead of a misleading
        /// scalar).
        total: Option<Money>,
        /// Delta from the base cost in the same currency as `total`.
        delta: Option<Money>,
    },
    Err {
        value: f64,
        message: String,
    },
}

/// Build the main sensitivity sweep table for `sensitivity`.
///
/// One row per step: the varied variable's value, total monthly cost, and
/// coloured delta from the base cost.
///
/// `currency` is the display currency code (single-currency native or
/// `--display-currency` target), used for the column header.  Pass `None`
/// only when the model is mixed-currency and no `--display-currency` was
/// supplied (header falls back to `"Cost"`).
pub(crate) fn render_sensitivity_table(
    var_name: &str,
    rows: &[SensitivityRow],
    currency: Option<&str>,
) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    let cost_header = match currency {
        Some(ccy) => format!("Total Monthly Cost ({ccy})"),
        None => "Total Monthly Cost".to_string(),
    };
    table.set_header(vec![
        Cell::new(var_name),
        Cell::new(cost_header),
        Cell::new("Delta from Base"),
    ]);

    for row in rows {
        match row {
            SensitivityRow::Ok {
                value,
                total,
                delta,
            } => {
                let total_str = match total {
                    Some(m) => fmt_money(m),
                    None => "mixed (see breakdown)".to_string(),
                };
                let (delta_str, color) = match delta {
                    Some(m) if m.value > 0.0 => (format!("+{}", fmt_money(m)), Color::Red),
                    Some(m) if m.value < 0.0 => {
                        // negative delta: show absolute value with minus sign
                        let abs_m = Money::monthly(m.value.abs(), &m.currency);
                        (format!("-{}", fmt_money(&abs_m)), Color::Green)
                    }
                    Some(m) => (format!("+{}", fmt_money(m)), Color::White),
                    None => ("mixed".to_string(), Color::Yellow),
                };
                table.add_row(vec![
                    Cell::new(format_number(*value)),
                    Cell::new(total_str),
                    Cell::new(delta_str).fg(color),
                ]);
            }
            SensitivityRow::Err { value, message } => {
                table.add_row(vec![
                    Cell::new(format_number(*value)),
                    Cell::new(format!("ERROR: {message}")),
                    Cell::new("-"),
                ]);
            }
        }
    }

    table
}

/// Build the per-resource breakdown table for `sensitivity --breakdown`.
///
/// `resource_costs` contains per-step, per-resource cost as `Option<Money>`;
/// `None` means the cost could not be determined (evaluation error or
/// mixed-currency without conversion).  `currency` is used for the column
/// header suffix.
pub(crate) fn render_sensitivity_breakdown_table(
    var_name: &str,
    resource_labels: &[String],
    breakdown_rows: &[(f64, Vec<Option<Money>>)],
    currency: Option<&str>,
) -> Table {
    let mut bd_table = Table::new();
    bd_table.load_preset(UTF8_FULL);

    let mut bd_header = vec![Cell::new(var_name)];
    let ccy_suffix = currency.map(|c| format!(" ({c})")).unwrap_or_default();
    for label in resource_labels {
        bd_header.push(Cell::new(format!("{label}{ccy_suffix}")));
    }
    bd_table.set_header(bd_header);

    for (value, costs) in breakdown_rows {
        let mut row = vec![Cell::new(format_number(*value))];
        for cost in costs {
            let cell = match cost {
                Some(m) => fmt_money(m),
                None => "n/a".to_string(),
            };
            row.push(Cell::new(cell));
        }
        bd_table.add_row(row);
    }

    bd_table
}

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

/// Build the capacity violations table for `validate`.
pub(crate) fn render_validate_table(violations: &[Violation]) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec![
        Cell::new("Severity"),
        Cell::new("Resource"),
        Cell::new("Constraint"),
        Cell::new("Required"),
        Cell::new("Limit"),
        Cell::new("Message"),
    ]);

    for v in violations {
        let color = match v.severity {
            Severity::Error => Color::Red,
            Severity::Warning => Color::Yellow,
            Severity::Info => Color::Cyan,
        };
        table.add_row(vec![
            Cell::new(v.severity.to_string()).fg(color),
            Cell::new(v.resource.to_string()),
            Cell::new(&v.dimension),
            Cell::new(format!("{:.0}", v.required)),
            Cell::new(format!("{:.0}", v.limit)),
            Cell::new(&v.message),
        ]);
    }

    table
}

// ---------------------------------------------------------------------------
// simulate
// ---------------------------------------------------------------------------

/// Determine the display currency string for the column header of a simulate
/// table. Mirrors the logic used by `eval`/`compare`:
/// - `display_currency` set → use that code
/// - all sims are single-currency and share the same code → use it
/// - otherwise (mixed) → `None` (header shows `"rate/mo"`)
fn simulate_header_currency<'a>(
    arch_results: &'a [ArchSimulation],
    display_currency: Option<&'a str>,
) -> Option<&'a str> {
    if let Some(target) = display_currency {
        return Some(target);
    }
    if arch_results.is_empty() {
        return None;
    }
    let first = arch_results[0].single_currency()?;
    if arch_results
        .iter()
        .all(|s| s.single_currency() == Some(first))
    {
        Some(first)
    } else {
        None
    }
}

/// Build the hourly load simulation table for `simulate`.
///
/// When `conversion` is `Some((rates, target, at))`, every hourly cell is
/// converted to `target` currency via [`try_convert_money`].  Rows whose
/// per-hour value cannot be converted (e.g. missing rate) show `"n/a"`.
/// When `conversion` is `None` and the model has mixed currencies, each cell
/// shows `"mixed"`.
pub(crate) fn render_simulate_table(
    arch_results: &[ArchSimulation],
    multiplier_at: impl Fn(u32) -> f64,
    display_currency: Option<&str>,
    conversion: Option<(&StaticRates, &str, RateDate)>,
) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);

    let header_ccy = simulate_header_currency(arch_results, display_currency);

    let mut header = vec![Cell::new("Hour"), Cell::new("Multiplier")];
    for sim in arch_results {
        let col_label = match header_ccy {
            Some(ccy) => format!("{} (rate/mo, {})", sim.name, ccy),
            None => format!("{} (rate/mo)", sim.name),
        };
        header.push(Cell::new(col_label));
    }
    table.set_header(header);

    for hour in 0..24 {
        let mult = multiplier_at(hour);
        let mut row = vec![
            Cell::new(format!("{hour:02}:00")),
            Cell::new(format!("{mult:.2}x")),
        ];
        for sim in arch_results {
            let cell_str = if let Some(by_ccy) = sim
                .hourly_costs
                .iter()
                .find(|(h, _)| *h == hour)
                .map(|(_, m)| m)
            {
                if let Some((rates, target, at)) = conversion {
                    // --display-currency is set: convert the hourly total to target.
                    match convert_to(by_ccy, target, rates, at) {
                        Ok(money) => fmt_money(&money),
                        Err(_) => "n/a".to_string(),
                    }
                } else if by_ccy.len() == 1 {
                    let (ccy, &v) = by_ccy.iter().next().unwrap();
                    fmt_amount(v, ccy)
                } else if by_ccy.is_empty() {
                    fmt_amount(0.0, "USD")
                } else {
                    // Mixed currencies per hour: show "mixed"
                    "mixed".to_string()
                }
            } else {
                fmt_amount(0.0, "USD")
            };
            row.push(Cell::new(cell_str));
        }
        table.add_row(row);
    }

    // Total row
    let mut total_row = vec![Cell::new("MONTHLY TOTAL").fg(Color::Green), Cell::new("")];
    for sim in arch_results {
        let total_str = if let Some(money) = &sim.display_total {
            fmt_money(money)
        } else if sim.totals_by_currency.len() > 1 {
            "mixed (see breakdown)".to_string()
        } else if sim.totals_by_currency.len() == 1 {
            let (ccy, &v) = sim.totals_by_currency.iter().next().unwrap();
            fmt_amount(v, ccy)
        } else {
            fmt_amount(0.0, "USD")
        };
        total_row.push(Cell::new(total_str).fg(Color::Green));
    }
    table.add_row(total_row);

    table
}

/// Build the resource breakdown table for `simulate --breakdown`.
///
/// When `display_currency` is `Some((rates, target, at))`, each
/// `base_resource_costs` entry is converted to `target`; rows that cannot be
/// converted show `"n/a"`.
pub(crate) fn render_simulate_breakdown_table(
    arch_results: &[ArchSimulation],
    all_labels: &[String],
    display_currency: Option<(&StaticRates, &str, RateDate)>,
) -> Table {
    let mut bd_table = Table::new();
    bd_table.load_preset(UTF8_FULL);

    let mut bd_header = vec![Cell::new("Resource")];
    for sim in arch_results {
        bd_header.push(Cell::new(&sim.name));
    }
    bd_table.set_header(bd_header);

    for label in all_labels {
        let mut row = vec![Cell::new(label)];
        for sim in arch_results {
            let cost = sim
                .base_resource_costs
                .iter()
                .find(|(l, _)| l == label)
                .map_or_else(
                    || "-".to_string(),
                    |(_, c)| {
                        if let Some((rates, target, at)) = display_currency {
                            match try_convert_money(c, target, rates, at) {
                                Some(m) => fmt_money(&m),
                                None => "n/a".to_string(),
                            }
                        } else {
                            fmt_money(c)
                        }
                    },
                );
            row.push(Cell::new(cost));
        }
        bd_table.add_row(row);
    }

    bd_table
}

// ---------------------------------------------------------------------------
// Shared helper
// ---------------------------------------------------------------------------

pub(crate) fn format_number(n: f64) -> String {
    if n >= 1_000_000.0 {
        format!("{:.1}M", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.1}K", n / 1_000.0)
    } else {
        format!("{n:.2}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::NaiveDate;
    use yevice_core::fx::{RateDate, StaticRates};
    use yevice_core::simulate::ArchSimulation;

    use yevice_core::Money;

    use super::{render_simulate_breakdown_table, render_simulate_table};

    /// Build a minimal [`ArchSimulation`] with a single currency.
    fn make_jpy_sim(name: &str, jpy_per_hour: f64) -> ArchSimulation {
        let mut totals_by_currency = BTreeMap::new();
        totals_by_currency.insert("JPY".to_string(), jpy_per_hour * 24.0);

        let mut hourly_costs = Vec::new();
        for hour in 0..24_u32 {
            let mut by_ccy = BTreeMap::new();
            by_ccy.insert("JPY".to_string(), jpy_per_hour);
            hourly_costs.push((hour, by_ccy));
        }

        ArchSimulation {
            name: name.to_string(),
            totals_by_currency,
            display_total: None,
            hourly_costs,
            base_resource_costs: vec![],
        }
    }

    /// Build a mixed-currency [`ArchSimulation`] (USD + JPY per hour).
    fn make_mixed_sim(name: &str, usd_per_hour: f64, jpy_per_hour: f64) -> ArchSimulation {
        let mut totals_by_currency = BTreeMap::new();
        totals_by_currency.insert("USD".to_string(), usd_per_hour * 24.0);
        totals_by_currency.insert("JPY".to_string(), jpy_per_hour * 24.0);

        let mut hourly_costs = Vec::new();
        for hour in 0..24_u32 {
            let mut by_ccy = BTreeMap::new();
            by_ccy.insert("USD".to_string(), usd_per_hour);
            by_ccy.insert("JPY".to_string(), jpy_per_hour);
            hourly_costs.push((hour, by_ccy));
        }

        ArchSimulation {
            name: name.to_string(),
            totals_by_currency,
            display_total: None,
            hourly_costs,
            base_resource_costs: vec![],
        }
    }

    fn make_rates_jpy_to_usd(rate: f64) -> StaticRates {
        let mut rates = StaticRates::new();
        rates.insert("JPY", "USD", rate);
        rates
    }

    fn test_date() -> RateDate {
        RateDate::new(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap())
    }

    /// JPY-only model with `--display-currency USD`: hourly cells must show USD values.
    #[test]
    fn simulate_hourly_jpy_model_converted_to_usd() {
        // 1 JPY per hour; JPY=USD rate 0.0067 → ~0.0067 USD per hour.
        let sim = make_jpy_sim("arch-jpy", 1.0);
        let rates = make_rates_jpy_to_usd(0.0067);
        let at = test_date();
        let conversion = Some((&rates, "USD", at));

        let table = render_simulate_table(&[sim], |_hour| 1.0, Some("USD"), conversion);
        let rendered = table.to_string();

        // Header should mention USD.
        assert!(
            rendered.contains("USD"),
            "header should contain USD, got:\n{rendered}"
        );
        // Each hourly cell should be a dollar amount, not a raw JPY value.
        // 1 JPY * 0.0067 = 0.0067 USD → renders as "$0.01" (two decimal places).
        assert!(
            rendered.contains("$0.01"),
            "hourly cells should show USD amounts, got:\n{rendered}"
        );
        // Must NOT contain raw JPY formatting like "1.00 JPY" in hourly rows.
        // (The header row might contain "USD" which is fine.)
        assert!(
            !rendered.contains("1.00 JPY"),
            "hourly cells must not contain JPY after conversion, got:\n{rendered}"
        );
    }

    /// Mixed-currency model with `--display-currency USD`: all rows should be
    /// converted to USD, not "mixed".
    #[test]
    fn simulate_hourly_mixed_model_converted_to_usd() {
        // 1.0 USD + 100.0 JPY per hour; with JPY=USD:0.01 → 1.0 + 1.0 = 2.0 USD/hr.
        let sim = make_mixed_sim("arch-mixed", 1.0, 100.0);
        let mut rates = StaticRates::new();
        rates.insert("JPY", "USD", 0.01);
        rates.insert("USD", "USD", 1.0);
        let at = test_date();
        let conversion = Some((&rates, "USD", at));

        let table = render_simulate_table(&[sim], |_hour| 1.0, Some("USD"), conversion);
        let rendered = table.to_string();

        // Hourly cells must NOT contain "mixed" since conversion is active.
        // The MONTHLY TOTAL row may still show "mixed (see breakdown)" because
        // display_total is set by the CLI layer (apply_simulate_display_currency),
        // not by render_simulate_table itself.  We only verify that the per-hour
        // cells are converted correctly.
        assert!(
            rendered.contains("$2.00"),
            "hourly cells should show USD dollar amounts ($2.00), got:\n{rendered}"
        );
        // The only "mixed" occurrence allowed is in the MONTHLY TOTAL row
        // ("mixed (see breakdown)").  Individual hourly rows (lines starting with
        // e.g. "│ 00:00") must not contain "mixed".
        for line in rendered.lines() {
            // Lines that contain a time stamp like "│ 00:00" are hourly rows.
            if line.contains(":00") && !line.contains("MONTHLY") {
                assert!(
                    !line.contains("mixed"),
                    "hourly row must not show 'mixed' when --display-currency is set: {line}"
                );
            }
        }
    }

    /// Without `--display-currency`, mixed-currency model rows still show "mixed".
    #[test]
    fn simulate_hourly_mixed_model_no_conversion_shows_mixed() {
        let sim = make_mixed_sim("arch-mixed", 1.0, 100.0);

        let table = render_simulate_table(&[sim], |_hour| 1.0, None, None);
        let rendered = table.to_string();

        assert!(
            rendered.contains("mixed"),
            "without --display-currency, mixed rows should show 'mixed', got:\n{rendered}"
        );
    }

    /// Build an [`ArchSimulation`] with `base_resource_costs` in USD.
    fn make_sim_with_usd_resources(name: &str) -> ArchSimulation {
        let mut totals_by_currency = BTreeMap::new();
        totals_by_currency.insert("USD".to_string(), 10.0 * 24.0);

        let mut hourly_costs = Vec::new();
        for hour in 0..24_u32 {
            let mut by_ccy = BTreeMap::new();
            by_ccy.insert("USD".to_string(), 10.0);
            hourly_costs.push((hour, by_ccy));
        }

        ArchSimulation {
            name: name.to_string(),
            totals_by_currency,
            display_total: None,
            hourly_costs,
            base_resource_costs: vec![
                ("resource-a".to_string(), Money::monthly(100.0, "USD")),
                ("resource-b".to_string(), Money::monthly(50.0, "USD")),
            ],
        }
    }

    /// Build an [`ArchSimulation`] with `base_resource_costs` in JPY.
    fn make_sim_with_jpy_resources(name: &str) -> ArchSimulation {
        let mut totals_by_currency = BTreeMap::new();
        totals_by_currency.insert("JPY".to_string(), 1000.0 * 24.0);

        let mut hourly_costs = Vec::new();
        for hour in 0..24_u32 {
            let mut by_ccy = BTreeMap::new();
            by_ccy.insert("JPY".to_string(), 1000.0);
            hourly_costs.push((hour, by_ccy));
        }

        ArchSimulation {
            name: name.to_string(),
            totals_by_currency,
            display_total: None,
            hourly_costs,
            base_resource_costs: vec![("resource-a".to_string(), Money::monthly(15000.0, "JPY"))],
        }
    }

    /// Breakdown rows for a JPY model with `--display-currency JPY` must show
    /// JPY values (identity conversion), not native USD.
    #[test]
    fn simulate_breakdown_jpy_only_rows_show_jpy() {
        let sim = make_sim_with_jpy_resources("arch-jpy");
        let all_labels = vec!["resource-a".to_string()];
        // No conversion needed: JPY → JPY is identity, but we pass no rates.
        let table = render_simulate_breakdown_table(&[sim], &all_labels, None);
        let rendered = table.to_string();

        assert!(
            rendered.contains("15000.00 JPY"),
            "breakdown row should show native JPY value, got:\n{rendered}"
        );
        assert!(
            !rendered.contains('$'),
            "breakdown row must not show USD symbol when currency is JPY, got:\n{rendered}"
        );
    }

    /// Breakdown rows for a USD model with `--display-currency JPY` must show
    /// JPY-converted values, not the original `$` amounts.
    #[test]
    fn simulate_breakdown_usd_resources_converted_to_jpy() {
        let sim = make_sim_with_usd_resources("arch-usd");
        let all_labels = vec!["resource-a".to_string(), "resource-b".to_string()];

        let mut rates = StaticRates::new();
        // 1 USD = 150 JPY
        rates.insert("USD", "JPY", 150.0);
        let at = test_date();
        let conversion = Some((&rates, "JPY", at));

        let table = render_simulate_breakdown_table(&[sim], &all_labels, conversion);
        let rendered = table.to_string();

        // resource-a: $100 * 150 = 15000 JPY
        assert!(
            rendered.contains("15000.00 JPY"),
            "resource-a should be 15000.00 JPY, got:\n{rendered}"
        );
        // resource-b: $50 * 150 = 7500 JPY
        assert!(
            rendered.contains("7500.00 JPY"),
            "resource-b should be 7500.00 JPY, got:\n{rendered}"
        );
        // Must NOT contain raw USD amounts like "$100.00"
        assert!(
            !rendered.contains("$100.00"),
            "breakdown must not show native USD after --display-currency JPY, got:\n{rendered}"
        );
        assert!(
            !rendered.contains("$50.00"),
            "breakdown must not show native USD after --display-currency JPY, got:\n{rendered}"
        );
    }

    /// When `--display-currency` is not set, breakdown rows show native currency.
    #[test]
    fn simulate_breakdown_no_display_currency_shows_native() {
        let sim = make_sim_with_usd_resources("arch-usd");
        let all_labels = vec!["resource-a".to_string()];

        let table = render_simulate_breakdown_table(&[sim], &all_labels, None);
        let rendered = table.to_string();

        assert!(
            rendered.contains("$100.00"),
            "without --display-currency, breakdown should show native USD, got:\n{rendered}"
        );
    }
}
