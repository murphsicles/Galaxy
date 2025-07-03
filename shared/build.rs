fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .compile(
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
