//! Parse Wrangler config into a yevice-core Architecture.

use std::path::Path;

use serde::Deserialize;
use yevice_core::{
    resource::{Architecture, Connection, Provider, Resource, ResourceShell},
    types::{LogicalId, Region, ResourceType},
};

use crate::{
    error::WranglerError,
    services::{
        CloudflareD1Spec, CloudflareDurableObjectSpec, CloudflareKvSpec, CloudflareQueueSpec,
        CloudflareR2Spec, CloudflareUsageModel, CloudflareWorkerSpec,
    },
};

// ---- Raw Wrangler config deserialization ----

#[derive(Debug, Deserialize, Default)]
struct RawWrangler {
    name: Option<String>,
    #[serde(default)]
    usage_model: Option<String>,
    #[serde(default)]
    kv_namespaces: Vec<RawKvNamespace>,
    #[serde(default)]
    r2_buckets: Vec<RawR2Bucket>,
    #[serde(default)]
    d1_databases: Vec<RawD1Database>,
    #[serde(default)]
    queues: Option<RawQueues>,
    #[serde(default)]
    durable_objects: Option<RawDurableObjects>,
}

#[derive(Debug, Deserialize)]
struct RawKvNamespace {
    binding: String,
    #[serde(default)]
    #[allow(dead_code)]
    id: String,
}

#[derive(Debug, Deserialize)]
struct RawR2Bucket {
    binding: String,
    bucket_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawD1Database {
    binding: String,
    #[allow(dead_code)]
    database_id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawQueues {
    #[serde(default)]
    producers: Vec<RawQueueProducer>,
    #[serde(default)]
    consumers: Vec<RawQueueConsumer>,
}

#[derive(Debug, Deserialize)]
struct RawQueueProducer {
    #[allow(dead_code)]
    binding: String,
    queue: String,
}

#[derive(Debug, Deserialize)]
struct RawQueueConsumer {
    queue: String,
}

#[derive(Debug, Deserialize, Default)]
struct RawDurableObjects {
    #[serde(default)]
    bindings: Vec<RawDoBinding>,
}

#[derive(Debug, Deserialize)]
struct RawDoBinding {
    #[allow(dead_code)]
    name: String,
    class_name: String,
}

// ---- Public API ----

/// Parse a Wrangler config file into an Architecture.
pub fn parse_wrangler(path: &Path) -> Result<Architecture, WranglerError> {
    let content = std::fs::read_to_string(path)?;
    let default_name = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("worker");

    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonc"))
    {
        return parse_wrangler_jsonc_str(&content, default_name);
    }

    parse_wrangler_str(&content, default_name)
}

/// Parse wrangler.toml content string into an Architecture.
pub fn parse_wrangler_str(
    content: &str,
    default_name: &str,
) -> Result<Architecture, WranglerError> {
    let raw: RawWrangler = toml::from_str(content)?;
    Ok(build_architecture(&raw, default_name))
}

fn parse_wrangler_jsonc_str(
    content: &str,
    default_name: &str,
) -> Result<Architecture, WranglerError> {
    let raw = parse_wrangler_jsonc(content)?;
    Ok(build_architecture(&raw, default_name))
}

fn parse_wrangler_jsonc(content: &str) -> Result<RawWrangler, WranglerError> {
    let without_comments = strip_jsonc_comments(content);
    let normalized = strip_trailing_commas(&without_comments);
    Ok(serde_json::from_str(&normalized)?)
}

fn strip_jsonc_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;

    while let Some(ch) = chars.next() {
        if line_comment {
            if ch == '\n' {
                line_comment = false;
                result.push(ch);
            }
            continue;
        }

        if block_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block_comment = false;
            }
            continue;
        }

        if in_string {
            result.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                result.push(ch);
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                line_comment = true;
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                block_comment = true;
            }
            _ => result.push(ch),
        }
    }

    result
}

fn strip_trailing_commas(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;

    for ch in input.chars() {
        if in_string {
            result.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            result.push(ch);
            continue;
        }

        if matches!(ch, '}' | ']') {
            let mut trailing_whitespace = String::new();
            while result.chars().last().is_some_and(char::is_whitespace) {
                if let Some(last) = result.pop() {
                    trailing_whitespace.insert(0, last);
                }
            }
            if result.ends_with(',') {
                result.pop();
            }
            result.push_str(&trailing_whitespace);
        }

        result.push(ch);
    }

    result
}

