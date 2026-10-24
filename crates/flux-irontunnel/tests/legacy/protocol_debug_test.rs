use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::time::Duration;

#[tokio::test]
async fn test_server_protocol_detailed() {
    println!("🔍 Starting detailed protocol analysis test");
    
    // Connect to the actual server
    let server = "161.35.219.10:1194";
    println!("🔗 Connecting to {}", server);
    
    let mut stream = match TcpStream::connect(server).await {
        Ok(s) => {
            println!("✅ TCP connection established");
            s
        }
        Err(e) => {
            println!("❌ Failed to connect: {}", e);
            return;
        }
    };
    
    // Set a timeout for reads
    stream.set_nodelay(true).unwrap();
    
    // Test 1: Read initial server response
    println!("\n📨 TEST 1: Reading initial server response");
    let mut buffer = [0u8; 1024];
    match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer)).await {
        Ok(Ok(n)) if n > 0 => {
            println!("📊 Received {} bytes", n);
            println!("📝 Raw bytes: {:?}", &buffer[..n]);
            println!("📝 As string: {:?}", String::from_utf8_lossy(&buffer[..n]));
            println!("📝 Hex dump: {}", hex_dump(&buffer[..n]));
        }
        Ok(Ok(0)) => {
            println!("❌ Server closed connection immediately");
            return;
        }
        Ok(Err(e)) => {
            println!("❌ Read error: {}", e);
            return;
        }
        Err(_) => {
            println!("❌ Timeout reading initial response");
            return;
        }
    }
    
    // Test 2: Send initial ZK proof response
    println!("\n📤 TEST 2: Sending initial ZK proof response");
    let initial_proof = b"mock_zk_proof_response\n";
    println!("📝 Sending {} bytes: {:?}", initial_proof.len(), initial_proof);
    println!("📝 As string: {:?}", String::from_utf8_lossy(initial_proof));
    
    if let Err(e) = stream.write_all(initial_proof).await {
        println!("❌ Failed to send initial proof: {}", e);
        return;
    }
    println!("✅ Sent initial ZK proof");
    
    // Test 3: Read auth challenge response
    println!("\n📨 TEST 3: Reading auth challenge response");
    let mut auth_buffer = [0u8; 1024];
    match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut auth_buffer)).await {
        Ok(Ok(n)) if n > 0 => {
            println!("📊 Received {} bytes", n);
            println!("📝 Raw bytes: {:?}", &auth_buffer[..n]);
            println!("📝 As string: {:?}", String::from_utf8_lossy(&auth_buffer[..n]));
            println!("📝 Hex dump: {}", hex_dump(&auth_buffer[..n]));
            
            let auth_response = String::from_utf8_lossy(&auth_buffer[..n]);
            
            // Analyze the response
            if auth_response.contains("ZK_AUTH_REQUIRED") {
                println!("✅ Found ZK_AUTH_REQUIRED");
                
                if auth_response.contains("irontunnel_zk_challenge") {
                    println!("✅ Found challenge in response");
                    
                    // Extract challenge
                    if let Some(challenge_start) = auth_response.find("irontunnel_zk_challenge") {
                        let challenge_part = &auth_response[challenge_start..];
                        let challenge = challenge_part.trim();
                        println!("🔑 Extracted challenge: '{}'", challenge);
                        
                        // Test 4: Send ZK proof for challenge
                        println!("\n📤 TEST 4: Sending ZK proof for challenge");
                        let zk_proof = "mock_zk_proof_response\n";
                        println!("📝 Sending {} bytes: {:?}", zk_proof.len(), zk_proof.as_bytes());
                        println!("📝 As string: {:?}", zk_proof);
                        
                        if let Err(e) = stream.write_all(zk_proof.as_bytes()).await {
                            println!("❌ Failed to send ZK proof: {}", e);
                            return;
                        }
                        println!("✅ Sent ZK proof");
                        
                        // Test 5: Read verification response
                        println!("\n📨 TEST 5: Reading ZK verification response");
                        let mut verify_buffer = [0u8; 1024];
                        match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut verify_buffer)).await {
                            Ok(Ok(n)) if n > 0 => {
                                println!("📊 Received {} bytes", n);
                                println!("📝 Raw bytes: {:?}", &verify_buffer[..n]);
                                println!("📝 As string: {:?}", String::from_utf8_lossy(&verify_buffer[..n]));
                                println!("📝 Hex dump: {}", hex_dump(&verify_buffer[..n]));
                                
                                let verify_response = String::from_utf8_lossy(&verify_buffer[..n]);
                                if verify_response.contains("ZK_AUTH_SUCCESS") {
                                    println!("✅ ZK authentication successful!");
                                    
                                    // Test 6: Send CLIENT_AUTH
                                    println!("\n📤 TEST 6: Sending CLIENT_AUTH");
                                    let client_auth = format!("CLIENT_AUTH:client-test-{}\n", std::process::id());
                                    println!("📝 Sending {} bytes: {:?}", client_auth.len(), client_auth.as_bytes());
                                    println!("📝 As string: {:?}", client_auth);
                                    
                                    if let Err(e) = stream.write_all(client_auth.as_bytes()).await {
                                        println!("❌ Failed to send CLIENT_AUTH: {}", e);
                                        return;
                                    }
                                    println!("✅ Sent CLIENT_AUTH");
                                    
                                    // Test 7: Read final ACK
                                    println!("\n📨 TEST 7: Reading final AUTH_ACK");
                                    let mut ack_buffer = [0u8; 1024];
                                    match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut ack_buffer)).await {
                                        Ok(Ok(n)) if n > 0 => {
                                            println!("📊 Received {} bytes", n);
                                            println!("📝 Raw bytes: {:?}", &ack_buffer[..n]);
                                            println!("📝 As string: {:?}", String::from_utf8_lossy(&ack_buffer[..n]));
                                            println!("📝 Hex dump: {}", hex_dump(&ack_buffer[..n]));
                                            
                                            let ack_response = String::from_utf8_lossy(&ack_buffer[..n]);
                                            if ack_response.contains("AUTH_ACK") {
                                                println!("🎉 FULL AUTHENTICATION SUCCESSFUL!");
                                            } else {
                                                println!("❌ Unexpected final response");
                                            }
                                        }
                                        Ok(Ok(0)) => {
                                            println!("❌ Server closed connection after CLIENT_AUTH");
                                        }
                                        Ok(Err(e)) => {
                                            println!("❌ Read error on final ACK: {}", e);
                                        }
                                        Err(_) => {
                                            println!("❌ Timeout reading final ACK");
                                        }
                                    }
                                } else {
                                    println!("❌ ZK authentication failed or unexpected response");
                                }
                            }
                            Ok(Ok(0)) => {
                                println!("❌ Server closed connection after ZK proof");
                            }
                            Ok(Err(e)) => {
                                println!("❌ Read error on verification: {}", e);
                            }
                            Err(_) => {
                                println!("❌ Timeout reading verification response");
                            }
                        }
                    } else {
                        println!("❌ Could not find challenge in response");
                    }
                } else {
                    println!("❌ No challenge found in auth response");
                }
            } else {
                println!("❌ No ZK_AUTH_REQUIRED found in response");
            }
        }
        Ok(Ok(0)) => {
            println!("❌ Server closed connection after initial proof");
        }
        Ok(Err(e)) => {
            println!("❌ Read error on auth response: {}", e);
        }
        Err(_) => {
            println!("❌ Timeout reading auth response");
        }
    }
    
    println!("\n🏁 Protocol analysis test completed");
}

