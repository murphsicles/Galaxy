fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("protos/network.proto")?;
    tonic_build::compile_protos("protos/transaction.proto")?;
    tonic_build::compile_protos("protos/block.proto")?;
    tonic_build::compile_protos("protos/storage.proto")?;
    tonic_build::compile_protos("protos/consensus.proto")?;
    tonic_build::compile_protos("protos/api.proto")?;
    tonic_build::compile_protos("protos/mining.proto")?;
    Ok(())
}
