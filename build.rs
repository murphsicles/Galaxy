// build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Skip proto compilation in CI, assuming pre-generated files are in src
    if std::env::var("CI").is_ok() {
        println!("Skipping proto compilation in CI environment");
        return Ok(());
    }

    tonic_build::configure()
        .build_server(true)
        .out_dir("src")
        .compile_protos(
            &[
                "protos/network.proto",
                "protos/transaction.proto",
                "protos/block.proto",
                "protos/storage.proto",
                "protos/consensus.proto",
                "protos/overlay.proto",
                "protos/validation.proto",
                "protos/mining.proto",
                "protos/auth.proto",
                "protos/alert.proto",
                "protos/index.proto",
                "protos/metrics.proto",
            ],
            &["protos", "protos/google/api", "protos/google/protobuf"],
        )?;
    Ok(())
}
