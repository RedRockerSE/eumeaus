//! Scan orchestration (M4, SPEC.md §2.1/§4.2): worker-pool-bounded
//! concurrent plugin invocation, rate limiting, scan-state persistence and
//! resumability, and result ingestion through the same auto-merge path
//! [`crate::crud`] uses for manual entry.
//!
//! `eumeaus-plugin-host`'s API is async (gRPC/subprocess I/O); the rest of
//! the engine is sync (`rusqlite`). [`start`]/[`resume`] bridge the two by
//! owning a single-use `tokio::runtime::Runtime` and `block_on`-ing the
//! orchestration to completion — this is a one-shot CLI process, not a
//! daemon, so "start a scan" already means "block until it's done or the
//! process dies," matching SPEC.md §6's "kill the process mid-scan" model.
//! Only the *dispatched-to-a-plugin* work runs concurrently
//! (`tokio::spawn`, bounded by `worker_pool`); every DB write happens
//! sequentially in the orchestrating task itself, so `rusqlite::Connection`
//! (which is `!Sync`) never needs to cross a task boundary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ed25519_dalek::VerifyingKey;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use uuid::Uuid;

use eumeaus_plugin_host::{
    CheckRequest, CheckResult, PluginError as HostError, PluginHost, PluginManifest,
    RateLimitConfig, TrustPolicy,
};

use crate::{
    crud, now_unix_ms, Attribute, ConfidenceStatus, EngineError, Entity, EntityId, EntityType,
    Provenance, RelationshipType, ScanConfig, ScanId, ScanStatus, TargetEntity,
};

const DEFAULT_WORKER_POOL: u32 = 4;

/// Everything needed to resume a scan later, since `resume_scan(scan_id)`
/// (SPEC.md §3.1) takes no other arguments — persisted as JSON in
/// `scans.config_snapshot`.
#[derive(Debug, Serialize, Deserialize)]
struct ConfigSnapshot {
    plugins_dir: PathBuf,
    worker_pool: u32,
    rate_limit_per_sec: Option<u32>,
    proxy: Option<String>,
    trust_policy: TrustPolicySnapshot,
}

#[derive(Debug, Serialize, Deserialize)]
enum TrustPolicySnapshot {
    AllowUnsigned,
    RequireSignature { trusted_key_hex: String },
}

fn snapshot_trust_policy(policy: &TrustPolicy) -> TrustPolicySnapshot {
    match policy {
        TrustPolicy::AllowUnsigned => TrustPolicySnapshot::AllowUnsigned,
        TrustPolicy::RequireSignature { trusted_key } => TrustPolicySnapshot::RequireSignature {
            trusted_key_hex: to_hex(&trusted_key.to_bytes()),
        },
    }
}

fn restore_trust_policy(snapshot: &TrustPolicySnapshot) -> Result<TrustPolicy, EngineError> {
    Ok(match snapshot {
        TrustPolicySnapshot::AllowUnsigned => TrustPolicy::AllowUnsigned,
        TrustPolicySnapshot::RequireSignature { trusted_key_hex } => {
            let bytes = from_hex(trusted_key_hex)?;
            let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
                EngineError::InvalidScanConfig("stored trusted key is not 32 bytes".to_string())
            })?;
            let trusted_key = VerifyingKey::from_bytes(&bytes)
                .map_err(|e| EngineError::InvalidScanConfig(e.to_string()))?;
            TrustPolicy::RequireSignature { trusted_key }
        }
    })
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>, EngineError> {
    if !s.len().is_multiple_of(2) {
        return Err(EngineError::InvalidScanConfig(
            "odd-length hex string".to_string(),
        ));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|e| EngineError::InvalidScanConfig(e.to_string()))
        })
        .collect()
}

