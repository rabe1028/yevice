//! Table-rendering helpers extracted from the command implementations.
//!
//! Each function constructs a [`comfy_table::Table`] from structured data and
//! returns it ready for printing.  The calling command function calls
//! `println!("{table}")` — no formatting logic lives outside this module.

use comfy_table::{Cell, Color, Table, presets::UTF8_FULL};
use yevice_core::capacity::{Severity, Violation};
use yevice_core::evaluate::ArchitectureResult;
use yevice_core::simulate::ArchSimulation;

// ---------------------------------------------------------------------------
// eval
// ---------------------------------------------------------------------------

/// Build the per-resource cost table for `eval --breakdown`.
///
/// Each resource appears as a coloured row followed by indented component rows.
/// A green TOTAL row is appended at the bottom.
pub(crate) fn render_eval_breakdown_table(result: &ArchitectureResult) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Resource / Component", "Monthly Cost (USD)"]);

    for r in &result.resources {
        table.add_row(vec![
            Cell::new(&r.label).fg(Color::Cyan),
            Cell::new(format!("${:.2}", r.monthly_cost.value)).fg(Color::Cyan),
        ]);
        for (name, cost) in &r.component_costs {
            table.add_row(vec![
                Cell::new(format!("  └─ {name}")),
                Cell::new(format!("${:.4}", cost.value)),
            ]);
        }
    }

    table.add_row(vec![
        Cell::new("TOTAL").fg(Color::Green),
        Cell::new(format!("${:.2}", result.naive_total())).fg(Color::Green),
    ]);

    table
}

/// Build the simple (no-breakdown) per-resource cost table for `eval`.
///
/// Each resource appears on one row with label, type, and monthly cost.
/// A green TOTAL row is appended at the bottom.
pub(crate) fn render_eval_table(result: &ArchitectureResult) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Resource", "Type", "Monthly Cost (USD)"]);

    for r in &result.resources {
        table.add_row(vec![
            Cell::new(&r.label),
            Cell::new(&r.resource_type),
            Cell::new(format!("${:.2}", r.monthly_cost.value)),
        ]);
    }

    table.add_row(vec![
        Cell::new("TOTAL").fg(Color::Green),
        Cell::new(""),
        Cell::new(format!("${:.2}", result.naive_total())).fg(Color::Green),
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

    // Total row
    let mut total_row = vec![Cell::new("Total Monthly Cost").fg(Color::Green)];
    for r in results {
        total_row.push(Cell::new(format!("${:.2}", r.naive_total())));
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
                .map_or_else(
                    || "-".to_string(),
                    |res| format!("${:.2}", res.monthly_cost.value),
                );
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
                        .map_or_else(|| "-".to_string(), |(_, v)| format!("${:.4}", v.value));
                    comp_row.push(Cell::new(comp_cost));
                }
                summary.add_row(comp_row);
            }
        }
    }

    // Difference row (only when exactly 2 architectures are compared).
    if results.len() == 2 {
        let diff = results[1].naive_total() - results[0].naive_total();
        let diff_str = if diff >= 0.0 {
            format!("+${diff:.2}")
        } else {
            format!("-${:.2}", diff.abs())
        };
        let color = if diff > 0.0 { Color::Red } else { Color::Green };
        summary.add_row(vec![
            Cell::new("Difference"),
            Cell::new("-"),
            Cell::new(diff_str).fg(color),
        ]);
    }

    summary
}

// ---------------------------------------------------------------------------
// sensitivity
// ---------------------------------------------------------------------------

/// One row of data for the sensitivity sweep table.
pub(crate) enum SensitivityRow {
    Ok { value: f64, total: f64, delta: f64 },
    Err { value: f64, message: String },
}

/// Build the main sensitivity sweep table for `sensitivity`.
///
/// One row per step: the varied variable's value, total monthly cost, and
/// coloured delta from the base cost.
pub(crate) fn render_sensitivity_table(var_name: &str, rows: &[SensitivityRow]) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec![
        Cell::new(var_name),
        Cell::new("Total Monthly Cost"),
        Cell::new("Delta from Base"),
    ]);

    for row in rows {
        match row {
            SensitivityRow::Ok {
                value,
                total,
                delta,
            } => {
                let delta_str = if *delta >= 0.0 {
                    format!("+${delta:.2}")
                } else {
                    format!("-${:.2}", delta.abs())
                };
                let color = if *delta > 0.0 {
                    Color::Red
                } else if *delta < 0.0 {
                    Color::Green
                } else {
                    Color::White
                };
                table.add_row(vec![
                    Cell::new(format_number(*value)),
                    Cell::new(format!("${total:.2}")),
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
pub(crate) fn render_sensitivity_breakdown_table(
    var_name: &str,
    resource_labels: &[String],
    breakdown_rows: &[(f64, Vec<f64>)],
) -> Table {
    let mut bd_table = Table::new();
    bd_table.load_preset(UTF8_FULL);

    let mut bd_header = vec![Cell::new(var_name)];
    for label in resource_labels {
        bd_header.push(Cell::new(label));
    }
    bd_table.set_header(bd_header);

    for (value, costs) in breakdown_rows {
        let mut row = vec![Cell::new(format_number(*value))];
        for cost in costs {
            row.push(Cell::new(format!("${cost:.2}")));
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

/// Build the hourly load simulation table for `simulate`.
pub(crate) fn render_simulate_table(
    arch_results: &[ArchSimulation],
    multiplier_at: impl Fn(u32) -> f64,
) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);

    let mut header = vec![Cell::new("Hour"), Cell::new("Multiplier")];
    for sim in arch_results {
        header.push(Cell::new(format!("{} (rate/mo)", sim.name)));
    }
    table.set_header(header);

    for hour in 0..24 {
        let mult = multiplier_at(hour);
        let mut row = vec![
            Cell::new(format!("{hour:02}:00")),
            Cell::new(format!("{mult:.2}x")),
        ];
        for sim in arch_results {
            let cost = sim
                .hourly_costs
                .iter()
                .find(|(h, _)| *h == hour)
                .map_or(0.0, |(_, c)| *c);
            row.push(Cell::new(format!("${cost:.2}")));
        }
        table.add_row(row);
    }

    // Total row
    let mut total_row = vec![Cell::new("MONTHLY TOTAL").fg(Color::Green), Cell::new("")];
    for sim in arch_results {
        total_row.push(Cell::new(format!("${:.2}", sim.total_monthly_cost)).fg(Color::Green));
    }
    table.add_row(total_row);

    table
}

/// Build the resource breakdown table for `simulate --breakdown`.
pub(crate) fn render_simulate_breakdown_table(
    arch_results: &[ArchSimulation],
    all_labels: &[String],
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
                .map_or_else(|| "-".to_string(), |(_, c)| format!("${c:.2}"));
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
