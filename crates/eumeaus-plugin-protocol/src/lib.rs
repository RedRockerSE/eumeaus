//! Wire contract between `eumeaus-engine` (via `eumeaus-plugin-host`) and
//! plugin subprocesses. Generated from `plugin.proto` by build.rs; no
//! hand-written logic belongs in this crate beyond that.

// tonic-generated server trait methods return Result<Response<T>, Status>,
// and tonic::Status itself is >128 bytes — inherent to every tonic service,
// not something generated code (or its implementors, e.g. eumeaus-plugin-sdk)
// can restructure without diverging from tonic's own trait shape.
#![allow(clippy::result_large_err)]

tonic::include_proto!("eumeaus.plugin.v1");