fn build_architecture(raw: &RawWrangler, default_name: &str) -> Architecture {
    let worker_name = raw.name.as_deref().unwrap_or(default_name);
    let mut resources = Vec::<Resource>::new();

    // Worker itself
    let usage_model = match raw.usage_model.as_deref() {
        Some("bundled") => CloudflareUsageModel::Bundled,
        _ => CloudflareUsageModel::Standard,
    };
    resources.push(Resource {
        logical_id: LogicalId::new(worker_name),
        resource_type: ResourceType::new("cloudflare_worker"),
        shell: ResourceShell::new(
            "cloudflare.worker",
            Provider::Cloudflare,
            &CloudflareWorkerSpec { usage_model },
        ),
    });

    // KV namespaces
    for kv in &raw.kv_namespaces {
        resources.push(Resource {
            logical_id: LogicalId::new(format!("{}_kv_{}", worker_name, kv.binding.to_lowercase())),
            resource_type: ResourceType::new("cloudflare_workers_kv_namespace"),
            shell: ResourceShell::new("cloudflare.kv", Provider::Cloudflare, &CloudflareKvSpec {}),
        });
    }

    // R2 buckets
    for r2 in &raw.r2_buckets {
        let bucket = r2.bucket_name.as_deref().unwrap_or(&r2.binding);
        resources.push(Resource {
            logical_id: LogicalId::new(format!("{}_r2_{}", worker_name, bucket.to_lowercase())),
            resource_type: ResourceType::new("cloudflare_r2_bucket"),
            shell: ResourceShell::new("cloudflare.r2", Provider::Cloudflare, &CloudflareR2Spec {}),
        });
    }

    // D1 databases
    for d1 in &raw.d1_databases {
        resources.push(Resource {
            logical_id: LogicalId::new(format!("{}_d1_{}", worker_name, d1.binding.to_lowercase())),
            resource_type: ResourceType::new("cloudflare_d1_database"),
            shell: ResourceShell::new("cloudflare.d1", Provider::Cloudflare, &CloudflareD1Spec {}),
        });
    }

    // Queues (deduplicate by queue name)
    if let Some(queues) = &raw.queues {
        let mut seen = std::collections::HashSet::new();
        for producer in &queues.producers {
            if seen.insert(&producer.queue) {
                resources.push(Resource {
                    logical_id: LogicalId::new(format!(
                        "{}_queue_{}",
                        worker_name,
                        producer.queue.to_lowercase().replace('-', "_")
                    )),
                    resource_type: ResourceType::new("cloudflare_queue"),
                    shell: ResourceShell::new(
                        "cloudflare.queue",
                        Provider::Cloudflare,
                        &CloudflareQueueSpec {},
                    ),
                });
            }
        }
        for consumer in &queues.consumers {
            if seen.insert(&consumer.queue) {
                resources.push(Resource {
                    logical_id: LogicalId::new(format!(
                        "{}_queue_{}",
                        worker_name,
                        consumer.queue.to_lowercase().replace('-', "_")
                    )),
                    resource_type: ResourceType::new("cloudflare_queue"),
                    shell: ResourceShell::new(
                        "cloudflare.queue",
                        Provider::Cloudflare,
                        &CloudflareQueueSpec {},
                    ),
                });
            }
        }
    }

    // Durable Objects (one resource per unique class)
    if let Some(dos) = &raw.durable_objects {
        let mut seen_classes = std::collections::HashSet::new();
        for binding in &dos.bindings {
            if seen_classes.insert(&binding.class_name) {
                resources.push(Resource {
                    logical_id: LogicalId::new(format!(
                        "{}_do_{}",
                        worker_name,
                        binding.class_name.to_lowercase()
                    )),
                    resource_type: ResourceType::new("cloudflare_durable_object"),
                    shell: ResourceShell::new(
                        "cloudflare.durable_object",
                        Provider::Cloudflare,
                        &CloudflareDurableObjectSpec {},
                    ),
                });
            }
        }
    }

    Architecture {
        name: worker_name.to_string(),
        region: Region::new("global"),
        resources,
        connections: Vec::<Connection>::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonc_wranger_config() {
        let content = r#"
        {
          // Worker metadata
          "name": "edge-worker",
          "usage_model": "bundled",
          "kv_namespaces": [
            {
              "binding": "CACHE",
              "id": "kv-id",
            }
          ],
          "queues": {
            "producers": [
              {
                "binding": "JOBS",
                "queue": "jobs",
              }
            ]
          }
        }
        "#;

        let architecture = parse_wrangler_jsonc_str(content, "fallback").unwrap();
        assert_eq!(architecture.name, "edge-worker");
        assert_eq!(architecture.region, "global");
        assert_eq!(architecture.resources.len(), 3);
    }
}
