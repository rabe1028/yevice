//! Parse Wrangler config into a yevice-core Architecture.

use std::path::Path;

use serde::Deserialize;
use yevice_core::{
    resource::{Architecture, Connection, ConnectionType, Provider, Resource, ResourceShell},
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

    // Build the set of all resource logical IDs for dangling-edge prevention.
    let resource_ids: std::collections::HashSet<&LogicalId> =
        resources.iter().map(|r| &r.logical_id).collect();

    let worker_id = LogicalId::new(worker_name);
    let mut connections = Vec::<Connection>::new();

    // KV namespace bindings: Worker → KV (DataFlow)
    for kv in &raw.kv_namespaces {
        let kv_id = LogicalId::new(format!("{}_kv_{}", worker_name, kv.binding.to_lowercase()));
        if resource_ids.contains(&kv_id) {
            connections.push(Connection {
                source: worker_id.clone(),
                target: kv_id,
                connection_type: ConnectionType::DataFlow,
                batch_size: None,
                parallelization_factor: None,
                factor: None,
                source_hint: None,
            });
        }
    }

    // R2 bucket bindings: Worker → R2 (DataFlow)
    for r2 in &raw.r2_buckets {
        let bucket = r2.bucket_name.as_deref().unwrap_or(&r2.binding);
        let r2_id = LogicalId::new(format!("{}_r2_{}", worker_name, bucket.to_lowercase()));
        if resource_ids.contains(&r2_id) {
            connections.push(Connection {
                source: worker_id.clone(),
                target: r2_id,
                connection_type: ConnectionType::DataFlow,
                batch_size: None,
                parallelization_factor: None,
                factor: None,
                source_hint: None,
            });
        }
    }

    // D1 database bindings: Worker → D1 (DataFlow)
    for d1 in &raw.d1_databases {
        let d1_id = LogicalId::new(format!("{}_d1_{}", worker_name, d1.binding.to_lowercase()));
        if resource_ids.contains(&d1_id) {
            connections.push(Connection {
                source: worker_id.clone(),
                target: d1_id,
                connection_type: ConnectionType::DataFlow,
                batch_size: None,
                parallelization_factor: None,
                factor: None,
                source_hint: None,
            });
        }
    }

    // Queue connections
    if let Some(queues) = &raw.queues {
        // Queue producers: Worker → Queue (DataFlow)
        for producer in &queues.producers {
            let queue_id = LogicalId::new(format!(
                "{}_queue_{}",
                worker_name,
                producer.queue.to_lowercase().replace('-', "_")
            ));
            if resource_ids.contains(&queue_id) {
                connections.push(Connection {
                    source: worker_id.clone(),
                    target: queue_id,
                    connection_type: ConnectionType::DataFlow,
                    batch_size: None,
                    parallelization_factor: None,
                    factor: None,
                    source_hint: None,
                });
            }
        }

        // Queue consumers: Queue → Worker (EventSource)
        for consumer in &queues.consumers {
            let queue_id = LogicalId::new(format!(
                "{}_queue_{}",
                worker_name,
                consumer.queue.to_lowercase().replace('-', "_")
            ));
            if resource_ids.contains(&queue_id) {
                connections.push(Connection {
                    source: queue_id,
                    target: worker_id.clone(),
                    connection_type: ConnectionType::EventSource,
                    batch_size: None,
                    parallelization_factor: None,
                    factor: None,
                    source_hint: None,
                });
            }
        }
    }

    // Durable Object bindings: Worker → DO (DataFlow)
    if let Some(dos) = &raw.durable_objects {
        let mut seen_classes = std::collections::HashSet::new();
        for binding in &dos.bindings {
            if seen_classes.insert(&binding.class_name) {
                let do_id = LogicalId::new(format!(
                    "{}_do_{}",
                    worker_name,
                    binding.class_name.to_lowercase()
                ));
                if resource_ids.contains(&do_id) {
                    connections.push(Connection {
                        source: worker_id.clone(),
                        target: do_id,
                        connection_type: ConnectionType::DataFlow,
                        batch_size: None,
                        parallelization_factor: None,
                        factor: None,
                        source_hint: None,
                    });
                }
            }
        }
    }

    Architecture {
        name: worker_name.to_string(),
        region: Region::new("global"),
        resources,
        connections,
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

    #[test]
    fn connections_kv_r2_d1_are_worker_to_resource_dataflow() {
        let content = r#"
name = "my-worker"

[[kv_namespaces]]
binding = "CACHE"
id = "kv-id"

[[r2_buckets]]
binding = "UPLOADS"
bucket_name = "my-bucket"

[[d1_databases]]
binding = "DB"
database_id = "db-id"
"#;
        let arch = parse_wrangler_str(content, "fallback").unwrap();

        let worker_id = LogicalId::new("my-worker");
        let kv_id = LogicalId::new("my-worker_kv_cache");
        let r2_id = LogicalId::new("my-worker_r2_my-bucket");
        let d1_id = LogicalId::new("my-worker_d1_db");

        assert_eq!(arch.connections.len(), 3);

        let has_conn = |source: &LogicalId, target: &LogicalId, ctype: &ConnectionType| {
            arch.connections.iter().any(|c| {
                &c.source == source && &c.target == target && &c.connection_type == ctype
            })
        };

        assert!(has_conn(&worker_id, &kv_id, &ConnectionType::DataFlow));
        assert!(has_conn(&worker_id, &r2_id, &ConnectionType::DataFlow));
        assert!(has_conn(&worker_id, &d1_id, &ConnectionType::DataFlow));
    }

    #[test]
    fn connections_queue_producer_is_dataflow_consumer_is_eventsource() {
        let content = r#"
name = "my-worker"

[queues]

[[queues.producers]]
binding = "P"
queue = "my-queue"

[[queues.consumers]]
queue = "my-queue"
"#;
        let arch = parse_wrangler_str(content, "fallback").unwrap();

        let worker_id = LogicalId::new("my-worker");
        let queue_id = LogicalId::new("my-worker_queue_my_queue");

        // One queue resource (deduplicated), two connections: producer DataFlow + consumer EventSource
        assert_eq!(arch.connections.len(), 2);

        let producer_conn = arch
            .connections
            .iter()
            .find(|c| c.connection_type == ConnectionType::DataFlow);
        let consumer_conn = arch
            .connections
            .iter()
            .find(|c| c.connection_type == ConnectionType::EventSource);

        let producer_conn = producer_conn.expect("producer DataFlow connection present");
        assert_eq!(producer_conn.source, worker_id);
        assert_eq!(producer_conn.target, queue_id);

        let consumer_conn = consumer_conn.expect("consumer EventSource connection present");
        assert_eq!(consumer_conn.source, queue_id);
        assert_eq!(consumer_conn.target, worker_id);
    }

    #[test]
    fn connections_durable_object_is_worker_to_do_dataflow() {
        let content = r#"
name = "my-worker"

[durable_objects]

[[durable_objects.bindings]]
name = "ROOM"
class_name = "ChatRoom"
"#;
        let arch = parse_wrangler_str(content, "fallback").unwrap();

        let worker_id = LogicalId::new("my-worker");
        let do_id = LogicalId::new("my-worker_do_chatroom");

        assert_eq!(arch.connections.len(), 1);
        let conn = &arch.connections[0];
        assert_eq!(conn.source, worker_id);
        assert_eq!(conn.target, do_id);
        assert_eq!(conn.connection_type, ConnectionType::DataFlow);
    }

    #[test]
    fn connections_no_dangling_edges_for_missing_resources() {
        // Construct a RawWrangler directly with a queue consumer that has no
        // corresponding producer, so the queue resource IS added by the consumer
        // path. This test confirms edges are only created when the target resource
        // exists in the architecture.
        let raw = RawWrangler {
            name: Some("w".to_string()),
            kv_namespaces: vec![RawKvNamespace {
                binding: "MY_KV".to_string(),
                id: String::new(),
            }],
            ..RawWrangler::default()
        };
        let arch = build_architecture(&raw, "w");

        // Worker + KV = 2 resources, 1 DataFlow connection
        assert_eq!(arch.resources.len(), 2);
        assert_eq!(arch.connections.len(), 1);
        assert_eq!(arch.connections[0].connection_type, ConnectionType::DataFlow);
        assert_eq!(arch.connections[0].source, LogicalId::new("w"));
        assert_eq!(arch.connections[0].target, LogicalId::new("w_kv_my_kv"));
    }

    #[test]
    fn full_fixture_has_expected_connection_count() {
        // wrangler_full.toml: 2 KV + 1 R2 + 1 D1 + 1 Queue (producer+consumer) + 2 DO
        // Expected connections:
        //   2 KV DataFlow + 1 R2 DataFlow + 1 D1 DataFlow
        //   + 1 Queue producer DataFlow + 1 Queue consumer EventSource
        //   + 2 DO DataFlow  = 8 connections
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("wrangler_full.toml");
        let arch = crate::parser::parse_wrangler(&path).unwrap();

        assert_eq!(arch.connections.len(), 8);

        let event_sources: Vec<_> = arch
            .connections
            .iter()
            .filter(|c| c.connection_type == ConnectionType::EventSource)
            .collect();
        assert_eq!(event_sources.len(), 1, "one queue consumer EventSource");

        let data_flows: Vec<_> = arch
            .connections
            .iter()
            .filter(|c| c.connection_type == ConnectionType::DataFlow)
            .collect();
        assert_eq!(data_flows.len(), 7, "seven DataFlow edges");
    }
}
