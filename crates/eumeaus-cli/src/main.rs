//! `eumeaus-cli` — thin wrapper translating CLI subcommands into
//! `eumeaus-engine` API calls. This is also the end-to-end test surface for
//! v1 (see `tests/e2e_case_lifecycle.rs`).
//!
//! STUB CRATE (milestone M0). Every subcommand either calls a still-stubbed
//! engine method (surfacing `EngineError::NotImplemented`) or, for surface
//! not yet backed by any crate (plugin/credential management, case/scan
//! listing), prints its own "not yet implemented" message.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use eumeaus_engine::{
    Attribute, Case, EngineError, EntityFilter, EntityType, ExportFormat, Provenance,
    RelationshipType,
};

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("not yet implemented: {0}")]
    NotImplemented(String),
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
    Audit {
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

fn parse_entity_type(s: &str) -> EntityType {
    match s {
        "Person" => EntityType::Person,
        "Username" => EntityType::Username,
        "Email" => EntityType::Email,
        "PhoneNumber" => EntityType::PhoneNumber,
        "Domain" => EntityType::Domain,
        "IPAddress" => EntityType::IpAddress,
        "OnlineAccount" => EntityType::OnlineAccount,
        "Organization" => EntityType::Organization,
        "Location" => EntityType::Location,
        "Document" => EntityType::Document,
        "Image" => EntityType::Image,
        "Vehicle" => EntityType::Vehicle,
        other => EntityType::Custom(other.to_string()),
    }
}

fn parse_relationship_type(s: &str) -> RelationshipType {
    match s {
        "HasAccount" => RelationshipType::HasAccount,
        "Owns" => RelationshipType::Owns,
        "AssociatedWith" => RelationshipType::AssociatedWith,
        "LocatedAt" => RelationshipType::LocatedAt,
        "MemberOf" => RelationshipType::MemberOf,
        "ResolvesTo" => RelationshipType::ResolvesTo,
        "Mentions" => RelationshipType::Mentions,
        _ => RelationshipType::RelatedTo,
    }
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

fn manual_provenance() -> Provenance {
    Provenance {
        source: "user".to_string(),
        source_url: None,
        retrieval_method: None,
        raw_response_sha256: None,
        collected_at_unix_ms: 0,
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
                case.add_entity(
                    parse_entity_type(&entity_type),
                    key,
                    parse_attrs(&attrs),
                    manual_provenance(),
                )
                .map(|_| ())
                .map_err(CliError::from)
            }
            EntityCmd::List { entity_type, .. } => {
                let case = require_case(&cli.case)?;
                case.list_entities(EntityFilter {
                    entity_type: entity_type.map(|t| parse_entity_type(&t)),
                })
                .map(|_| ())
                .map_err(CliError::from)
            }
            EntityCmd::Show { .. } => not_implemented("entity show"),
            EntityCmd::Merge { .. } => not_implemented("entity merge"),
            EntityCmd::Split { .. } => not_implemented("entity split"),
        },
        Commands::Relationship(cmd) => match cmd {
            RelationshipCmd::Add {
                rel_type, attrs, ..
            } => {
                let mut case = require_case(&cli.case)?;
                case.add_relationship(
                    eumeaus_engine::EntityId(uuid::Uuid::nil()),
                    eumeaus_engine::EntityId(uuid::Uuid::nil()),
                    parse_relationship_type(&rel_type),
                    parse_attrs(&attrs),
                    manual_provenance(),
                )
                .map(|_| ())
                .map_err(CliError::from)
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
        Commands::Audit { .. } => not_implemented("audit show"),
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
