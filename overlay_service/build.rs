fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .compile_protos(&["../protos/overlay.proto", "../protos/metrics.proto"], &["../protos"])?;
    Ok(())
}
