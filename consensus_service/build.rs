fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .compile_protos(&["../protos/consensus.proto", "../protos/metrics.proto"], &["../protos"])?;
    Ok(())
}