fn hex_dump(data: &[u8]) -> String {
    data.iter()
        .enumerate()
        .map(|(i, b)| {
            if i % 16 == 0 && i > 0 {
                format!("\n{:04x}: {:02x}", i, b)
            } else if i % 16 == 0 {
                format!("{:04x}: {:02x}", i, b)
            } else {
                format!(" {:02x}", b)
            }
        })
        .collect::<String>()
}

// Test the framed protocol variant as well
#[tokio::test]
async fn test_framed_protocol_attempt() {
    println!("🔍 Testing framed protocol variant");
    
    let server = "161.35.219.10:1194";
    let mut stream = match TcpStream::connect(server).await {
        Ok(s) => s,
        Err(e) => {
            println!("❌ Failed to connect: {}", e);
            return;
        }
    };
    
    // Read initial response
    let mut buffer = [0u8; 1024];
    let n = match stream.read(&mut buffer).await {
        Ok(n) => n,
        Err(e) => {
            println!("❌ Failed to read initial response: {}", e);
            return;
        }
    };
    
    println!("📨 Initial response: {:?}", String::from_utf8_lossy(&buffer[..n]));
    
    // Send initial proof
    let _ = stream.write_all(b"mock_zk_proof_response\n").await;
    
    // Read auth response
    let mut auth_buffer = [0u8; 1024];
    let auth_n = match stream.read(&mut auth_buffer).await {
        Ok(n) => n,
        Err(e) => {
            println!("❌ Failed to read auth response: {}", e);
            return;
        }
    };
    
    println!("📨 Auth response: {:?}", String::from_utf8_lossy(&auth_buffer[..auth_n]));
    
    // Try sending framed ZK proof
    println!("📤 Trying framed ZK proof");
    let zk_proof = b"mock_zk_proof_response";
    let length = zk_proof.len() as u32;
    
    // Send length prefix
    if let Err(e) = stream.write_all(&length.to_be_bytes()).await {
        println!("❌ Failed to send length: {}", e);
        return;
    }
    
    // Send proof data
    if let Err(e) = stream.write_all(zk_proof).await {
        println!("❌ Failed to send proof: {}", e);
        return;
    }
    
    println!("✅ Sent framed proof: {} bytes", zk_proof.len());
    
    // Try to read response
    let mut verify_buffer = [0u8; 1024];
    match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut verify_buffer)).await {
        Ok(Ok(n)) if n > 0 => {
            println!("📨 Framed verification response: {:?}", String::from_utf8_lossy(&verify_buffer[..n]));
            println!("📝 Raw bytes: {:?}", &verify_buffer[..n]);
        }
        Ok(Ok(0)) => {
            println!("❌ Server closed connection after framed proof");
        }
        Ok(Err(e)) => {
            println!("❌ Read error: {}", e);
        }
        Err(_) => {
            println!("❌ Timeout - no response to framed proof");
        }
    }
}