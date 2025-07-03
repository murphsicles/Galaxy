fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos(
        tonic_build::configure()
            .build_server(false),
        &[
            "../protos/block.proto",
            "../protos/consensus.proto",
            "../protos/auth.proto",
            "../protos/alert.proto",
            "../protos/metrics.proto",
        ],
        &["../protos"],
    )?;
    Ok(())
}
