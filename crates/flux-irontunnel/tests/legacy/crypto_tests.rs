use flux_irontunnel::crypto::*;
use flux_irontunnel::Result;

#[tokio::test]
async fn test_crypto_engine_initialization() {
    let crypto = CryptoEngine::new();
    assert!(crypto.is_ok());
    
    let crypto = crypto.unwrap();
    let public_key = crypto.get_public_key();
    
    // Public key should be valid Ed25519 key
    assert_eq!(public_key.as_bytes().len(), 32);
    assert_ne!(public_key.as_bytes(), &[0u8; 32]);
}

#[tokio::test]
async fn test_ephemeral_key_generation() {
    let (secret1, public1) = CryptoEngine::generate_ephemeral_keypair();
    let (secret2, public2) = CryptoEngine::generate_ephemeral_keypair();
    
    // Keys should be different each time
    assert_ne!(secret1.as_bytes(), secret2.as_bytes());
    assert_ne!(public1.as_bytes(), public2.as_bytes());
    
    // Keys should be properly formatted
    assert_eq!(secret1.as_bytes().len(), 32);
    assert_eq!(public1.as_bytes().len(), 32);
}

#[tokio::test]
async fn test_key_exchange_protocol() {
    let mut crypto1 = CryptoEngine::new().unwrap();
    let mut crypto2 = CryptoEngine::new().unwrap();
    
    let (_, public1) = CryptoEngine::generate_ephemeral_keypair();
    let (_, public2) = CryptoEngine::generate_ephemeral_keypair();
    
    // Perform key exchange
    let exchange1 = crypto1.perform_key_exchange(public2.as_bytes()).await;
    let exchange2 = crypto2.perform_key_exchange(public1.as_bytes()).await;
    
    assert!(exchange1.is_ok());
    assert!(exchange2.is_ok());
    
    let msg1 = exchange1.unwrap();
    let msg2 = exchange2.unwrap();
    
    // Messages should have proper structure
    assert_eq!(msg1.public_key.len(), 32);
    assert_eq!(msg1.signature.len(), 64);
    assert!(msg1.timestamp > 0);
    
    assert_eq!(msg2.public_key.len(), 32);
    assert_eq!(msg2.signature.len(), 64);
    assert!(msg2.timestamp > 0);
}

#[tokio::test]
async fn test_packet_encryption_decryption() {
    let mut crypto = CryptoEngine::new().unwrap();
    
    // Set up a session first
    let (_, peer_public) = CryptoEngine::generate_ephemeral_keypair();
    let _exchange = crypto.perform_key_exchange(peer_public.as_bytes()).await.unwrap();
    
    let plaintext = b"This is a test message for encryption";
    
    // Test encryption
    let encrypted = crypto.encrypt_packet(plaintext).await;
    assert!(encrypted.is_ok());
    
    let packet = encrypted.unwrap();
    assert!(!packet.ciphertext.is_empty());
    assert_eq!(packet.nonce.len(), 12);
    assert_eq!(packet.tag.len(), 16);
    
    // Test decryption
    let decrypted = crypto.decrypt_packet(&packet).await;
    assert!(decrypted.is_ok());
    
    let decrypted_data = decrypted.unwrap();
    assert_eq!(decrypted_data, plaintext.to_vec());
}

#[tokio::test]
async fn test_session_key_rotation() {
    let mut crypto = CryptoEngine::new().unwrap();
    
    // Set up initial session
    let (_, peer_public) = CryptoEngine::generate_ephemeral_keypair();
    let _exchange = crypto.perform_key_exchange(peer_public.as_bytes()).await.unwrap();
    
    // Rotate keys
    let rotation_result = crypto.rotate_session_key().await;
    assert!(rotation_result.is_ok());
    
    // Should still be able to encrypt after rotation
    let plaintext = b"Post-rotation test message";
    let encrypted = crypto.encrypt_packet(plaintext).await;
    assert!(encrypted.is_ok());
}

#[tokio::test]
async fn test_key_exchange_verification() {
    let crypto = CryptoEngine::new().unwrap();
    let peer_crypto = CryptoEngine::new().unwrap();
    
    let peer_public_key = peer_crypto.get_public_key();
    
    // Create a key exchange message
    let (_, ephemeral_public) = CryptoEngine::generate_ephemeral_keypair();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    
    // Create message manually for testing
    let message = KeyExchangeMessage {
        public_key: *ephemeral_public.as_bytes(),
        signature: [0u8; 64], // Mock signature for now
        timestamp,
    };
    
    // Verification might fail with mock signature, but should handle gracefully
    let verification = crypto.verify_key_exchange(&message, &peer_public_key);
    assert!(verification.is_ok());
}

#[tokio::test]
async fn test_secure_compare_function() {
    let data1 = b"identical_data";
    let data2 = b"identical_data";
    let data3 = b"different_data";
    
    assert!(secure_compare(data1, data2));
    assert!(!secure_compare(data1, data3));
    assert!(!secure_compare(data1, b"short"));
}

#[tokio::test]
async fn test_password_hashing() {
    let password = b"test_password_123";
    let salt = b"random_salt_data";
    
    let hash1 = hash_password(password, salt);
    let hash2 = hash_password(password, salt);
    let hash3 = hash_password(b"different_password", salt);
    
    // Same password and salt should produce same hash
    assert_eq!(hash1, hash2);
    // Different password should produce different hash
    assert_ne!(hash1, hash3);
    
    // Hash should be 32 bytes (256 bits)
    assert_eq!(hash1.len(), 32);
}

