fn main() {
    // No system protoc is assumed to be installed (see CLAUDE.md); use the
    // vendored binary instead of requiring one.
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc binary");
    std::env::set_var("PROTOC", protoc);

    println!("cargo:rerun-if-changed=plugin.proto");
    tonic_build::compile_protos("plugin.proto").expect("compile plugin.proto");
}
