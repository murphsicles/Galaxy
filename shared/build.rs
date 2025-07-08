use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_files = [
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
    ];
    let out_dir = Path::new("src");
    if !out_dir.exists() {
        std::fs::create_dir_all(out_dir)?;
        println!("cargo:warning=Created src directory");
    }
    if std::env::var("CI").is_ok() && proto_files.iter().all(|f| {
        let generated = f.replace("../protos/", "src/").replace(".proto", ".rs");
        Path::new(&generated).exists()
    }) {
        println!("cargo:warning=Pre-compiled proto files found, skipping compilation");
        return Ok(());
    }
    let protoc_path = std::env::var("PROTOC").unwrap_or_else(|_| "protoc".to_string());
    println!("cargo:warning=Using protoc: {}", protoc_path);
    if !Path::new(&protoc_path).exists() {
        eprintln!("protoc binary not found at: {}", protoc_path);
        std::process::exit(1);
    }
    tonic_build::configure()
        .build_server(true)
        .out_dir("src")
        .compile_protos(
            &proto_files,
            &["../protos", "../protos/google/api", "../protos/google/protobuf"],
        )?;
    println!("cargo:warning=Proto files generated in src");
    Ok(())
}
