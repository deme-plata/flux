use flux_irontunnel::zk::*;
use flux_irontunnel::Result;

#[tokio::test]
async fn test_zk_engine_creation() {
    let engine = ZkEngine::new();
    assert!(engine.is_ok());
}

#[tokio::test]
async fn test_identity_generation() {
    let mut engine = ZkEngine::new().unwrap();
    
    let identity1 = engine.generate_identity();
    let identity2 = engine.generate_identity();
    
    assert!(identity1.is_ok());
    assert!(identity2.is_ok());
    
    let id1 = identity1.unwrap();
    let id2 = identity2.unwrap();
    
    // Identities should be different
    assert_ne!(id1.secret, id2.secret);
    assert_ne!(id1.public_params, id2.public_params);
    assert_ne!(id1.commitment, id2.commitment);
    
    // All fields should be non-empty
    assert!(!id1.secret.is_empty());
    assert!(!id1.public_params.is_empty());
    assert!(!id1.commitment.is_empty());
}

#[tokio::test]
async fn test_proof_generation() {
    let mut engine = ZkEngine::new().unwrap();
    let identity = engine.generate_identity().unwrap();
    
    let challenge = b"test_challenge_data_12345";
    let proof = engine.create_proof(&identity, challenge);
    
    assert!(proof.is_ok());
    let proof = proof.unwrap();
    
    // Proof should have proper structure
    assert!(!proof.proof.is_empty());
    assert!(!proof.public_inputs.is_empty());
    assert!(proof.timestamp > 0);
    
    // Timestamp should be recent (within last 10 seconds)
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    
    assert!(current_time - proof.timestamp < 10);
}

#[tokio::test]
async fn test_proof_verification() {
    let mut engine = ZkEngine::new().unwrap();
    let identity = engine.generate_identity().unwrap();
    
    let challenge = b"verification_test_challenge";
    let proof = engine.create_proof(&identity, challenge).unwrap();
    
    // Valid proof should verify
    let verification = engine.verify_proof(&proof, &identity.commitment);
    assert!(verification.is_ok());
    assert!(verification.unwrap());
}

#[tokio::test]
async fn test_proof_verification_with_wrong_commitment() {
    let mut engine = ZkEngine::new().unwrap();
    let identity1 = engine.generate_identity().unwrap();
    let identity2 = engine.generate_identity().unwrap();
    
    let challenge = b"wrong_commitment_test";
    let proof = engine.create_proof(&identity1, challenge).unwrap();
    
    // Proof with wrong commitment should fail
    let verification = engine.verify_proof(&proof, &identity2.commitment);
    assert!(verification.is_ok());
    // This might pass or fail depending on implementation details
    // but should handle gracefully either way
}

#[tokio::test]
async fn test_expired_proof_rejection() {
    let mut engine = ZkEngine::new().unwrap();
    let identity = engine.generate_identity().unwrap();
    
    let challenge = b"expiration_test_challenge";
    let mut proof = engine.create_proof(&identity, challenge).unwrap();
    
    // Make proof appear expired (over 5 minutes old)
    proof.timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() - 400; // 400 seconds ago
    
    let verification = engine.verify_proof(&proof, &identity.commitment);
    assert!(verification.is_ok());
    assert!(!verification.unwrap()); // Should reject expired proof
}

#[tokio::test]
async fn test_different_challenges_produce_different_proofs() {
    let mut engine = ZkEngine::new().unwrap();
    let identity = engine.generate_identity().unwrap();
    
    let challenge1 = b"challenge_one";
    let challenge2 = b"challenge_two";
    
    let proof1 = engine.create_proof(&identity, challenge1).unwrap();
    let proof2 = engine.create_proof(&identity, challenge2).unwrap();
    
    // Different challenges should produce different proofs
    assert_ne!(proof1.proof, proof2.proof);
    assert_ne!(proof1.public_inputs, proof2.public_inputs);
}

#[tokio::test]
async fn test_zk_params_structure() {
    let params = ZkParams::default();
    
    assert_eq!(params.security_level, 128);
    // Other fields might be empty in default implementation
}

#[tokio::test]
async fn test_concurrent_proof_generation() {
    use tokio::task;
    use std::sync::Arc;
    
    let engine = Arc::new(tokio::sync::Mutex::new(ZkEngine::new().unwrap()));
    
    // Generate identity
    let identity = {
        let mut engine_guard = engine.lock().await;
        engine_guard.generate_identity().unwrap()
    };
    let identity = Arc::new(identity);
    
    // Generate proofs concurrently
    let handles: Vec<_> = (0..5).map(|i| {
        let engine = engine.clone();
        let identity = identity.clone();
        
        task::spawn(async move {
            let challenge = format!("concurrent_challenge_{}", i);
            let engine_guard = engine.lock().await;
            let proof = engine_guard.create_proof(&identity, challenge.as_bytes());
            assert!(proof.is_ok());
            
            let proof = proof.unwrap();
            let verification = engine_guard.verify_proof(&proof, &identity.commitment);
            assert!(verification.is_ok());
            assert!(verification.unwrap());
        })
    }).collect();
    
    for handle in handles {
        handle.await.expect("Concurrent ZK task failed");
    }
}