/// Starts a new scan: resolves the target entity, discovers plugins in
/// `plugins_dir` (filtered to `plugin_names` if non-empty, else every
/// discovered plugin whose `contract.input_entity_types` includes the
/// target's entity type), persists `scans`/`scan_plugin_runs` rows, and
/// runs them all to completion (bounded by `config.worker_pool`) before
/// returning.
///
/// Deviates from SPEC.md §3.1's `start_scan(plugin: PluginRef, ...)`
/// (singular): `ScanConfig.worker_pool` only means something if a scan can
/// invoke more than one plugin at once, so this takes `plugin_names`
/// (plural) instead. `plugins_dir`/`trust_policy` aren't in the
/// illustrative signature either — SPEC.md never resolves where a scan's
/// plugin list or trust key comes from, so this makes them explicit
/// parameters here and persists both in `config_snapshot` for
/// [`resume`], which does keep the spec's `resume_scan(scan_id)` shape.
pub(crate) fn start(
    conn: &mut Connection,
    plugins_dir: &Path,
    plugin_names: &[String],
    target: TargetEntity,
    config: ScanConfig,
    trust_policy: TrustPolicy,
) -> Result<ScanId, EngineError> {
    let target_entity = crud::get_entity(conn, target.id)?;

    let manifests = eumeaus_plugin_host::PluginHost::discover(plugins_dir)?;
    let selected = select_plugins(manifests, plugin_names, &target_entity.entity_type);
    if selected.is_empty() {
        return Err(EngineError::NoCompatiblePlugins(
            plugins_dir.to_path_buf(),
            target_entity.entity_type.to_string(),
        ));
    }

    let scan_id = Uuid::new_v4();
    let worker_pool = config.worker_pool.unwrap_or(DEFAULT_WORKER_POOL).max(1);
    let snapshot = ConfigSnapshot {
        plugins_dir: plugins_dir.to_path_buf(),
        worker_pool,
        rate_limit_per_sec: config.rate_limit_per_sec,
        proxy: config.proxy.clone(),
        trust_policy: snapshot_trust_policy(&trust_policy),
    };
    let now = now_unix_ms();

    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO scans (id, target_entity_id, config_snapshot, status, started_at, completed_at)
         VALUES (?1, ?2, ?3, 'RUNNING', ?4, NULL)",
        params![
            scan_id.to_string(),
            target.id.0.to_string(),
            serde_json::to_string(&snapshot)?,
            now,
        ],
    )?;
    {
        let mut insert_run = tx.prepare(
            "INSERT INTO scan_plugin_runs (id, scan_id, plugin_name, plugin_version, status, started_at, completed_at, error_message)
             VALUES (?1, ?2, ?3, ?4, 'PENDING', NULL, NULL, NULL)",
        )?;
        for manifest in &selected {
            insert_run.execute(params![
                Uuid::new_v4().to_string(),
                scan_id.to_string(),
                manifest.plugin.name,
                manifest.plugin.version,
            ])?;
        }
    }
    tx.commit()?;

    run_to_completion(
        conn,
        scan_id,
        &target_entity,
        selected,
        worker_pool,
        config.rate_limit_per_sec,
        trust_policy,
    )?;
    finalize_scan_status(conn, scan_id)?;

    Ok(ScanId(scan_id))
}

