fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .out_dir("src")
        .compile(
            &["../../protos/index.proto", "../../protos/auth.proto", "../../protos/metrics.proto"],
            &["../../protos"],
        )?;
    Ok(())
}
