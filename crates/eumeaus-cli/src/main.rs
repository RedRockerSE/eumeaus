//! `eumeaus-cli` — thin wrapper translating CLI subcommands into
//! `eumeaus-engine` API calls. This is also the end-to-end test surface for
//! v1 (see `tests/e2e_case_lifecycle.rs`).
//!
//! Case lifecycle (M1) and entity/relationship CRUD, merge/split, and audit
//! trail (M2) are wired to real engine calls. Plugin/scan/credential
//! management (M3+) still print their own "not yet implemented" message.

use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use clap::{Parser, Subcommand};
use eumeaus_engine::{
    Actor, Attribute, AuditTarget, Case, EngineError, EntityFilter, EntityId, EntityType,
    ExportFormat, FactId, Provenance, RelationshipId, RelationshipType, ScanId,
};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("not yet implemented: {0}")]
    NotImplemented(String),
    #[error("invalid id {0:?}: {1}")]
    InvalidId(String, uuid::Error),
    #[error("specify exactly one of --entity, --relationship, --scan")]
    AmbiguousAuditTarget,
}

#[derive(Parser)]
#[command(name = "eumeaus", version, about = "Local-first OSINT case management")]
struct Cli {
    /// Path to the open case file. Required by any subcommand that touches
    /// case data (entity/relationship/scan/audit).
    #[arg(long, global = true)]
    case: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(subcommand)]
    Case(CaseCmd),
    #[command(subcommand)]
    Entity(EntityCmd),
    #[command(subcommand)]
    Relationship(RelationshipCmd),
    #[command(subcommand)]
    Plugin(PluginCmd),
    #[command(subcommand)]
    Scan(ScanCmd),
    #[command(subcommand)]
    Credential(CredentialCmd),
    #[command(subcommand)]
    Audit(AuditCmd),
}

#[derive(Subcommand)]
enum AuditCmd {
    Show {
        #[arg(long)]
        entity: Option<String>,
        #[arg(long)]
        relationship: Option<String>,
        #[arg(long)]
        scan: Option<String>,
    },
}

#[derive(Subcommand)]
enum CaseCmd {
    Create {
        name: String,
        #[arg(long)]
        path: Option<PathBuf>,
    },
    Open {
        path: PathBuf,
    },
    List,
    Export {
        path: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value = "sqlite")]
        format: String,
    },
}

#[derive(Subcommand)]
enum EntityCmd {
    Add {
        #[arg(long = "type")]
        entity_type: String,
        #[arg(long)]
        key: Option<String>,
        #[arg(long = "attr")]
        attrs: Vec<String>,
    },
    List {
        #[arg(long = "type")]
        entity_type: Option<String>,
        #[arg(long)]
        filter: Option<String>,
    },
    Show {
        id: String,
    },
    Merge {
        id1: String,
        id2: String,
    },
    Split {
        id: String,
        #[arg(long)]
        facts: String,
    },
}

#[derive(Subcommand)]
enum RelationshipCmd {
    Add {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long = "type")]
        rel_type: String,
        #[arg(long = "attr")]
        attrs: Vec<String>,
    },
}

#[derive(Subcommand)]
enum PluginCmd {
    List {
        #[arg(long)]
        installed: bool,
        #[arg(long)]
        available: bool,
    },
    Install {
        path: PathBuf,
    },
    Verify {
        name: String,
    },
}

#[derive(Subcommand)]
enum ScanCmd {
    Run {
        #[arg(long)]
        plugin: String,
        #[arg(long = "target-type")]
        target_type: String,
        #[arg(long = "target-value")]
        target_value: String,
        #[arg(long = "rate-limit")]
        rate_limit: Option<u32>,
        #[arg(long)]
        proxy: Option<String>,
        #[arg(long = "worker-pool")]
        worker_pool: Option<u32>,
    },
    Status {
        scan_id: String,
    },
    Resume {
        scan_id: String,
    },
    List,
}

#[derive(Subcommand)]
enum CredentialCmd {
    Set { name: String },
    List,
    Remove { name: String },
}

fn not_implemented(op: &str) -> Result<(), CliError> {
    Err(CliError::NotImplemented(op.to_string()))
}

fn parse_attrs(raw: &[String]) -> Vec<Attribute> {
    raw.iter()
        .filter_map(|kv| kv.split_once('='))
        .map(|(key, value)| Attribute {
            key: key.to_string(),
            value: value.to_string(),
        })
        .collect()
}

fn parse_uuid(raw: &str) -> Result<Uuid, CliError> {
    Uuid::parse_str(raw).map_err(|e| CliError::InvalidId(raw.to_string(), e))
}

fn parse_entity_id(raw: &str) -> Result<EntityId, CliError> {
    Ok(EntityId(parse_uuid(raw)?))
}

fn parse_relationship_id(raw: &str) -> Result<RelationshipId, CliError> {
    Ok(RelationshipId(parse_uuid(raw)?))
}

fn parse_fact_ids(raw: &str) -> Result<Vec<FactId>, CliError> {
    raw.split(',')
        .map(str::trim)
        .map(|id| parse_uuid(id).map(FactId))
        .collect()
}

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

/// No identity/auth system yet (SPEC.md doesn't specify one for v1's CLI),
/// so every manual edit is attributed to this fixed actor.
fn default_actor() -> Actor {
    Actor {
        name: "user".to_string(),
    }
}

fn require_case(case: &Option<PathBuf>) -> Result<Case, CliError> {
    let path = case.clone().unwrap_or_else(|| PathBuf::from("."));
    Case::open(&path).map_err(CliError::from)
}

fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Commands::Case(cmd) => match cmd {
            CaseCmd::Create { name, path } => {
                let dir = path.unwrap_or_else(|| PathBuf::from("."));
                Case::create(&dir, &name)
                    .map(|_| ())
                    .map_err(CliError::from)
            }
            CaseCmd::Open { path } => Case::open(&path).map(|_| ()).map_err(CliError::from),
            CaseCmd::List => not_implemented("case list"),
            CaseCmd::Export { path, out, format } => {
                let case = Case::open(&path)?;
                let format = match format.as_str() {
                    "report" => ExportFormat::Report,
                    _ => ExportFormat::Sqlite,
                };
                case.export(&out, format).map_err(CliError::from)
            }
        },
        Commands::Entity(cmd) => match cmd {
            EntityCmd::Add {
                entity_type,
                key,
                attrs,
            } => {
                let mut case = require_case(&cli.case)?;
                let id = case.add_entity(
                    EntityType::from_str(&entity_type).expect("EntityType::from_str is infallible"),
                    key,
                    parse_attrs(&attrs),
                    manual_provenance(),
                )?;
                println!("{id}");
                Ok(())
            }
            EntityCmd::List { entity_type, .. } => {
                let case = require_case(&cli.case)?;
                let entities = case.list_entities(EntityFilter {
                    entity_type: entity_type.map(|t| EntityType::from_str(&t).expect("infallible")),
                })?;
                for entity in entities {
                    println!(
                        "{}\t{}\t{}\t{}",
                        entity.id,
                        entity.entity_type,
                        entity.canonical_key.as_deref().unwrap_or("-"),
                        entity.display_label,
                    );
                }
                Ok(())
            }
            EntityCmd::Show { id } => {
                let case = require_case(&cli.case)?;
                let entity_id = parse_entity_id(&id)?;
                let entity = case.get_entity(entity_id)?;
                println!("id:            {}", entity.id);
                println!("type:          {}", entity.entity_type);
                println!(
                    "canonical_key: {}",
                    entity.canonical_key.as_deref().unwrap_or("-")
                );
                println!("label:         {}", entity.display_label);

                let attrs = case.list_attribute_records(entity_id)?;
                if attrs.is_empty() {
                    println!("attributes:    (none)");
                } else {
                    println!("attributes:");
                    for a in attrs {
                        let flag = match (a.is_current, a.conflicting) {
                            (true, true) => "* (conflict, other values exist)",
                            (true, false) => "*",
                            (false, _) => " ",
                        };
                        println!(
                            "  {flag} {} = {} (source: {}, collected_at: {})",
                            a.key, a.value, a.source, a.collected_at_unix_ms
                        );
                    }
                }
                Ok(())
            }
            EntityCmd::Merge { id1, id2 } => {
                let mut case = require_case(&cli.case)?;
                let survivor = case.merge_entities(
                    parse_entity_id(&id1)?,
                    parse_entity_id(&id2)?,
                    default_actor(),
                )?;
                println!("{survivor}");
                Ok(())
            }
            EntityCmd::Split { id, facts } => {
                let mut case = require_case(&cli.case)?;
                let new_id = case.split_entity(
                    parse_entity_id(&id)?,
                    parse_fact_ids(&facts)?,
                    default_actor(),
                )?;
                println!("{new_id}");
                Ok(())
            }
        },
        Commands::Relationship(cmd) => match cmd {
            RelationshipCmd::Add {
                from,
                to,
                rel_type,
                attrs,
            } => {
                let mut case = require_case(&cli.case)?;
                let id = case.add_relationship(
                    parse_entity_id(&from)?,
                    parse_entity_id(&to)?,
                    RelationshipType::from_str(&rel_type).expect("infallible"),
                    parse_attrs(&attrs),
                    manual_provenance(),
                )?;
                println!("{id}");
                Ok(())
            }
        },
        Commands::Plugin(cmd) => match cmd {
            PluginCmd::List { .. } => not_implemented("plugin list"),
            PluginCmd::Install { .. } => not_implemented("plugin install"),
            PluginCmd::Verify { .. } => not_implemented("plugin verify"),
        },
        Commands::Scan(cmd) => match cmd {
            ScanCmd::Run { .. } => {
                let mut case = require_case(&cli.case)?;
                case.start_scan(
                    eumeaus_engine::PluginRef {
                        name: "unset".to_string(),
                    },
                    eumeaus_engine::TargetEntity {
                        id: eumeaus_engine::EntityId(uuid::Uuid::nil()),
                    },
                    eumeaus_engine::ScanConfig::default(),
                )
                .map(|_| ())
                .map_err(CliError::from)
            }
            ScanCmd::Status { .. } => not_implemented("scan status"),
            ScanCmd::Resume { .. } => not_implemented("scan resume"),
            ScanCmd::List => not_implemented("scan list"),
        },
        Commands::Credential(cmd) => match cmd {
            CredentialCmd::Set { .. } => not_implemented("credential set"),
            CredentialCmd::List => not_implemented("credential list"),
            CredentialCmd::Remove { .. } => not_implemented("credential remove"),
        },
        Commands::Audit(AuditCmd::Show {
            entity,
            relationship,
            scan,
        }) => {
            let case = require_case(&cli.case)?;
            let target = match (entity, relationship, scan) {
                (Some(id), None, None) => AuditTarget::Entity(parse_entity_id(&id)?),
                (None, Some(id), None) => AuditTarget::Relationship(parse_relationship_id(&id)?),
                (None, None, Some(id)) => AuditTarget::Scan(ScanId(parse_uuid(&id)?)),
                _ => return Err(CliError::AmbiguousAuditTarget),
            };
            for event in case.audit_trail(target)? {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    event.occurred_at_unix_ms,
                    event.event_type,
                    event.actor,
                    event.id,
                    event.description
                );
            }
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
