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
  hidden: boolean;
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

export interface EntityImageSummary {
  id: string;
  fact_id: string;
  mime_type: string;
  collected_at_unix_ms: number;
  is_current: boolean;
}

export interface EntityImageData {
  mime_type: string;
  data_base64: string;
}

export interface AttributeInput {
  key: string;
  value: string;
}

export interface EntityPosition {
  entity_id: string;
  x: number;
  y: number;
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

export const entityList = (entityType: string | null, includeHidden = false) =>
  invoke<EntitySummary[]>("entity_list", { entityType, includeHidden });
export const entityShow = (id: string) => invoke<EntityDetail>("entity_show", { id });
export const entityAdd = (entityType: string, key: string | null, attrs: AttributeInput[]) =>
  invoke<EntitySummary>("entity_add", { entityType, key, attrs });
export const entityHide = (id: string, reason: string | null) =>
  invoke<void>("entity_hide", { id, reason });
export const entityUnhide = (id: string) => invoke<void>("entity_unhide", { id });
export const entityMerge = (id1: string, id2: string) =>
  invoke<EntitySummary>("entity_merge", { id1, id2 });
export const entitySplit = (
  id: string,
  factIds: string[],
  entityType: string,
  key: string | null,
) => invoke<EntitySummary>("entity_split", { id, factIds, entityType, key });
export const entityAddFact = (id: string, attrs: AttributeInput[]) =>
  invoke<EntityDetail>("entity_add_fact", { id, attrs });
export const entityAddImage = (id: string, path: string) =>
  invoke<EntityImageSummary>("entity_add_image", { id, path });
export const entityListImages = (id: string) =>
  invoke<EntityImageSummary[]>("entity_list_images", { id });
export const entityGetImage = (imageId: string) =>
  invoke<EntityImageData>("entity_get_image", { imageId });
export const factRedact = (factId: string, reason: string) =>
  invoke<void>("fact_redact", { factId, reason });
export const relationshipAdd = (
  from: string,
  to: string,
  relType: string,
  attrs: AttributeInput[],
) => invoke<string>("relationship_add", { from, to, relType, attrs });
export const relationshipList = (includeHidden = false) =>
  invoke<RelationshipDto[]>("relationship_list", { includeHidden });
export const entitySetPosition = (id: string, x: number, y: number) =>
  invoke<void>("entity_set_position", { id, x, y });
export const entityListPositions = () => invoke<EntityPosition[]>("entity_list_positions");
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

// ---- report_state.rs ----

export const caseExport = (
  out: string,
  format: "sqlite" | "report" | "html" | "portable",
  passphrase: string | null,
  signKeyHex: string | null,
) => invoke<string | null>("case_export", { out, format, passphrase, signKeyHex });
export const reportVerify = (
  reportPath: string,
  sigPath: string,
  trustedKey: string | null,
  trust: string | null,
) => invoke<void>("report_verify", { reportPath, sigPath, trustedKey, trust });

// ---- settings_state.rs ----

export const settingsGetPluginsDir = () =>
  invoke<string | null>("settings_get_plugins_dir");
export const settingsSetPluginsDir = (dir: string) =>
  invoke<void>("settings_set_plugins_dir", { dir });

// ---- misc ----

export const listEntityTypes = () => invoke<string[]>("list_entity_types");
