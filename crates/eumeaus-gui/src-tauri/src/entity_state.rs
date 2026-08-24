//! G2 (SPEC.md §9.6): entity/fact browsing, read-only — wraps
//! `Case::list_entities`/`get_entity`/`list_attribute_records`, the same
//! calls `entity list`/`entity show` make (`eumeaus-cli/src/main.rs`).
//!
//! G4: the write path — `entity_add`/`entity_merge`/`entity_split`/
//! `relationship_add`, wrapping `Case::add_entity`/`merge_entities`/
//! `split_entity`/`add_relationship`, same as `eumeaus-cli`'s `entity
//! add`/`merge`/`split` and `relationship add`. Attribute pairs come from
//! the frontend as structured `{key, value}` objects rather than the
//! CLI's `"key=value"` strings — that format exists for shell/argv
//! convenience the GUI doesn't have, not because it's part of the
//! engine's own shape (SPEC.md §9.3's note on `credential set`'s TTY
//! prompt vs. a normal password `<input>` is the same kind of
//! CLI-vs-GUI-native-ergonomics call).
//!
//! Reads the currently open case from `case_state::AppState`'s shared
//! `Mutex<Option<Case>>` — no case open is a normal, expected error here
//! ("open a case first"), not a bug.

use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine;
use eumeaus_engine::{
    Actor, Attribute, Case, DocumentId, EntityFilter, EntityId, EntityPosition, EntityType, FactId,
    ImageId, Provenance, Relationship, RelationshipType,
};
use serde::{Deserialize, Serialize};

use crate::case_state::AppState;

#[derive(Deserialize, Debug, Clone)]
pub struct AttributeInput {
    pub key: String,
    pub value: String,
}

impl From<AttributeInput> for Attribute {
    fn from(a: AttributeInput) -> Self {
        Attribute {
            key: a.key,
            value: a.value,
        }
    }
}

/// No identity/auth system yet (SPEC.md doesn't specify one for v1), same
/// as `eumeaus-cli`'s own `default_actor`/`manual_provenance` — every
/// GUI-initiated write is attributed to a fixed `"user"` actor/source too.
fn default_actor() -> Actor {
    Actor {
        name: "user".to_string(),
    }
}

// eumeaus_engine::now_unix_ms is crate-private — eumeaus-cli has its own
// identical copy for the same reason.
fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as i64
}

fn manual_provenance() -> Provenance {
    Provenance {
        source: "user".to_string(),
        source_version: env!("CARGO_PKG_VERSION").to_string(),
        source_url: None,
        retrieval_method: None,
        raw_response_sha256: None,
        collected_at_unix_ms: now_unix_ms(),
    }
}

#[derive(Serialize, Debug)]
pub struct EntitySummary {
    pub id: String,
    pub entity_type: String,
    pub canonical_key: Option<String>,
    pub display_label: String,
}

