// tests/spv_validation.rs
use galaxy::validation::validation_client::ValidationClient;
use galaxy::validation::{GenerateSPVProofRequest, VerifySPVProofRequest};
use tonic::transport::Channel;

#[tokio::test]
async fn test_spv_proof_generation_and_verification() {
    // Connect to validation_service
    let channel = Channel::from_static("http://[::1]:50057")
        .connect()
        .await
        .expect("Failed to connect to validation_service");
    let mut client = ValidationClient::new(channel);

    // Test data: a sample transaction hex (simplified for testing)
    let tx_hex = "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff04deadbeef0100000001000000000000000000000000";

    // Generate SPV proof
    let generate_request = tonic::Request::new(GenerateSPVProofRequest {
        txid: tx_hex.to_string(),
    });
    let generate_response = client
        .generate_spv_proof(generate_request)
        .await
        .expect("Failed to generate SPV proof")
        .into_inner();

    assert!(
        generate_response.success,
        "SPV proof generation failed: {}",
        generate_response.error
    );

    // Verify SPV proof
    let verify_request = tonic::Request::new(VerifySPVProofRequest {
        txid: tx_hex.to_string(),
        merkle_path: generate_response.merkle_path,
        block_headers: generate_response.block_headers,
    });
    let verify_response = client
        .verify_spv_proof(verify_request)
        .await
        .expect("Failed to verify SPV proof")
        .into_inner();

    assert!(
        verify_response.success,
        "SPV proof verification failed: {}",
        verify_response.error
    );
}