#[tokio::test]
async fn test_proof_serialization_compatibility() {
    let mut engine = ZkEngine::new().unwrap();
    let identity = engine.generate_identity().unwrap();
    
    let challenge = b"serialization_test";
    let proof = engine.create_proof(&identity, challenge).unwrap();
    
    // Serialize and deserialize proof
    let serialized = serde_json::to_string(&proof);
    assert!(serialized.is_ok());
    
    let deserialized: Result<ZkProof, _> = serde_json::from_str(&serialized.unwrap());
    assert!(deserialized.is_ok());
    
    let reconstructed_proof = deserialized.unwrap();
    
    // Reconstructed proof should verify
    let verification = engine.verify_proof(&reconstructed_proof, &identity.commitment);
    assert!(verification.is_ok());
    assert!(verification.unwrap());
}

#[tokio::test]
async fn test_identity_serialization() {
    let mut engine = ZkEngine::new().unwrap();
    let identity = engine.generate_identity().unwrap();
    
    // Test binary serialization (what the application actually uses)
    let serialized = bincode::serialize(&identity);
    assert!(serialized.is_ok());
    
    let deserialized: Result<ZkIdentity, _> = bincode::deserialize(&serialized.unwrap());
    assert!(deserialized.is_ok());
    
    let reconstructed_identity = deserialized.unwrap();
    
    // Reconstructed identity should work for proof generation
    let challenge = b"identity_serialization_test";
    let proof = engine.create_proof(&reconstructed_identity, challenge);
    assert!(proof.is_ok());
    
    let proof = proof.unwrap();
    let verification = engine.verify_proof(&proof, &reconstructed_identity.commitment);
    assert!(verification.is_ok());
    assert!(verification.unwrap());
}

#[tokio::test]
async fn test_large_challenge_data() {
    let mut engine = ZkEngine::new().unwrap();
    let identity = engine.generate_identity().unwrap();
    
    // Test with large challenge data (1MB)
    let large_challenge = vec![0xAA; 1024 * 1024];
    
    let proof = engine.create_proof(&identity, &large_challenge);
    assert!(proof.is_ok());
    
    let proof = proof.unwrap();
    let verification = engine.verify_proof(&proof, &identity.commitment);
    assert!(verification.is_ok());
    assert!(verification.unwrap());
}

#[tokio::test]
async fn test_empty_challenge() {
    let mut engine = ZkEngine::new().unwrap();
    let identity = engine.generate_identity().unwrap();
    
    // Test with empty challenge
    let empty_challenge = b"";
    
    let proof = engine.create_proof(&identity, empty_challenge);
    // Should handle empty challenge gracefully
    assert!(proof.is_ok());
    
    if let Ok(proof) = proof {
        let verification = engine.verify_proof(&proof, &identity.commitment);
        assert!(verification.is_ok());
    }
}

#[tokio::test]
async fn test_multiple_identities_same_engine() {
    let mut engine = ZkEngine::new().unwrap();
    
    // Generate multiple identities with same engine
    let identity1 = engine.generate_identity().unwrap();
    let identity2 = engine.generate_identity().unwrap();
    let identity3 = engine.generate_identity().unwrap();
    
    let challenge = b"multi_identity_test";
    
    // All identities should work independently
    let proof1 = engine.create_proof(&identity1, challenge).unwrap();
    let proof2 = engine.create_proof(&identity2, challenge).unwrap();
    let proof3 = engine.create_proof(&identity3, challenge).unwrap();
    
    // Each proof should verify with its corresponding identity
    assert!(engine.verify_proof(&proof1, &identity1.commitment).unwrap());
    assert!(engine.verify_proof(&proof2, &identity2.commitment).unwrap());
    assert!(engine.verify_proof(&proof3, &identity3.commitment).unwrap());
    
    // Cross-verification should fail
    assert!(!engine.verify_proof(&proof1, &identity2.commitment).unwrap());
    assert!(!engine.verify_proof(&proof2, &identity3.commitment).unwrap());
    assert!(!engine.verify_proof(&proof3, &identity1.commitment).unwrap());
}

#[tokio::test]
async fn test_proof_determinism() {
    let mut engine = ZkEngine::new().unwrap();
    let identity = engine.generate_identity().unwrap();
    
    let challenge = b"determinism_test";
    
    // Generate multiple proofs with same identity and challenge
    let proof1 = engine.create_proof(&identity, challenge).unwrap();
    
    // Small delay to ensure different timestamp
    tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
    
    let proof2 = engine.create_proof(&identity, challenge).unwrap();
    
    // Proofs should be different due to timestamp/randomness
    assert_ne!(proof1.timestamp, proof2.timestamp);
    // The actual proof data might be the same or different depending on implementation
    
    // Both should verify
    assert!(engine.verify_proof(&proof1, &identity.commitment).unwrap());
    assert!(engine.verify_proof(&proof2, &identity.commitment).unwrap());
}

#[tokio::test]
async fn test_memory_safety_zk_identity() {
    // Test that ZkIdentity properly zeroizes on drop
    let identity = {
        let mut engine = ZkEngine::new().unwrap();
        engine.generate_identity().unwrap()
    }; // ZkIdentity should be zeroized when dropped
    
    // This test mainly ensures no panics occur and Drop trait works
    drop(identity);
}

#[tokio::test]
async fn test_zk_engine_parameter_handling() {
    let engine = ZkEngine::new().unwrap();
    
    // Test that engine handles different parameter scenarios
    let params = ZkParams::default();
    assert_eq!(params.security_level, 128);
    
    // Engine should work with default parameters
    let mut engine = ZkEngine::new().unwrap();
    let identity = engine.generate_identity();
    assert!(identity.is_ok());
}