impl From<eumeaus_engine::Entity> for EntitySummary {
    fn from(e: eumeaus_engine::Entity) -> Self {
        EntitySummary {
            id: e.id.to_string(),
            entity_type: e.entity_type.to_string(),
            canonical_key: e.canonical_key,
            display_label: e.display_label,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct AttributeRecordDto {
    pub fact_id: String,
    pub key: String,
    pub value: String,
    pub source: String,
    pub collected_at_unix_ms: i64,
    pub is_current: bool,
    pub conflicting: bool,
}

impl From<eumeaus_engine::AttributeRecord> for AttributeRecordDto {
    fn from(a: eumeaus_engine::AttributeRecord) -> Self {
        AttributeRecordDto {
            fact_id: a.fact_id.to_string(),
            key: a.key,
            value: a.value,
            source: a.source,
            collected_at_unix_ms: a.collected_at_unix_ms,
            is_current: a.is_current,
            conflicting: a.conflicting,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct EntityDetail {
    #[serde(flatten)]
    pub summary: EntitySummary,
    pub attributes: Vec<AttributeRecordDto>,
}

#[derive(Serialize, Debug)]
pub struct EntityImageSummaryDto {
    pub id: String,
    pub fact_id: String,
    pub mime_type: String,
    pub collected_at_unix_ms: i64,
    pub is_current: bool,
}

impl From<eumeaus_engine::EntityImageSummary> for EntityImageSummaryDto {
    fn from(i: eumeaus_engine::EntityImageSummary) -> Self {
        EntityImageSummaryDto {
            id: i.id.to_string(),
            fact_id: i.fact_id.to_string(),
            mime_type: i.mime_type,
            collected_at_unix_ms: i.collected_at_unix_ms,
            is_current: i.is_current,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct EntityImageDataDto {
    pub mime_type: String,
    pub data_base64: String,
}

impl From<eumeaus_engine::EntityImageData> for EntityImageDataDto {
    fn from(i: eumeaus_engine::EntityImageData) -> Self {
        EntityImageDataDto {
            mime_type: i.mime_type,
            data_base64: base64::engine::general_purpose::STANDARD.encode(i.data),
        }
    }
}

// No data-carrying Dto counterpart to EntityImageDataDto: a document's
// bytes never cross the JS<->Rust IPC boundary at all (issue #10) —
// entity_open_document writes them straight to a temp file and opens it
// server-side, so there's nothing to base64-encode for the frontend.
#[derive(Serialize, Debug)]
pub struct EntityDocumentSummaryDto {
    pub id: String,
    pub fact_id: String,
    pub file_name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub collected_at_unix_ms: i64,
}

impl From<eumeaus_engine::EntityDocumentSummary> for EntityDocumentSummaryDto {
    fn from(d: eumeaus_engine::EntityDocumentSummary) -> Self {
        EntityDocumentSummaryDto {
            id: d.id.to_string(),
            fact_id: d.fact_id.to_string(),
            file_name: d.file_name,
            mime_type: d.mime_type,
            size_bytes: d.size_bytes,
            collected_at_unix_ms: d.collected_at_unix_ms,
        }
    }
}

// Bounded to what the picker's own filter already allows (see
// pickers.ts's pickImageFile) — a fixed, known set, so a real
// content-sniffing dependency (mime_guess et al.) would be solving a
// bigger problem than this feature actually has.
fn mime_type_for_extension(path: &Path) -> Result<&'static str, String> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Ok("image/png"),
        Some("jpg") | Some("jpeg") => Ok("image/jpeg"),
        Some("gif") => Ok("image/gif"),
        Some("webp") => Ok("image/webp"),
        _ => Err("unsupported image type — expected .png, .jpg, .jpeg, .gif, or .webp".to_string()),
    }
}

// Avatar-shaped images only for phase one — keeps the base64-over-JSON
// transport and the case DB's own size bounded and fast. A real
// full-resolution photo gallery would need a different transport
// (Tauri's asset protocol) and is out of scope here.
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

// A fixed allowlist, same as mime_type_for_extension — "could be changed
// later" per issue #10, not content-sniffed.
fn document_mime_type_for_extension(path: &Path) -> Result<&'static str, String> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("txt") => Ok("text/plain"),
        Some("pdf") => Ok("application/pdf"),
        _ => Err("unsupported document type — expected .txt or .pdf".to_string()),
    }
}

// PDFs can legitimately run larger than an avatar-shaped image — a
// separate, bigger cap than MAX_IMAGE_BYTES rather than reusing it.
const MAX_DOCUMENT_BYTES: u64 = 25 * 1024 * 1024;

const NO_CASE_OPEN: &str = "no case is currently open — open a case first";

fn do_entity_list(
    cell: &Arc<Mutex<Option<Case>>>,
    entity_type: Option<String>,
) -> Result<Vec<EntitySummary>, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    let filter = EntityFilter {
        // EntityType::from_str is Infallible (SPEC.md §4.3's Custom escape
        // hatch) — an unrecognized string just becomes Custom(s), same as
        // the CLI's own `entity list --type` handling.
        entity_type: entity_type.map(|t| t.parse().expect("infallible")),
    };
    case.list_entities(filter)
        .map(|entities| entities.into_iter().map(EntitySummary::from).collect())
        .map_err(|e| e.to_string())
}

fn do_entity_show(cell: &Arc<Mutex<Option<Case>>>, id: &str) -> Result<EntityDetail, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    let entity_id = EntityId(id.parse().map_err(|e| format!("invalid entity id: {e}"))?);

    let entity = case.get_entity(entity_id).map_err(|e| e.to_string())?;
    let attributes = case
        .list_attribute_records(entity_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(AttributeRecordDto::from)
        .collect();

    Ok(EntityDetail {
        summary: EntitySummary::from(entity),
        attributes,
    })
}

fn do_entity_add(
    cell: &Arc<Mutex<Option<Case>>>,
    entity_type: &str,
    key: Option<String>,
    attrs: Vec<AttributeInput>,
) -> Result<EntitySummary, String> {
    let mut guard = cell.lock().unwrap();
    let case = guard.as_mut().ok_or(NO_CASE_OPEN)?;

    let entity_type: EntityType = entity_type.parse().expect("infallible");
    let attrs: Vec<Attribute> = attrs.into_iter().map(Attribute::from).collect();
    let id = case
        .add_entity(entity_type, key, attrs, manual_provenance())
        .map_err(|e| e.to_string())?;
    case.get_entity(id)
        .map(EntitySummary::from)
        .map_err(|e| e.to_string())
}

/// Adds a fact directly to an *existing* entity (SPEC.md §9.3, the
/// exploratory test's §3.1 finding: `entity_add` only supports "add or
/// auto-merge by type+key," which silently creates a duplicate for a
/// keyless entity rather than adding to it — see
/// `Case::add_fact_to_entity`'s doc comment). Not a CLI command; this is
/// a GUI-only affordance, same precedent as `case_stats`/`audit_trail_all`.
fn do_entity_add_fact(
    cell: &Arc<Mutex<Option<Case>>>,
    id: &str,
    attrs: Vec<AttributeInput>,
) -> Result<EntityDetail, String> {
    let mut guard = cell.lock().unwrap();
    let case = guard.as_mut().ok_or(NO_CASE_OPEN)?;

    let entity_id = EntityId(id.parse().map_err(|e| format!("invalid entity id: {e}"))?);
    let attrs: Vec<Attribute> = attrs.into_iter().map(Attribute::from).collect();
    case.add_fact_to_entity(entity_id, attrs, manual_provenance())
        .map_err(|e| e.to_string())?;

    let entity = case.get_entity(entity_id).map_err(|e| e.to_string())?;
    let attributes = case
        .list_attribute_records(entity_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(AttributeRecordDto::from)
        .collect();
    Ok(EntityDetail {
        summary: EntitySummary::from(entity),
        attributes,
    })
}

/// Attaches an image to an entity (SPEC.md §9.3's "attach a profile
/// picture" idea) — `Case::add_image_to_entity`'s GUI-only affordance,
/// same precedent as `entity_add_fact`. `path` comes from the native
/// picker (`pickImageFile()`); the file is read here, server-side, never
/// pushed through the JS→Rust IPC boundary as a JSON byte array.
fn do_entity_add_image(
    cell: &Arc<Mutex<Option<Case>>>,
    id: &str,
    path: &Path,
) -> Result<EntityImageSummaryDto, String> {
    let entity_id = EntityId(id.parse().map_err(|e| format!("invalid entity id: {e}"))?);
    let mime_type = mime_type_for_extension(path)?;

    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_IMAGE_BYTES {
        return Err(format!(
            "image is too large ({:.1} MiB, max {} MiB)",
            size as f64 / (1024.0 * 1024.0),
            MAX_IMAGE_BYTES / (1024 * 1024),
        ));
    }
    let data = std::fs::read(path).map_err(|e| e.to_string())?;

    let mut guard = cell.lock().unwrap();
    let case = guard.as_mut().ok_or(NO_CASE_OPEN)?;
    case.add_image_to_entity(entity_id, mime_type.to_string(), data, manual_provenance())
        .map_err(|e| e.to_string())?;

    case.list_entity_images(entity_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|i| i.is_current)
        .map(EntityImageSummaryDto::from)
        .ok_or_else(|| "image uploaded but not found on re-list".to_string())
}

fn do_entity_list_images(
    cell: &Arc<Mutex<Option<Case>>>,
    id: &str,
) -> Result<Vec<EntityImageSummaryDto>, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    let entity_id = EntityId(id.parse().map_err(|e| format!("invalid entity id: {e}"))?);
    case.list_entity_images(entity_id)
        .map(|imgs| imgs.into_iter().map(EntityImageSummaryDto::from).collect())
        .map_err(|e| e.to_string())
}

fn do_entity_get_image(
    cell: &Arc<Mutex<Option<Case>>>,
    image_id: &str,
) -> Result<EntityImageDataDto, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    let image_id = ImageId(
        image_id
            .parse()
            .map_err(|e| format!("invalid image id: {e}"))?,
    );
    case.get_entity_image(image_id)
        .map(EntityImageDataDto::from)
        .map_err(|e| e.to_string())
}

/// Attaches a document to an entity (issue #10) — same pattern as
/// `do_entity_add_image`: `path` comes from the native picker
/// (`pickDocumentFile()`), read server-side. The original file's own
/// name (not the full picked path) becomes `file_name`.
fn do_entity_add_document(
    cell: &Arc<Mutex<Option<Case>>>,
    id: &str,
    path: &Path,
) -> Result<EntityDocumentSummaryDto, String> {
    let entity_id = EntityId(id.parse().map_err(|e| format!("invalid entity id: {e}"))?);
    let mime_type = document_mime_type_for_extension(path)?;
    let file_name = path
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or("document path has no file name")?
        .to_string();

    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "document is too large ({:.1} MiB, max {} MiB)",
            size as f64 / (1024.0 * 1024.0),
            MAX_DOCUMENT_BYTES / (1024 * 1024),
        ));
    }
    let data = std::fs::read(path).map_err(|e| e.to_string())?;

