//! Wire contract between `eumeaus-engine` (via `eumeaus-plugin-host`) and
//! plugin subprocesses. Generated from `plugin.proto` by build.rs; no
//! hand-written logic belongs in this crate beyond that.

tonic::include_proto!("eumeaus.plugin.v1");