/// Re-runs only the `scan_plugin_runs` still `PENDING` for `scan_id` — not
/// the ones already `SUCCESS`, `TIMEOUT`, or `ERROR` (SPEC.md §5's "`scan
/// resume` picks up only the incomplete work"). A crashed prior run's
/// `RUNNING` rows are reconciled to `PENDING` on [`crate::Case::open`],
/// before this ever sees them.
pub(crate) fn resume(conn: &mut Connection, scan_id: Uuid) -> Result<(), EngineError> {
    let (target_entity_id, config_json): (String, String) = conn
        .query_row(
            "SELECT target_entity_id, config_snapshot FROM scans WHERE id = ?1",
            params![scan_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or(EngineError::ScanNotFound(ScanId(scan_id)))?;

    let snapshot: ConfigSnapshot = serde_json::from_str(&config_json)?;
    let trust_policy = restore_trust_policy(&snapshot.trust_policy)?;
    let target_entity = crud::get_entity(
        conn,
        EntityId(Uuid::parse_str(&target_entity_id).expect("stored entity id is a valid uuid")),
    )?;

    let pending_names: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT plugin_name FROM scan_plugin_runs WHERE scan_id = ?1 AND status = 'PENDING'",
        )?;
        let names = stmt
            .query_map(params![scan_id.to_string()], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        names
    };
    if pending_names.is_empty() {
        finalize_scan_status(conn, scan_id)?;
        return Ok(());
    }

    let manifests = eumeaus_plugin_host::PluginHost::discover(&snapshot.plugins_dir)?;
    let selected = select_plugins(manifests, &pending_names, &target_entity.entity_type);

    // A plugin required by a pending run may have been uninstalled since
    // the scan started; that run can never complete on its own, so it's a
    // per-plugin ERROR (SPEC.md §5's "one bad plugin never aborts a
    // scan"), not a reason to fail the whole resume.
    let found_names: std::collections::HashSet<&str> =
        selected.iter().map(|m| m.plugin.name.as_str()).collect();
    for name in &pending_names {
        if !found_names.contains(name.as_str()) {
            set_run_status(
                conn,
                scan_id,
                name,
                "ERROR",
                Some(now_unix_ms()),
                Some("plugin no longer discoverable/compatible at resume time"),
            )?;
        }
    }

    conn.execute(
        "UPDATE scans SET status = 'RUNNING' WHERE id = ?1",
        params![scan_id.to_string()],
    )?;

    run_to_completion(
        conn,
        scan_id,
        &target_entity,
        selected,
        snapshot.worker_pool,
        snapshot.rate_limit_per_sec,
        trust_policy,
    )?;
    finalize_scan_status(conn, scan_id)?;
    Ok(())
}

pub(crate) fn status(conn: &Connection, scan_id: Uuid) -> Result<ScanStatus, EngineError> {
    let status: String = conn
        .query_row(
            "SELECT status FROM scans WHERE id = ?1",
            params![scan_id.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(EngineError::ScanNotFound(ScanId(scan_id)))?;
    status.parse()
}

/// On [`crate::Case::open`]: any `scan_plugin_runs` row still `RUNNING` was
/// left there by a process that died before finishing it — this is a
/// single-process, non-daemon CLI, so nothing else could still be running
/// it. Reset to `PENDING` so `resume_scan` picks it back up (SPEC.md §4.2).
pub(crate) fn reconcile_orphaned_runs(conn: &Connection) -> Result<(), EngineError> {
    conn.execute(
        "UPDATE scan_plugin_runs SET status = 'PENDING' WHERE status = 'RUNNING'",
        [],
    )?;
    Ok(())
}

fn select_plugins(
    manifests: Vec<PluginManifest>,
    plugin_names: &[String],
    target_entity_type: &EntityType,
) -> Vec<PluginManifest> {
    let target_type_str = target_entity_type.to_string();
    manifests
        .into_iter()
        .filter(|m| {
            if plugin_names.is_empty() {
                m.contract.input_entity_types.contains(&target_type_str)
            } else {
                plugin_names.iter().any(|n| n == &m.plugin.name)
            }
        })
        .collect()
}

struct PluginOutcome {
    plugin_name: String,
    plugin_version: String,
    result: Result<Vec<CheckResult>, HostError>,
}

/// Runs `manifests` to completion against `target_entity`, up to
/// `worker_pool` concurrently in flight at once, pacing new dispatches to
/// `rate_limit_per_sec` (a simple fixed-interval throttle — not a true
/// token bucket, but sufficient to keep a scan from opening every plugin's
/// connection in the same instant). Every `scan_plugin_runs` transition
/// and every successful result's ingestion happens synchronously in this
/// task, between `.await` points, never inside a spawned one.
fn run_to_completion(
    conn: &mut Connection,
    scan_id: Uuid,
    target_entity: &Entity,
    manifests: Vec<PluginManifest>,
    worker_pool: u32,
    rate_limit_per_sec: Option<u32>,
    trust_policy: TrustPolicy,
) -> Result<(), EngineError> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(run_to_completion_async(
        conn,
        scan_id,
        target_entity,
        manifests,
        worker_pool as usize,
        rate_limit_per_sec,
        trust_policy,
    ))
}

async fn run_to_completion_async(
    conn: &mut Connection,
    scan_id: Uuid,
    target_entity: &Entity,
    manifests: Vec<PluginManifest>,
    worker_pool: usize,
    rate_limit_per_sec: Option<u32>,
    trust_policy: TrustPolicy,
) -> Result<(), EngineError> {
    let min_interval = rate_limit_per_sec
        .filter(|&n| n > 0)
        .map(|n| Duration::from_secs_f64(1.0 / n as f64));
    let mut last_dispatch: Option<Instant> = None;

    let mut join_set: JoinSet<PluginOutcome> = JoinSet::new();
    let mut pending = manifests.into_iter();

    let mut dispatch_next = |conn: &mut Connection,
                             join_set: &mut JoinSet<PluginOutcome>,
                             last_dispatch: &mut Option<Instant>|
     -> Result<bool, EngineError> {
        let Some(manifest) = pending.next() else {
            return Ok(false);
        };
        set_run_status(conn, scan_id, &manifest.plugin.name, "RUNNING", None, None)?;
        let request = build_check_request(scan_id, target_entity, &manifest);
        *last_dispatch = Some(Instant::now());
        dispatch(join_set, manifest, request, trust_policy.clone());
        Ok(true)
    };

    for _ in 0..worker_pool {
        if let Some(interval) = min_interval {
            if let Some(prev) = last_dispatch {
                let elapsed = prev.elapsed();
                if elapsed < interval {
                    tokio::time::sleep(interval - elapsed).await;
                }
            }
        }
        if !dispatch_next(conn, &mut join_set, &mut last_dispatch)? {
            break;
        }
    }

    while let Some(joined) = join_set.join_next().await {
        let outcome = joined.expect("plugin worker task panicked");
        handle_outcome(conn, scan_id, target_entity, outcome)?;

        if let Some(interval) = min_interval {
            if let Some(prev) = last_dispatch {
                let elapsed = prev.elapsed();
                if elapsed < interval {
                    tokio::time::sleep(interval - elapsed).await;
                }
            }
        }
        dispatch_next(conn, &mut join_set, &mut last_dispatch)?;
    }

    Ok(())
}

fn build_check_request(
    scan_id: Uuid,
    target_entity: &Entity,
    manifest: &PluginManifest,
) -> CheckRequest {
    CheckRequest {
        scan_id: scan_id.to_string(),
        input_entity_type: target_entity.entity_type.to_string(),
        input_value: target_entity
            .canonical_key
            .clone()
            .unwrap_or_else(|| target_entity.display_label.clone()),
        resolved_credentials: HashMap::new(),
        rate_limit: Some(RateLimitConfig {
            requests_per_sec: manifest.execution.default_rate_limit_per_sec,
            timeout_ms: manifest.execution.default_timeout_ms as u32,
        }),
    }
}

fn dispatch(
    join_set: &mut JoinSet<PluginOutcome>,
    manifest: PluginManifest,
    request: CheckRequest,
    trust_policy: TrustPolicy,
) {
    let plugin_name = manifest.plugin.name.clone();
    let plugin_version = manifest.plugin.version.clone();
    join_set.spawn(async move {
        let result = invoke_one(manifest, request, trust_policy).await;
        PluginOutcome {
            plugin_name,
            plugin_version,
            result,
        }
    });
}

async fn invoke_one(
    manifest: PluginManifest,
    request: CheckRequest,
    trust_policy: TrustPolicy,
) -> Result<Vec<CheckResult>, HostError> {
    let mut host = PluginHost;
    let handle = host.load(&manifest, trust_policy).await?;
    let result = host.invoke(&handle, request).await;
    let _ = host.shutdown(handle).await;
    result
}

fn handle_outcome(
    conn: &mut Connection,
    scan_id: Uuid,
    target_entity: &Entity,
    outcome: PluginOutcome,
) -> Result<(), EngineError> {
    let now = now_unix_ms();
    match outcome.result {
        Ok(results) => {
            for result in &results {
                ingest_check_result(
                    conn,
                    scan_id,
                    &outcome.plugin_name,
                    &outcome.plugin_version,
                    target_entity,
                    result,
                )?;
            }
            set_run_status(
                conn,
                scan_id,
                &outcome.plugin_name,
                "SUCCESS",
                Some(now),
                None,
            )
        }
        Err(HostError::Timeout(_, _)) => set_run_status(
            conn,
            scan_id,
            &outcome.plugin_name,
            "TIMEOUT",
            Some(now),
            Some("plugin invocation timed out"),
        ),
        Err(e) => {
            let message = e.to_string();
            set_run_status(
                conn,
                scan_id,
                &outcome.plugin_name,
                "ERROR",
                Some(now),
                Some(&message),
            )
        }
    }
}

fn set_run_status(
    conn: &Connection,
    scan_id: Uuid,
    plugin_name: &str,
    status: &str,
    completed_at: Option<i64>,
    error_message: Option<&str>,
) -> Result<(), EngineError> {
    if status == "RUNNING" {
        conn.execute(
            "UPDATE scan_plugin_runs SET status = ?1, started_at = ?2
             WHERE scan_id = ?3 AND plugin_name = ?4",
            params![status, now_unix_ms(), scan_id.to_string(), plugin_name],
        )?;
    } else {
        conn.execute(
            "UPDATE scan_plugin_runs SET status = ?1, completed_at = ?2, error_message = ?3
             WHERE scan_id = ?4 AND plugin_name = ?5",
            params![
                status,
                completed_at,
                error_message,
                scan_id.to_string(),
                plugin_name
            ],
        )?;
    }
    Ok(())
}

/// Sets `scans.status` from the terminal distribution of its
/// `scan_plugin_runs`: `COMPLETED` if every run succeeded, `PARTIALLY_FAILED`
/// if at least one didn't, still-`PENDING` rows mean this scan isn't done
/// (stays `RUNNING`, ready for `resume_scan`).
fn finalize_scan_status(conn: &Connection, scan_id: Uuid) -> Result<(), EngineError> {
    let (pending, failed): (i64, i64) = conn.query_row(
        "SELECT
            SUM(CASE WHEN status IN ('PENDING', 'RUNNING') THEN 1 ELSE 0 END),
            SUM(CASE WHEN status IN ('TIMEOUT', 'ERROR') THEN 1 ELSE 0 END)
         FROM scan_plugin_runs WHERE scan_id = ?1",
        params![scan_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    if pending > 0 {
        return Ok(());
    }
    let status = if failed > 0 {
        "PARTIALLY_FAILED"
    } else {
        "COMPLETED"
    };
    conn.execute(
        "UPDATE scans SET status = ?1, completed_at = ?2 WHERE id = ?3",
        params![status, now_unix_ms(), scan_id.to_string()],
    )?;
    Ok(())
}

/// Converts one plugin's `CheckResult` into facts through the same
/// auto-merge path manual entry uses ([`crud::add_entity_from_scan`]) —
/// `entities` first (so later `relationships` can resolve their endpoints
/// by canonical key), falling back to the scan's own target entity when a
/// relationship's endpoint matches *its* canonical key instead of one from
/// this result. An endpoint matching neither is a plugin bug — SPEC.md §5
/// says no single bad plugin aborts a scan, so this only warns and skips
/// that one relationship, not the whole result.
fn ingest_check_result(
    conn: &mut Connection,
    scan_id: Uuid,
    plugin_name: &str,
    plugin_version: &str,
    target_entity: &Entity,
    result: &CheckResult,
) -> Result<(), EngineError> {
    let confidence = ConfidenceStatus::from_protocol_i32(result.status);
    let proto_provenance = result.provenance.as_ref();
    let non_empty = |s: &str| (!s.is_empty()).then(|| s.to_string());
    let provenance = Provenance {
        source: plugin_name.to_string(),
        source_version: plugin_version.to_string(),
        source_url: proto_provenance.and_then(|p| non_empty(&p.source_url)),
        retrieval_method: proto_provenance.and_then(|p| non_empty(&p.retrieval_method)),
        raw_response_sha256: proto_provenance.and_then(|p| non_empty(&p.raw_response_sha256)),
        collected_at_unix_ms: proto_provenance
            .map(|p| p.collected_at_unix_ms)
            .filter(|&t| t > 0)
            .unwrap_or_else(now_unix_ms),
    };

    let mut canonical_key_to_id: HashMap<String, EntityId> = HashMap::new();
    for finding in &result.entities {
        let entity_type: EntityType = finding
            .entity_type
            .parse()
            .expect("EntityType::from_str is infallible");
        let attrs: Vec<Attribute> = finding
            .attributes
            .iter()
            .map(|(k, v)| Attribute {
                key: k.clone(),
                value: v.clone(),
            })
            .collect();
        let key = non_empty(&finding.canonical_key);
        let display_label = non_empty(&finding.display_label).or_else(|| key.clone());

        let id = crud::add_entity_from_scan(
            conn,
            entity_type,
            key.clone(),
            display_label,
            attrs,
            provenance.clone(),
            confidence.clone(),
            scan_id,
        )?;
        if let Some(key) = key {
            canonical_key_to_id.insert(key, id);
        }
    }

    for rel in &result.relationships {
        let resolve = |key: &str| -> Option<EntityId> {
            canonical_key_to_id.get(key).copied().or({
                if target_entity.canonical_key.as_deref() == Some(key) {
                    Some(target_entity.id)
                } else {
                    None
                }
            })
        };
        match (
            resolve(&rel.from_canonical_key),
            resolve(&rel.to_canonical_key),
        ) {
            (Some(from), Some(to)) => {
                let rel_type: RelationshipType = rel
                    .relationship_type
                    .parse()
                    .expect("RelationshipType::from_str is infallible");
                crud::add_relationship_from_scan(
                    conn,
                    from,
                    to,
                    rel_type,
                    Vec::new(),
                    provenance.clone(),
                    confidence.clone(),
                    scan_id,
                )?;
            }
            _ => {
                eprintln!(
                    "warning: scan {scan_id}: plugin {plugin_name} reported a relationship \
                     with an unresolved endpoint ({} -> {}), skipping",
                    rel.from_canonical_key, rel.to_canonical_key
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use crate::{keystore, Case, EntityFilter, EntityType};

    fn example_binary_path(name: &str) -> PathBuf {
        let mut path = std::env::current_exe().expect("current test binary path");
        path.pop();
        if path.ends_with("deps") {
            path.pop();
        }
        path.push("examples");
        path.push(name);
        path
    }

    /// Writes `<plugins_dir>/<plugin_name>/plugin.toml`, always unsigned
    /// (tests use `TrustPolicy::AllowUnsigned` throughout — signature
    /// verification itself is already covered by
    /// eumeaus-plugin-host's own tests). `binary_example_name` may differ
    /// from `plugin_name`, so several manifests can share one compiled
    /// fixture binary under distinct plugin identities.
    fn write_manifest(
        plugins_dir: &Path,
        plugin_name: &str,
        binary_example_name: &str,
        default_timeout_ms: u64,
    ) {
        let plugin_dir = plugins_dir.join(plugin_name);
        std::fs::create_dir_all(&plugin_dir).unwrap();
        let entrypoint = example_binary_path(binary_example_name);
        let toml = format!(
            r#"
[plugin]
name = "{plugin_name}"
version = "0.1.0"
description = "test fixture"
author = "test"

[compatibility]
engine_min = "0.1.0"
engine_max = "0.x"
protocol_version = "1"

[contract]
input_entity_types = ["Username"]
output_entity_types = ["OnlineAccount"]
output_relationship_types = ["HasAccount"]

[permissions]
network = false
requested_credentials = []

[execution]
entrypoint = "{entrypoint}"
default_rate_limit_per_sec = 100
default_timeout_ms = {default_timeout_ms}
"#,
            entrypoint = entrypoint.display(),
        );
        std::fs::write(plugin_dir.join("plugin.toml"), toml).unwrap();
    }

    fn new_case_with_target(dir: &Path, case_name: &str) -> (Case, EntityId) {
        let mut case = Case::create(dir, case_name).unwrap();
        let id = case
            .add_entity(
                EntityType::Username,
                Some("carol".to_string()),
                vec![],
                Provenance {
                    source: "user".to_string(),
                    source_version: "0.1.0".to_string(),
                    source_url: None,
                    retrieval_method: None,
                    raw_response_sha256: None,
                    collected_at_unix_ms: now_unix_ms(),
                },
            )
            .unwrap();
        (case, id)
    }

    fn run_status(conn: &Connection, scan_id: Uuid, plugin_name: &str) -> String {
        conn.query_row(
            "SELECT status FROM scan_plugin_runs WHERE scan_id = ?1 AND plugin_name = ?2",
            params![scan_id.to_string(), plugin_name],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn scan_ok_ingests_entity_and_relationship() {
        let base = tempfile::tempdir().unwrap();
        let plugins_dir = base.path().join("plugins");
        write_manifest(&plugins_dir, "scan-ok", "scan_ok", 2000);

        let (mut case, target_id) = new_case_with_target(&base.path().join("case"), "basic");
        let scan_id = case
            .start_scan(
                &plugins_dir,
                vec![],
                TargetEntity { id: target_id },
                ScanConfig::default(),
                TrustPolicy::AllowUnsigned,
            )
            .unwrap();

        assert_eq!(case.scan_status(scan_id).unwrap(), ScanStatus::Completed);

        let entities = case
            .list_entities(EntityFilter {
                entity_type: Some(EntityType::OnlineAccount),
            })
            .unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].canonical_key.as_deref(), Some("carol-account"));

        let rel_count: i64 = case
            .conn_mut()
            .query_row("SELECT count(*) FROM relationships", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            rel_count, 1,
            "the HasAccount relationship should be ingested too"
        );

        keystore::delete_key(case.id()).ok();
    }

    #[test]
    fn worker_pool_bounds_concurrency() {
        let base = tempfile::tempdir().unwrap();
        let plugins_dir = base.path().join("plugins");
        for name in ["p1", "p2", "p3"] {
            write_manifest(&plugins_dir, name, "scan_slow", 5000);
        }

        let (mut serial_case, target_id) =
            new_case_with_target(&base.path().join("case-serial"), "serial");
        let start = Instant::now();
        serial_case
            .start_scan(
                &plugins_dir,
                vec![],
                TargetEntity { id: target_id },
                ScanConfig {
                    worker_pool: Some(1),
                    ..Default::default()
                },
                TrustPolicy::AllowUnsigned,
            )
            .unwrap();
        let serial_elapsed = start.elapsed();
        keystore::delete_key(serial_case.id()).ok();

        let (mut concurrent_case, target_id) =
            new_case_with_target(&base.path().join("case-concurrent"), "concurrent");
        let start = Instant::now();
        concurrent_case
            .start_scan(
                &plugins_dir,
                vec![],
                TargetEntity { id: target_id },
                ScanConfig {
                    worker_pool: Some(3),
                    ..Default::default()
                },
                TrustPolicy::AllowUnsigned,
            )
            .unwrap();
        let concurrent_elapsed = start.elapsed();
        keystore::delete_key(concurrent_case.id()).ok();

        // Three plugins each sleeping 300ms: serial (pool=1) must take
        // roughly 3x as long as concurrent (pool=3). A generous 1.5x
        // margin keeps this robust against CI/scheduler jitter while
        // still failing if the worker pool wasn't actually bounding
        // (or enabling) concurrency.
        assert!(
            concurrent_elapsed.as_millis() * 3 / 2 < serial_elapsed.as_millis(),
            "expected concurrent ({concurrent_elapsed:?}) to be well under serial ({serial_elapsed:?})"
        );
    }

    #[test]
    fn resume_scan_only_reruns_pending_runs() {
        let base = tempfile::tempdir().unwrap();
        let plugins_dir = base.path().join("plugins");
        write_manifest(&plugins_dir, "scan-ok", "scan_ok", 2000);

        let (mut case, target_id) = new_case_with_target(&base.path().join("case"), "resume");
        let scan_id = case
            .start_scan(
                &plugins_dir,
                vec![],
                TargetEntity { id: target_id },
                ScanConfig::default(),
                TrustPolicy::AllowUnsigned,
            )
            .unwrap();
        assert_eq!(run_status(case.conn_mut(), scan_id.0, "scan-ok"), "SUCCESS");

        // A second plugin the original scan never got to — added to the
        // plugins dir, and its scan_plugin_runs row inserted directly,
        // *after* the fact: this is what "the scan was supposed to also
        // run this, but the process died first" looks like on disk.
        write_manifest(&plugins_dir, "scan-ok-2", "scan_ok", 2000);
        case.conn_mut()
            .execute(
                "INSERT INTO scan_plugin_runs
                    (id, scan_id, plugin_name, plugin_version, status, started_at, completed_at, error_message)
                 VALUES (?1, ?2, 'scan-ok-2', '0.1.0', 'PENDING', NULL, NULL, NULL)",
                params![Uuid::new_v4().to_string(), scan_id.0.to_string()],
            )
            .unwrap();

        case.resume_scan(scan_id).unwrap();

        assert_eq!(
            run_status(case.conn_mut(), scan_id.0, "scan-ok-2"),
            "SUCCESS",
            "the newly-pending run should have executed"
        );

        let account_count: i64 = case
            .conn_mut()
            .query_row(
                "SELECT count(*) FROM entities WHERE canonical_key = 'carol-account'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            account_count, 1,
            "the two plugins' identical findings should auto-merge into one entity"
        );
        // Each plugin invocation produces one entity fact and one
        // relationship fact; if resume had wrongly re-invoked the
        // already-SUCCESS scan-ok run, this would be 4, not 2.
        let fact_count: i64 = case
            .conn_mut()
            .query_row(
                "SELECT count(*) FROM facts WHERE source = 'scan-ok'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            fact_count, 2,
            "resume must not re-invoke the already-SUCCESS scan-ok run"
        );

        keystore::delete_key(case.id()).ok();
    }

    #[test]
    fn crash_mid_scan_is_reconciled_to_pending_on_reopen() {
        let base = tempfile::tempdir().unwrap();
        let plugins_dir = base.path().join("plugins");
        write_manifest(&plugins_dir, "scan-ok", "scan_ok", 2000);

        let (mut case, target_id) = new_case_with_target(&base.path().join("case"), "crash");
        let case_path = case.path().to_path_buf();
        let scan_id = case
            .start_scan(
                &plugins_dir,
                vec![],
                TargetEntity { id: target_id },
                ScanConfig::default(),
                TrustPolicy::AllowUnsigned,
            )
            .unwrap();

        // Simulate a crash right after dispatch: force the (already
        // SUCCESS) row back to RUNNING, as SPEC.md §4.2 says a crashed
        // process would leave it.
        case.conn_mut()
            .execute(
                "UPDATE scan_plugin_runs SET status = 'RUNNING' WHERE scan_id = ?1",
                params![scan_id.0.to_string()],
            )
            .unwrap();
        case.close().unwrap();

        let mut reopened = Case::open(&case_path).unwrap();
        assert_eq!(
            run_status(reopened.conn_mut(), scan_id.0, "scan-ok"),
            "PENDING"
        );

        keystore::delete_key(reopened.id()).ok();
    }

    #[test]
    fn hanging_plugin_times_out_without_blocking_the_scan() {
        let base = tempfile::tempdir().unwrap();
        let plugins_dir = base.path().join("plugins");
        write_manifest(&plugins_dir, "scan-ok", "scan_ok", 2000);
        write_manifest(&plugins_dir, "scan-hang", "scan_hang", 200);

        let (mut case, target_id) = new_case_with_target(&base.path().join("case"), "hang");

        let start = Instant::now();
        let scan_id = case
            .start_scan(
                &plugins_dir,
                vec![],
                TargetEntity { id: target_id },
                ScanConfig {
                    worker_pool: Some(2),
                    ..Default::default()
                },
                TrustPolicy::AllowUnsigned,
            )
            .unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(5),
            "the hung plugin must not block the whole scan: took {elapsed:?}"
        );
        assert_eq!(
            case.scan_status(scan_id).unwrap(),
            ScanStatus::PartiallyFailed
        );
        assert_eq!(
            run_status(case.conn_mut(), scan_id.0, "scan-hang"),
            "TIMEOUT"
        );
        assert_eq!(run_status(case.conn_mut(), scan_id.0, "scan-ok"), "SUCCESS");

        keystore::delete_key(case.id()).ok();
    }
}