    let mut guard = cell.lock().unwrap();
    let case = guard.as_mut().ok_or(NO_CASE_OPEN)?;
    case.add_document_to_entity(
        entity_id,
        file_name,
        mime_type.to_string(),
        data,
        manual_provenance(),
    )
    .map_err(|e| e.to_string())?;

    // Newest first, and this is the upload that was just committed —
    // no `is_current` flag to filter on (documents aren't a gallery),
    // so the first row is simply it.
    case.list_entity_documents(entity_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
        .map(EntityDocumentSummaryDto::from)
        .ok_or_else(|| "document uploaded but not found on re-list".to_string())
}

fn do_entity_list_documents(
    cell: &Arc<Mutex<Option<Case>>>,
    id: &str,
) -> Result<Vec<EntityDocumentSummaryDto>, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    let entity_id = EntityId(id.parse().map_err(|e| format!("invalid entity id: {e}"))?);
    case.list_entity_documents(entity_id)
        .map(|docs| {
            docs.into_iter()
                .map(EntityDocumentSummaryDto::from)
                .collect()
        })
        .map_err(|e| e.to_string())
}

// Strips any directory components (including a maliciously crafted
// "../../etc/passwd"-shaped file_name from the case DB) down to the bare
// file name before it's ever used to build a filesystem path — file_name
// is stored as-is from the original upload, so this can't be trusted
// verbatim as a path segment.
fn safe_temp_file_name(document_id: DocumentId, file_name: &str) -> std::ffi::OsString {
    let base = Path::new(file_name)
        .file_name()
        .map(|f| f.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("document"));
    let mut name = std::ffi::OsString::from(format!("{document_id}_"));
    name.push(base);
    name
}

/// Fetches a document's bytes and writes them to a fresh temp file, ready
/// to hand to the OS's own opener — split out from `do_entity_open_document`
/// so this half (the part with anything meaningful to assert on) is
/// testable without a real `tauri::AppHandle`, which nothing in this
/// crate's test suite constructs.
fn write_document_to_temp_file(
    cell: &Arc<Mutex<Option<Case>>>,
    document_id: &str,
) -> Result<std::path::PathBuf, String> {
    let document_id = DocumentId(
        document_id
            .parse()
            .map_err(|e| format!("invalid document id: {e}"))?,
    );

    let data = {
        let guard = cell.lock().unwrap();
        let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
        case.get_entity_document(document_id)
            .map_err(|e| e.to_string())?
    };

    let temp_path = std::env::temp_dir().join(safe_temp_file_name(document_id, &data.file_name));
    std::fs::write(&temp_path, &data.data).map_err(|e| e.to_string())?;
    Ok(temp_path)
}