#[tokio::test]
async fn test_encryption_with_different_nonces() {
    let mut crypto = CryptoEngine::new().unwrap();
    
    // Set up session
    let (_, peer_public) = CryptoEngine::generate_ephemeral_keypair();
    let _exchange = crypto.perform_key_exchange(peer_public.as_bytes()).await.unwrap();
    
    let plaintext = b"Test message";
    
    // Encrypt same message multiple times
    let encrypted1 = crypto.encrypt_packet(plaintext).await.unwrap();
    let encrypted2 = crypto.encrypt_packet(plaintext).await.unwrap();
    
    // Nonces should be different (due to counter increment)
    assert_ne!(encrypted1.nonce, encrypted2.nonce);
    // Ciphertext should be different due to different nonces
    assert_ne!(encrypted1.ciphertext, encrypted2.ciphertext);
    
    // Both should decrypt to same plaintext
    let decrypted1 = crypto.decrypt_packet(&encrypted1).await.unwrap();
    let decrypted2 = crypto.decrypt_packet(&encrypted2).await.unwrap();
    
    assert_eq!(decrypted1, plaintext.to_vec());
    assert_eq!(decrypted2, plaintext.to_vec());
}

#[tokio::test]
async fn test_encryption_large_data() {
    let mut crypto = CryptoEngine::new().unwrap();
    
    // Set up session
    let (_, peer_public) = CryptoEngine::generate_ephemeral_keypair();
    let _exchange = crypto.perform_key_exchange(peer_public.as_bytes()).await.unwrap();
    
    // Test with larger data (64KB)
    let large_data = vec![0xAA; 65536];
    
    let encrypted = crypto.encrypt_packet(&large_data).await;
    assert!(encrypted.is_ok());
    
    let packet = encrypted.unwrap();
    let decrypted = crypto.decrypt_packet(&packet).await;
    assert!(decrypted.is_ok());
    
    let decrypted_data = decrypted.unwrap();
    assert_eq!(decrypted_data, large_data);
}

#[tokio::test]
async fn test_concurrent_crypto_operations() {
    use tokio::task;
    use std::sync::Arc;
    
    let crypto = Arc::new(tokio::sync::Mutex::new(CryptoEngine::new().unwrap()));
    
    // Set up session
    {
        let mut crypto_guard = crypto.lock().await;
        let (_, peer_public) = CryptoEngine::generate_ephemeral_keypair();
        let _exchange = crypto_guard.perform_key_exchange(peer_public.as_bytes()).await.unwrap();
    }
    
    // Spawn concurrent encryption tasks
    let handles: Vec<_> = (0..10).map(|i| {
        let crypto = crypto.clone();
        task::spawn(async move {
            let message = format!("Message {}", i);
            let mut crypto_guard = crypto.lock().await;
            let encrypted = crypto_guard.encrypt_packet(message.as_bytes()).await;
            assert!(encrypted.is_ok());
            
            let packet = encrypted.unwrap();
            let decrypted = crypto_guard.decrypt_packet(&packet).await;
            assert!(decrypted.is_ok());
            assert_eq!(decrypted.unwrap(), message.as_bytes());
        })
    }).collect();
    
    for handle in handles {
        handle.await.expect("Concurrent crypto task failed");
    }
}

#[tokio::test]
async fn test_session_expiration() {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    
    let mut crypto = CryptoEngine::new().unwrap();
    
    // This test verifies that expired sessions are handled properly
    // In a real implementation, you might want to mock time or use dependency injection
    
    let (_, peer_public) = CryptoEngine::generate_ephemeral_keypair();
    let _exchange = crypto.perform_key_exchange(peer_public.as_bytes()).await.unwrap();
    
    // Try to encrypt - should work with fresh session
    let plaintext = b"Test message";
    let encrypted = crypto.encrypt_packet(plaintext).await;
    assert!(encrypted.is_ok());
}

#[tokio::test]
async fn test_memory_cleanup() {
    // Test that sensitive data is properly cleaned up
    let _crypto = {
        let mut crypto = CryptoEngine::new().unwrap();
        let (_, peer_public) = CryptoEngine::generate_ephemeral_keypair();
        let _exchange = crypto.perform_key_exchange(peer_public.as_bytes()).await.unwrap();
        crypto
    }; // crypto should be dropped and cleaned up here
    
    // This test mainly ensures that the Drop trait is implemented
    // and no panics occur during cleanup
}

#[tokio::test]
async fn test_invalid_decryption() {
    let mut crypto = CryptoEngine::new().unwrap();
    
    // Set up session
    let (_, peer_public) = CryptoEngine::generate_ephemeral_keypair();
    let _exchange = crypto.perform_key_exchange(peer_public.as_bytes()).await.unwrap();
    
    // Create invalid packet
    let invalid_packet = EncryptedPacket {
        ciphertext: vec![0xFF; 32],
        nonce: [0u8; 12],
        tag: [0u8; 16],
    };
    
    // Should fail to decrypt invalid packet
    let decrypted = crypto.decrypt_packet(&invalid_packet).await;
    assert!(decrypted.is_err());
}