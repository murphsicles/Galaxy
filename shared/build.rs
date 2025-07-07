// shared/build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = std::env::var("PROTOC").unwrap_or_else(|_| "protoc".to_string());
    println!("cargo:warning=Using protoc: {}", protoc);
    if !std::path::Path::new(&protoc).exists() {
        eprintln!("protoc binary not found at: {}", protoc);
        std::process::exit(1);
    }
    let src_dir = std::path::Path::new("src");
    if !src_dir.exists() {
        std::fs::create_dir_all(src_dir)?;
        println!("cargo:warning=Created src directory");
    }
    for proto_file in [
        "../protos/network.proto",
        "../protos/transaction.proto",
        "../protos/block.proto",
        "../protos/storage.proto",
        "../protos/consensus.proto",
        "../protos/overlay.proto",
        "../protos/validation.proto",
        "../protos/mining.proto",
        "../protos/auth.proto",
        "../protos/alert.proto",
        "../protos/index.proto",
        "../protos/metrics.proto",
    ] {
        if !std::path::Path::new(proto_file).exists() {
            eprintln!("Proto file not found: {}", proto_file);
            std::process::exit(1);
        }
    }
    for include_dir in ["../protos", "../protos/google/api", "../protos/google/protobuf"] {
        if !std::path::Path::new(include_dir).exists() {
            eprintln!("Include directory not found: {}", include_dir);
            std::process::exit(1);
        }
    }
    tonic_build::configure()
        .build_server(true)
        .out_dir("src")
        .compile_protos(
            &[
                "../protos/network.proto",
                "../protos/transaction.proto",
                "../protos/block.proto",
                "../protos/storage.proto",
                "../protos/consensus.proto",
                "../protos/overlay.proto",
                "../protos/validation.proto",
                "../protos/mining.proto",
                "../protos/auth.proto",
                "../protos/alert.proto",
                "../protos/index.proto",
                "../protos/metrics.proto",
            ],
            &["../protos", "../protos/google/api", "../protos/google/protobuf"],
        )?;
    println!("cargo:warning=Proto files generated in src");
    Ok(())
}