/// Writes a document's bytes to a temp file and opens it with the OS
/// default handler (issue #10) — decided over a Save-As download so a
/// click is one step, and the OS's own PDF viewer/text editor covers "no
/// preview needed" for free. Uses `tauri_plugin_opener`'s Rust-side API
/// directly (`app.opener()...`), not the JS-facing `invoke`d command, so
/// this needs no IPC scope/capability entry — same reasoning as why this
/// takes `app: tauri::AppHandle`, following `scan_run`'s existing
/// precedent for a command that needs plugin access beyond `AppState`.
/// The temp file is deliberately left behind after opening — the OS app
/// that just opened it may still be reading it.
fn do_entity_open_document(
    app: &tauri::AppHandle,
    cell: &Arc<Mutex<Option<Case>>>,
    document_id: &str,
) -> Result<(), String> {
    let temp_path = write_document_to_temp_file(cell, document_id)?;

    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(temp_path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// `fact redact` (SPEC.md §8.4) had no GUI command until now — CLI-only.
/// Needed here so a mis-uploaded image (or any other fact) can actually
/// be removed from the GUI; not image-specific itself, `Case::redact_fact`
/// already deletes the associated `entity_images` row for free.
fn do_fact_redact(
    cell: &Arc<Mutex<Option<Case>>>,
    fact_id: &str,
    reason: &str,
) -> Result<(), String> {
    let mut guard = cell.lock().unwrap();
    let case = guard.as_mut().ok_or(NO_CASE_OPEN)?;
    let fact_id = FactId(
        fact_id
            .parse()
            .map_err(|e| format!("invalid fact id: {e}"))?,
    );
    case.redact_fact(fact_id, default_actor(), reason)
        .map_err(|e| e.to_string())
}

fn do_entity_merge(
    cell: &Arc<Mutex<Option<Case>>>,
    id1: &str,
    id2: &str,
) -> Result<EntitySummary, String> {
    let mut guard = cell.lock().unwrap();
    let case = guard.as_mut().ok_or(NO_CASE_OPEN)?;

    let id1 = EntityId(id1.parse().map_err(|e| format!("invalid entity id: {e}"))?);
    let id2 = EntityId(id2.parse().map_err(|e| format!("invalid entity id: {e}"))?);
    let survivor = case
        .merge_entities(id1, id2, default_actor())
        .map_err(|e| e.to_string())?;
    case.get_entity(survivor)
        .map(EntitySummary::from)
        .map_err(|e| e.to_string())
}

fn do_entity_split(
    cell: &Arc<Mutex<Option<Case>>>,
    id: &str,
    fact_ids: Vec<String>,
    entity_type: &str,
    key: Option<String>,
) -> Result<EntitySummary, String> {
    let mut guard = cell.lock().unwrap();
    let case = guard.as_mut().ok_or(NO_CASE_OPEN)?;

    let entity_id = EntityId(id.parse().map_err(|e| format!("invalid entity id: {e}"))?);
    let fact_ids = fact_ids
        .into_iter()
        .map(|f| {
            f.parse()
                .map(eumeaus_engine::FactId)
                .map_err(|e| format!("invalid fact id: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // EntityType::from_str is infallible (SPEC.md §4.3's Custom escape
    // hatch) — same handling as do_entity_add's own entity_type.
    let entity_type: EntityType = entity_type.parse().expect("infallible");
    let new_id = case
        .split_entity(entity_id, fact_ids, entity_type, key, default_actor())
        .map_err(|e| e.to_string())?;
    case.get_entity(new_id)
        .map(EntitySummary::from)
        .map_err(|e| e.to_string())
}

/// Persists the Graph screen's drag-to-reposition (SPEC.md §9.3) —
/// `Case::set_entity_position`'s GUI-only affordance, same precedent as
/// `entity_add_fact`/`entity_add_image`. No CLI equivalent; view state, not
/// case data a `eumeaus entity` command would ever need to touch.
fn do_entity_set_position(
    cell: &Arc<Mutex<Option<Case>>>,
    id: &str,
    x: f64,
    y: f64,
) -> Result<(), String> {
    let mut guard = cell.lock().unwrap();
    let case = guard.as_mut().ok_or(NO_CASE_OPEN)?;
    let entity_id = EntityId(id.parse().map_err(|e| format!("invalid entity id: {e}"))?);
    case.set_entity_position(entity_id, x, y)
        .map_err(|e| e.to_string())
}

fn do_entity_list_positions(
    cell: &Arc<Mutex<Option<Case>>>,
) -> Result<Vec<EntityPositionDto>, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    case.list_entity_positions()
        .map(|ps| ps.into_iter().map(EntityPositionDto::from).collect())
        .map_err(|e| e.to_string())
}

fn do_relationship_add(
    cell: &Arc<Mutex<Option<Case>>>,
    from: &str,
    to: &str,
    rel_type: &str,
    attrs: Vec<AttributeInput>,
) -> Result<String, String> {
    let mut guard = cell.lock().unwrap();
    let case = guard.as_mut().ok_or(NO_CASE_OPEN)?;

    let from = EntityId(
        from.parse()
            .map_err(|e| format!("invalid entity id: {e}"))?,
    );
    let to = EntityId(to.parse().map_err(|e| format!("invalid entity id: {e}"))?);
    let rel_type: RelationshipType = rel_type.parse().expect("infallible");
    let attrs: Vec<Attribute> = attrs.into_iter().map(Attribute::from).collect();

    case.add_relationship(from, to, rel_type, attrs, manual_provenance())
        .map(|id| id.to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn entity_add(
    state: tauri::State<'_, AppState>,
    entity_type: String,
    key: Option<String>,
    attrs: Vec<AttributeInput>,
) -> Result<EntitySummary, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_add(&cell, &entity_type, key, attrs))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_add_fact(
    state: tauri::State<'_, AppState>,
    id: String,
    attrs: Vec<AttributeInput>,
) -> Result<EntityDetail, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_add_fact(&cell, &id, attrs))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_add_image(
    state: tauri::State<'_, AppState>,
    id: String,
    path: String,
) -> Result<EntityImageSummaryDto, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_add_image(&cell, &id, Path::new(&path)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_list_images(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Vec<EntityImageSummaryDto>, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_list_images(&cell, &id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_get_image(
    state: tauri::State<'_, AppState>,
    image_id: String,
) -> Result<EntityImageDataDto, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_get_image(&cell, &image_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_add_document(
    state: tauri::State<'_, AppState>,
    id: String,
    path: String,
) -> Result<EntityDocumentSummaryDto, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        do_entity_add_document(&cell, &id, Path::new(&path))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_list_documents(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Vec<EntityDocumentSummaryDto>, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_list_documents(&cell, &id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_open_document(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    document_id: String,
) -> Result<(), String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_open_document(&app, &cell, &document_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn fact_redact(
    state: tauri::State<'_, AppState>,
    fact_id: String,
    reason: String,
) -> Result<(), String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_fact_redact(&cell, &fact_id, &reason))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_merge(
    state: tauri::State<'_, AppState>,
    id1: String,
    id2: String,
) -> Result<EntitySummary, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_merge(&cell, &id1, &id2))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_split(
    state: tauri::State<'_, AppState>,
    id: String,
    fact_ids: Vec<String>,
    entity_type: String,
    key: Option<String>,
) -> Result<EntitySummary, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        do_entity_split(&cell, &id, fact_ids, &entity_type, key)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_set_position(
    state: tauri::State<'_, AppState>,
    id: String,
    x: f64,
    y: f64,
) -> Result<(), String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_set_position(&cell, &id, x, y))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_list_positions(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<EntityPositionDto>, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_list_positions(&cell))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn relationship_add(
    state: tauri::State<'_, AppState>,
    from: String,
    to: String,
    rel_type: String,
    attrs: Vec<AttributeInput>,
) -> Result<String, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        do_relationship_add(&cell, &from, &to, &rel_type, attrs)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Debug)]
pub struct EntityPositionDto {
    pub entity_id: String,
    pub x: f64,
    pub y: f64,
}

impl From<EntityPosition> for EntityPositionDto {
    fn from(p: EntityPosition) -> Self {
        EntityPositionDto {
            entity_id: p.entity_id.to_string(),
            x: p.x,
            y: p.y,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct RelationshipDto {
    pub id: String,
    pub from: String,
    pub to: String,
    pub relationship_type: String,
    pub created_at_unix_ms: i64,
}

impl From<Relationship> for RelationshipDto {
    fn from(r: Relationship) -> Self {
        RelationshipDto {
            id: r.id.to_string(),
            from: r.from.to_string(),
            to: r.to.to_string(),
            relationship_type: r.relationship_type.to_string(),
            created_at_unix_ms: r.created_at_unix_ms,
        }
    }
}

fn do_relationship_list(cell: &Arc<Mutex<Option<Case>>>) -> Result<Vec<RelationshipDto>, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    case.list_relationships()
        .map(|rels| rels.into_iter().map(RelationshipDto::from).collect())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn relationship_list(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<RelationshipDto>, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_relationship_list(&cell))
        .await
        .map_err(|e| e.to_string())?
}

fn do_entity_audit(
    cell: &Arc<Mutex<Option<Case>>>,
    id: &str,
) -> Result<Vec<crate::overview_state::AuditEventDto>, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    let entity_id = EntityId(id.parse().map_err(|e| format!("invalid entity id: {e}"))?);
    case.audit_trail(eumeaus_engine::AuditTarget::Entity(entity_id))
        .map(|events| {
            events
                .into_iter()
                .map(crate::overview_state::AuditEventDto::from)
                .collect()
        })
        .map_err(|e| e.to_string())
}

/// One entity's own history — the design's "History" tab (SPEC.md §9.3),
/// distinct from `audit_list`'s case-wide feed (Overview). Reuses
/// `Case::audit_trail` (existing since M2), not the new `audit_trail_all`.
#[tauri::command]
pub async fn entity_audit(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Vec<crate::overview_state::AuditEventDto>, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_audit(&cell, &id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_list(
    state: tauri::State<'_, AppState>,
    entity_type: Option<String>,
) -> Result<Vec<EntitySummary>, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_list(&cell, entity_type))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_show(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<EntityDetail, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_show(&cell, &id))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_cell_with_case(case: Case) -> Arc<Mutex<Option<Case>>> {
        Arc::new(Mutex::new(Some(case)))
    }

    #[test]
    fn list_and_show_error_cleanly_with_no_case_open() {
        let cell: Arc<Mutex<Option<Case>>> = Arc::new(Mutex::new(None));
        assert_eq!(do_entity_list(&cell, None).unwrap_err(), NO_CASE_OPEN);
        assert_eq!(
            do_entity_show(&cell, "00000000-0000-0000-0000-000000000000").unwrap_err(),
            NO_CASE_OPEN
        );
    }

    #[test]
    fn show_rejects_a_malformed_id_before_touching_the_case() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "g2-bad-id").unwrap();
        let cell = tmp_cell_with_case(case);

        let err = do_entity_show(&cell, "not-a-uuid").unwrap_err();
        assert!(err.contains("invalid entity id"));
    }

    // The real data-shape proof (SPEC.md §9.6 G2's verify bar): an entity
    // added exactly the way eumeaus-cli's `entity add` does (Case::add_entity
    // with user provenance) lists and shows through these commands with no
    // translation bugs — same ids, same attribute/fact fields the CLI's
    // own `entity show` prints.
    #[test]
    fn cli_created_entity_lists_and_shows_correctly() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g2-roundtrip").unwrap();

        let entity_id = case
            .add_entity(
                eumeaus_engine::EntityType::Username,
                Some("octocat".to_string()),
                vec![Attribute {
                    key: "bio".to_string(),
                    value: "test account".to_string(),
                }],
                manual_provenance(),
            )
            .unwrap();

        let cell = tmp_cell_with_case(case);

        let listed = do_entity_list(&cell, None).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, entity_id.to_string());
        assert_eq!(listed[0].entity_type, "Username");
        assert_eq!(listed[0].canonical_key.as_deref(), Some("octocat"));

        let detail = do_entity_show(&cell, &entity_id.to_string()).unwrap();
        assert_eq!(detail.summary.id, entity_id.to_string());
        assert_eq!(detail.attributes.len(), 1);
        assert_eq!(detail.attributes[0].key, "bio");
        assert_eq!(detail.attributes[0].value, "test account");
        assert_eq!(detail.attributes[0].source, "user");
        assert!(detail.attributes[0].is_current);
    }

    #[test]
    fn list_filters_by_entity_type() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g2-filter").unwrap();
        case.add_entity(
            eumeaus_engine::EntityType::Username,
            Some("a".to_string()),
            vec![],
            manual_provenance(),
        )
        .unwrap();
        case.add_entity(
            eumeaus_engine::EntityType::Email,
            Some("b@example.com".to_string()),
            vec![],
            manual_provenance(),
        )
        .unwrap();

        let cell = tmp_cell_with_case(case);
        let usernames = do_entity_list(&cell, Some("Username".to_string())).unwrap();
        assert_eq!(usernames.len(), 1);
        assert_eq!(usernames[0].entity_type, "Username");
    }

    #[test]
    fn entity_add_creates_and_returns_the_new_entity() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "g4-add").unwrap();
        let cell = tmp_cell_with_case(case);

        let created = do_entity_add(
            &cell,
            "Email",
            Some("eve@example.com".to_string()),
            vec![AttributeInput {
                key: "verified".to_string(),
                value: "true".to_string(),
            }],
        )
        .unwrap();

        assert_eq!(created.entity_type, "Email");
        assert_eq!(created.canonical_key.as_deref(), Some("eve@example.com"));

        let shown = do_entity_show(&cell, &created.id).unwrap();
        assert_eq!(shown.attributes.len(), 1);
        assert_eq!(shown.attributes[0].key, "verified");
    }

    #[test]
    fn entity_add_fact_adds_to_a_keyless_entity_without_duplicating_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-add-fact").unwrap();
        let id = case
            .add_entity(
                EntityType::Person,
                None,
                vec![Attribute {
                    key: "name".to_string(),
                    value: "Alice".to_string(),
                }],
                manual_provenance(),
            )
            .unwrap();
        let cell = tmp_cell_with_case(case);

        let detail = do_entity_add_fact(
            &cell,
            &id.to_string(),
            vec![AttributeInput {
                key: "note".to_string(),
                value: "seen at the office".to_string(),
            }],
        )
        .unwrap();

        assert_eq!(detail.summary.id, id.to_string());
        assert_eq!(detail.attributes.len(), 2);
        assert!(detail.attributes.iter().any(|a| a.key == "note"));

        // Still exactly one Person — the bug this replaces would have
        // silently created a second entity via entity_add's no-key path.
        let all = do_entity_list(&cell, Some("Person".to_string())).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn entity_add_fact_errors_cleanly_with_no_case_open() {
        let cell: Arc<Mutex<Option<Case>>> = Arc::new(Mutex::new(None));
        assert_eq!(
            do_entity_add_fact(&cell, "00000000-0000-0000-0000-000000000000", vec![]).unwrap_err(),
            NO_CASE_OPEN
        );
    }

    #[test]
    fn entity_add_image_rejects_an_unsupported_extension() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "g-image-bad-ext").unwrap();
        let cell = tmp_cell_with_case(case);

        let bad_path = dir.path().join("not-an-image.txt");
        std::fs::write(&bad_path, b"hello").unwrap();

        let err = do_entity_add_image(&cell, "00000000-0000-0000-0000-000000000000", &bad_path)
            .unwrap_err();
        assert!(err.contains("unsupported image type"));
    }

    #[test]
    fn entity_add_image_rejects_an_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-image-too-big").unwrap();
        let id = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        let cell = tmp_cell_with_case(case);

        let big_path = dir.path().join("huge.png");
        let oversized = vec![0u8; (MAX_IMAGE_BYTES + 1) as usize];
        std::fs::write(&big_path, &oversized).unwrap();

        let err = do_entity_add_image(&cell, &id.to_string(), &big_path).unwrap_err();
        assert!(err.contains("too large"), "unexpected error: {err}");
    }

    #[test]
    fn entity_add_image_round_trips_through_list_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-image-roundtrip").unwrap();
        let id = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        let cell = tmp_cell_with_case(case);

        let image_path = dir.path().join("avatar.png");
        std::fs::write(&image_path, [1, 2, 3, 4, 5]).unwrap();

        let added = do_entity_add_image(&cell, &id.to_string(), &image_path).unwrap();
        assert_eq!(added.mime_type, "image/png");
        assert!(added.is_current);

        let listed = do_entity_list_images(&cell, &id.to_string()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, added.id);

        let fetched = do_entity_get_image(&cell, &added.id).unwrap();
        assert_eq!(fetched.mime_type, "image/png");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&fetched.data_base64)
            .unwrap();
        assert_eq!(decoded, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn fact_redact_removes_an_image() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-image-redact").unwrap();
        let id = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        let cell = tmp_cell_with_case(case);

        let image_path = dir.path().join("avatar.png");
        std::fs::write(&image_path, [9, 9, 9]).unwrap();
        let added = do_entity_add_image(&cell, &id.to_string(), &image_path).unwrap();

        do_fact_redact(&cell, &added.fact_id, "wrong photo").unwrap();

        let listed = do_entity_list_images(&cell, &id.to_string()).unwrap();
        assert!(listed.is_empty());
    }

    #[test]
    fn image_commands_error_cleanly_with_no_case_open() {
        let cell: Arc<Mutex<Option<Case>>> = Arc::new(Mutex::new(None));
        let dir = tempfile::tempdir().unwrap();
        let image_path = dir.path().join("avatar.png");
        std::fs::write(&image_path, [1]).unwrap();

        assert_eq!(
            do_entity_add_image(&cell, "00000000-0000-0000-0000-000000000000", &image_path)
                .unwrap_err(),
            NO_CASE_OPEN
        );
        assert_eq!(
            do_entity_list_images(&cell, "00000000-0000-0000-0000-000000000000").unwrap_err(),
            NO_CASE_OPEN
        );
        assert_eq!(
            do_entity_get_image(&cell, "00000000-0000-0000-0000-000000000000").unwrap_err(),
            NO_CASE_OPEN
        );
        assert_eq!(
            do_fact_redact(&cell, "00000000-0000-0000-0000-000000000000", "reason").unwrap_err(),
            NO_CASE_OPEN
        );
    }

    #[test]
    fn entity_add_document_rejects_an_unsupported_extension() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "g-doc-bad-ext").unwrap();
        let cell = tmp_cell_with_case(case);

        let bad_path = dir.path().join("not-a-document.png");
        std::fs::write(&bad_path, b"hello").unwrap();

        let err = do_entity_add_document(&cell, "00000000-0000-0000-0000-000000000000", &bad_path)
            .unwrap_err();
        assert!(err.contains("unsupported document type"));
    }

    #[test]
    fn entity_add_document_rejects_an_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-doc-too-big").unwrap();
        let id = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        let cell = tmp_cell_with_case(case);

        let big_path = dir.path().join("huge.pdf");
        let oversized = vec![0u8; (MAX_DOCUMENT_BYTES + 1) as usize];
        std::fs::write(&big_path, &oversized).unwrap();

        let err = do_entity_add_document(&cell, &id.to_string(), &big_path).unwrap_err();
        assert!(err.contains("too large"), "unexpected error: {err}");
    }

    #[test]
    fn entity_add_document_round_trips_through_list_and_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-doc-roundtrip").unwrap();
        let id = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        let cell = tmp_cell_with_case(case);

        let doc_path = dir.path().join("notes.txt");
        std::fs::write(&doc_path, b"case notes").unwrap();

        let added = do_entity_add_document(&cell, &id.to_string(), &doc_path).unwrap();
        assert_eq!(added.mime_type, "text/plain");
        assert_eq!(added.file_name, "notes.txt");

        let listed = do_entity_list_documents(&cell, &id.to_string()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, added.id);

        let temp_path = write_document_to_temp_file(&cell, &added.id).unwrap();
        assert_eq!(std::fs::read(&temp_path).unwrap(), b"case notes");
        assert!(
            temp_path.file_name().unwrap().to_string_lossy().ends_with("notes.txt"),
            "temp file should keep the original extension so the OS opener picks the right handler: {temp_path:?}"
        );
    }

    #[test]
    fn write_document_to_temp_file_sanitizes_a_path_traversal_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-doc-traversal").unwrap();
        let id = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        case.add_document_to_entity(
            id,
            "../../etc/passwd".to_string(),
            "text/plain".to_string(),
            b"not actually passwd".to_vec(),
            eumeaus_engine::Provenance {
                source: "user".to_string(),
                source_version: "0".to_string(),
                source_url: None,
                retrieval_method: None,
                raw_response_sha256: None,
                collected_at_unix_ms: 0,
            },
        )
        .unwrap();
        let doc_id = case
            .list_entity_documents(id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .id
            .to_string();
        let cell = tmp_cell_with_case(case);

        let temp_path = write_document_to_temp_file(&cell, &doc_id).unwrap();
        assert!(
            temp_path.starts_with(std::env::temp_dir()),
            "must stay inside the temp dir, got {temp_path:?}"
        );
        assert_eq!(std::fs::read(&temp_path).unwrap(), b"not actually passwd");
    }

    #[test]
    fn fact_redact_removes_a_document() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-doc-redact").unwrap();
        let id = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        let cell = tmp_cell_with_case(case);

        let doc_path = dir.path().join("notes.txt");
        std::fs::write(&doc_path, b"case notes").unwrap();
        let added = do_entity_add_document(&cell, &id.to_string(), &doc_path).unwrap();

        do_fact_redact(&cell, &added.fact_id, "wrong document").unwrap();

        let listed = do_entity_list_documents(&cell, &id.to_string()).unwrap();
        assert!(listed.is_empty());
    }

    #[test]
    fn document_commands_error_cleanly_with_no_case_open() {
        let cell: Arc<Mutex<Option<Case>>> = Arc::new(Mutex::new(None));
        let dir = tempfile::tempdir().unwrap();
        let doc_path = dir.path().join("notes.txt");
        std::fs::write(&doc_path, [1]).unwrap();

        assert_eq!(
            do_entity_add_document(&cell, "00000000-0000-0000-0000-000000000000", &doc_path)
                .unwrap_err(),
            NO_CASE_OPEN
        );
        assert_eq!(
            do_entity_list_documents(&cell, "00000000-0000-0000-0000-000000000000").unwrap_err(),
            NO_CASE_OPEN
        );
        assert_eq!(
            write_document_to_temp_file(&cell, "00000000-0000-0000-0000-000000000000").unwrap_err(),
            NO_CASE_OPEN
        );
    }

    // SPEC.md §9.6 G4's actual verify bar: a GUI-initiated merge produces
    // the *same* audit_events row shape crud.rs's own
    // merge_entities_moves_facts_and_records_audit_event test already
    // checks for a directly-called (i.e. CLI-equivalent) merge — same
    // event_type, same single-row count, same actor name.
    #[test]
    fn entity_merge_produces_the_same_audit_event_shape_the_cli_relies_on() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g4-merge").unwrap();
        let a = case
            .add_entity(
                EntityType::Person,
                None,
                vec![Attribute {
                    key: "name".to_string(),
                    value: "Alice".to_string(),
                }],
                manual_provenance(),
            )
            .unwrap();
        let b = case
            .add_entity(
                EntityType::Person,
                None,
                vec![Attribute {
                    key: "nickname".to_string(),
                    value: "Al".to_string(),
                }],
                manual_provenance(),
            )
            .unwrap();

        let cell = tmp_cell_with_case(case);
        let survivor = do_entity_merge(&cell, &a.to_string(), &b.to_string()).unwrap();
        assert_eq!(survivor.id, a.to_string());

        let guard = cell.lock().unwrap();
        let events = guard
            .as_ref()
            .unwrap()
            .audit_trail(eumeaus_engine::AuditTarget::Entity(a))
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "merge");
        assert_eq!(events[0].actor, "user");
    }

    #[test]
    fn entity_split_produces_the_same_audit_event_shape_the_cli_relies_on() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g4-split").unwrap();
        let id = case
            .add_entity(
                EntityType::Person,
                None,
                vec![
                    Attribute {
                        key: "name".to_string(),
                        value: "Alice".to_string(),
                    },
                    Attribute {
                        key: "alias".to_string(),
                        value: "Bob".to_string(),
                    },
                ],
                manual_provenance(),
            )
            .unwrap();
        let alias_fact_id = case
            .list_attribute_records(id)
            .unwrap()
            .into_iter()
            .find(|a| a.key == "alias")
            .unwrap()
            .fact_id;

        let cell = tmp_cell_with_case(case);
        let new_entity = do_entity_split(
            &cell,
            &id.to_string(),
            vec![alias_fact_id.to_string()],
            "Username",
            Some("Bob".to_string()),
        )
        .unwrap();
        assert_ne!(new_entity.id, id.to_string());
        assert_eq!(new_entity.entity_type, "Username");
        assert_eq!(new_entity.canonical_key.as_deref(), Some("bob"));

        let guard = cell.lock().unwrap();
        let case_ref = guard.as_ref().unwrap();
        let original_events = case_ref
            .audit_trail(eumeaus_engine::AuditTarget::Entity(id))
            .unwrap();
        assert_eq!(original_events.len(), 1);
        assert_eq!(original_events[0].event_type, "split");
        assert_eq!(original_events[0].actor, "user");
    }

    #[test]
    fn entity_set_position_round_trips_through_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-position-roundtrip").unwrap();
        let id = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        let cell = tmp_cell_with_case(case);

        do_entity_set_position(&cell, &id.to_string(), 12.5, -3.0).unwrap();

        let listed = do_entity_list_positions(&cell).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].entity_id, id.to_string());
        assert_eq!(listed[0].x, 12.5);
        assert_eq!(listed[0].y, -3.0);
    }

    #[test]
    fn entity_set_position_errors_cleanly_with_no_case_open() {
        let cell: Arc<Mutex<Option<Case>>> = Arc::new(Mutex::new(None));
        assert_eq!(
            do_entity_set_position(&cell, "00000000-0000-0000-0000-000000000000", 0.0, 0.0)
                .unwrap_err(),
            NO_CASE_OPEN
        );
        assert_eq!(do_entity_list_positions(&cell).unwrap_err(), NO_CASE_OPEN);
    }

    #[test]
    fn relationship_add_creates_a_relationship_between_two_entities() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g4-relationship").unwrap();
        let a = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        let b = case
            .add_entity(EntityType::Organization, None, vec![], manual_provenance())
            .unwrap();

        let cell = tmp_cell_with_case(case);
        let rel_id = do_relationship_add(
            &cell,
            &a.to_string(),
            &b.to_string(),
            "MemberOf",
            vec![AttributeInput {
                key: "role".to_string(),
                value: "engineer".to_string(),
            }],
        )
        .unwrap();

        assert!(uuid::Uuid::parse_str(&rel_id).is_ok());
    }

    #[test]
    fn write_commands_error_cleanly_with_no_case_open() {
        let cell: Arc<Mutex<Option<Case>>> = Arc::new(Mutex::new(None));
        assert_eq!(
            do_entity_add(&cell, "Person", None, vec![]).unwrap_err(),
            NO_CASE_OPEN
        );
        assert_eq!(
            do_entity_merge(
                &cell,
                "00000000-0000-0000-0000-000000000000",
                "00000000-0000-0000-0000-000000000001"
            )
            .unwrap_err(),
            NO_CASE_OPEN
        );
        assert_eq!(
            do_entity_split(
                &cell,
                "00000000-0000-0000-0000-000000000000",
                vec![],
                "Person",
                None
            )
            .unwrap_err(),
            NO_CASE_OPEN
        );
        assert_eq!(
            do_relationship_add(
                &cell,
                "00000000-0000-0000-0000-000000000000",
                "00000000-0000-0000-0000-000000000001",
                "MemberOf",
                vec![],
            )
            .unwrap_err(),
            NO_CASE_OPEN
        );
        assert_eq!(do_relationship_list(&cell).unwrap_err(), NO_CASE_OPEN);
    }

    #[test]
    fn relationship_list_shows_a_relationship_added_via_the_same_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-graph-rels").unwrap();
        let a = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        let b = case
            .add_entity(EntityType::Organization, None, vec![], manual_provenance())
            .unwrap();
        let cell = tmp_cell_with_case(case);

        let rel_id =
            do_relationship_add(&cell, &a.to_string(), &b.to_string(), "MemberOf", vec![]).unwrap();

        let rels = do_relationship_list(&cell).unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].id, rel_id);
        assert_eq!(rels[0].from, a.to_string());
        assert_eq!(rels[0].to, b.to_string());
        assert_eq!(rels[0].relationship_type, "MemberOf");
    }

    #[test]
    fn entity_audit_shows_only_that_entitys_events() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-entity-audit").unwrap();
        let a = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        let b = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        case.merge_entities(a, b, default_actor()).unwrap();
        let cell = tmp_cell_with_case(case);

        let events = do_entity_audit(&cell, &a.to_string()).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "merge");
    }
}
