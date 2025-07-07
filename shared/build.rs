// shared/build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_files = [
        "src/network.rs",
        "src/transaction.rs",
        "src/block.rs",
        "src/storage.rs",
        "src/consensus.rs",
        "src/overlay.rs",
        "src/validation.rs",
        "src/mining.rs",
        "src/auth.rs",
        "src/alert.rs",
        "src/index.rs",
        "src/metrics.rs",
    ];
    if std::env::var("CI").is_ok() && proto_files.iter().all(|f| std::path::Path::new(f).exists()) {
        println!("cargo:warning=Pre-compiled proto files found, skipping compilation");
        return Ok(());
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
