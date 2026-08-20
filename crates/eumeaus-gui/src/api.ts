// Typed wrappers over every Tauri command eumeaus-gui's Rust side exposes
// (case_state.rs / entity_state.rs / scan_state.rs / plugin_state.rs /
// credential_state.rs / trust_state.rs / overview_state.rs). Centralized
// here so screen components import functions, not raw invoke() calls with
// hand-typed generics scattered across files.

import { invoke } from "@tauri-apps/api/core";

export interface CaseInfo {
  id: string;
  name: string;
  path: string;
}

export interface EntitySummary {
  id: string;
  entity_type: string;
  canonical_key: string | null;
  display_label: string;
}

export interface AttributeRecord {
  fact_id: string;
  key: string;
  value: string;
  source: string;
  collected_at_unix_ms: number;
  is_current: boolean;
  conflicting: boolean;
}

export interface EntityDetail extends EntitySummary {
  attributes: AttributeRecord[];
}

export interface AttributeInput {
  key: string;
  value: string;
}

export interface RelationshipDto {
  id: string;
  from: string;
  to: string;
  relationship_type: string;
  created_at_unix_ms: number;
}

export interface ScanProgress {
  scan_id: string;
  plugin_name: string;
  status: string;
  error_message: string | null;
}

export interface ScanSummary {
  id: string;
  status: string;
  target_entity_id: string;
  started_at_unix_ms: number | null;
  completed_at_unix_ms: number | null;
}

export interface PluginSummary {
  name: string;
  version: string;
  description: string;
  signed: boolean;
  entrypoint: string;
  input_entity_types: string[];
}

export interface TrustedKey {
  name: string;
  public_key: string;
}

export interface CaseStats {
  entity_count: number;
  fact_count: number;
  relationship_count: number;
  conflicting_entity_count: number;
}

export interface AuditEvent {
  id: string;
  event_type: string;
  description: string;
  actor: string;
  occurred_at_unix_ms: number;
}

// ---- case_state.rs ----

export const caseCreate = (dir: string, name: string) =>
  invoke<CaseInfo>("case_create", { dir, name });
export const caseOpen = (path: string) => invoke<CaseInfo>("case_open", { path });
export const caseClose = () => invoke<void>("case_close");
export const caseCurrent = () => invoke<CaseInfo | null>("case_current");
export const caseList = (dir: string) => invoke<CaseInfo[]>("case_list", { dir });

// ---- entity_state.rs ----

export const entityList = (entityType: string | null) =>
  invoke<EntitySummary[]>("entity_list", { entityType });
export const entityShow = (id: string) => invoke<EntityDetail>("entity_show", { id });
export const entityAdd = (entityType: string, key: string | null, attrs: AttributeInput[]) =>
  invoke<EntitySummary>("entity_add", { entityType, key, attrs });
export const entityMerge = (id1: string, id2: string) =>
  invoke<EntitySummary>("entity_merge", { id1, id2 });
export const entitySplit = (id: string, factIds: string[]) =>
  invoke<EntitySummary>("entity_split", { id, factIds });
export const relationshipAdd = (
  from: string,
  to: string,
  relType: string,
  attrs: AttributeInput[],
) => invoke<string>("relationship_add", { from, to, relType, attrs });
export const relationshipList = () => invoke<RelationshipDto[]>("relationship_list");
export const entityAudit = (id: string) => invoke<AuditEvent[]>("entity_audit", { id });

// ---- overview_state.rs ----

export const caseStats = () => invoke<CaseStats>("case_stats");
export const auditList = (limit: number) => invoke<AuditEvent[]>("audit_list", { limit });

// ---- scan_state.rs ----

export const scanRun = (
  pluginsDir: string,
  plugin: string[],
  targetType: string,
  targetValue: string,
) => invoke<string>("scan_run", { pluginsDir, plugin, targetType, targetValue });
export const scanList = () => invoke<ScanSummary[]>("scan_list");

// ---- plugin_state.rs ----

export const pluginList = (pluginsDir: string) =>
  invoke<PluginSummary[]>("plugin_list", { pluginsDir });
export const pluginInstall = (sourcePath: string, pluginsDir: string) =>
  invoke<PluginSummary>("plugin_install", { sourcePath, pluginsDir });
export const pluginVerify = (
  name: string,
  pluginsDir: string,
  trustedKey: string | null,
  trust: string | null,
) => invoke<void>("plugin_verify", { name, pluginsDir, trustedKey, trust });

// ---- credential_state.rs ----

export const credentialSet = (name: string, value: string) =>
  invoke<void>("credential_set", { name, value });
export const credentialList = () => invoke<string[]>("credential_list");
export const credentialRemove = (name: string) => invoke<void>("credential_remove", { name });

// ---- trust_state.rs ----

export const trustAdd = (name: string, publicKey: string) =>
  invoke<void>("trust_add", { name, publicKey });
export const trustList = () => invoke<TrustedKey[]>("trust_list");
export const trustRemove = (name: string) => invoke<void>("trust_remove", { name });

// ---- misc ----

export const listEntityTypes = () => invoke<string[]>("list_entity_types");
