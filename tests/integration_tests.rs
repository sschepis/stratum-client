use std::error::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::Duration,
};
use serde_json::json;

// Import from our crate
use rust_stratum::stratum::{
    create_client,
    types::{Share, StratumVersion},
};

async fn setup_test_server(difficulty: f64) -> (TcpListener, String, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    println!("Test server listening on port {}", addr.port());
    (listener, addr.ip().to_string(), addr.port())
}

#[tokio::test]
async fn test_full_mining_cycle() -> Result<(), Box<dyn Error>> {
    let difficulty = 2.0;
    let (listener, host, port) = setup_test_server(difficulty).await;

    // Spawn test server and keep handle
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (read_half, write_half) = socket.into_split();
        let mut reader = BufReader::new(read_half);
        let mut writer = write_half;
        let mut buf = String::new();
        
        // Handle subscribe request
        reader.read_line(&mut buf).await.unwrap();
        
        let subscribe_response = json!({
            "id": 1,
            "result": [
                [
                    ["mining.set_difficulty", "1"],
                    ["mining.notify", "1"]
                ],
                "extranonce1",
                4
            ],
            "error": null
        });
        writer.write_all(format!("{}\n", subscribe_response).as_bytes()).await.unwrap();

        // Handle authorize request
        buf.clear();
        reader.read_line(&mut buf).await.unwrap();
        let auth_response = json!({
            "id": 2,
            "result": true,
            "error": null
        });
        writer.write_all(format!("{}\n", auth_response).as_bytes()).await.unwrap();

        // Send difficulty notification
        let difficulty_notify = json!({
            "method": "mining.set_difficulty",
            "params": [difficulty]
        });
        writer.write_all(format!("{}\n", difficulty_notify).as_bytes()).await.unwrap();

        // Send job notification
        let job = json!({
            "method": "mining.notify",
            "params": [
                "job1",
                "00000000deadbeef00000000deadbeef00000000deadbeef00000000deadbeef",
                "01000000",
                "02000000",
                ["1234567890abcdef"],
                "00000001",
                "1d00ffff",
                "60509af9",
                true
            ]
        });
        writer.write_all(format!("{}\n", job).as_bytes()).await.unwrap();

        // Handle share submission
        buf.clear();
        reader.read_line(&mut buf).await.unwrap();
        let submit_response = json!({
            "id": 3,
            "result": true,
            "error": null
        });
        writer.write_all(format!("{}\n", submit_response).as_bytes()).await.unwrap();

        // Keep server alive
        tokio::time::sleep(Duration::from_secs(1)).await;
    });

    // Create client and connect
    let mut client = create_client(
        StratumVersion::V1,
        host.clone(),
        port,
    ).await?;

    // Subscribe
    let subscription = client.subscribe().await?;
    assert!(!subscription.subscription_id.is_empty());
    assert!(!subscription.extranonce1.is_empty());
    assert!(subscription.extranonce2_size > 0);

    // Authorize
    let auth = client.authorize("test.worker1", "x").await?;
    assert!(auth.authorized);

    // Wait for job and target
    let mut job_received = false;
    let mut target_received = false;
    let start = std::time::Instant::now();

    while start.elapsed() < Duration::from_secs(5) {
        client.handle_notifications().await?;

        if !job_received {
            if let Some(job) = client.get_current_job().await? {
                assert!(!job.job_id.is_empty());
                assert!(!job.prev_hash.is_empty());
                job_received = true;
            }
        }

        if !target_received {
            if let Ok(target) = client.get_target().await {
                assert_eq!(target.difficulty, difficulty);
                target_received = true;
            }
        }

        if job_received && target_received {
            break;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(job_received, "No job received");
    assert!(target_received, "No target received");

    // Submit share
    let job = client.get_current_job().await?.unwrap();
    let share = Share {
        job_id: job.job_id,
        extranonce2: "00000000".to_string(),
        ntime: job.ntime,
        nonce: "00000000".to_string(),
    };

    let accepted = client.submit_share(share).await?;
    assert!(accepted);

    // Wait for server to finish
    server.await?;

    Ok(())
}

#[tokio::test]
async fn test_reconnection() -> Result<(), Box<dyn Error>> {
    let (listener, host, port) = setup_test_server(1.0).await;
    
    // Spawn test server 1 and keep handle
    let server1 = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (read_half, write_half) = socket.into_split();
        let mut reader = BufReader::new(read_half);
        let mut writer = write_half;
        let mut buf = String::new();
        
        // Handle subscribe request
        reader.read_line(&mut buf).await.unwrap();
        
        let subscribe_response = json!({
            "id": 1,
            "result": [
                [
                    ["mining.set_difficulty", "1"],
                    ["mining.notify", "1"]
                ],
                "extranonce1",
                4
            ],
            "error": null
        });
        writer.write_all(format!("{}\n", subscribe_response).as_bytes()).await.unwrap();

        // Handle authorize request
        buf.clear();
        reader.read_line(&mut buf).await.unwrap();
        let auth_response = json!({
            "id": 2,
            "result": true,
            "error": null
        });
        writer.write_all(format!("{}\n", auth_response).as_bytes()).await.unwrap();

        // Send difficulty notification
        let difficulty_notify = json!({
            "method": "mining.set_difficulty",
            "params": [1.0]
        });
        writer.write_all(format!("{}\n", difficulty_notify).as_bytes()).await.unwrap();

        // Keep server alive
        tokio::time::sleep(Duration::from_secs(2)).await;
    });

    // Create and connect client
    let mut client = create_client(
        StratumVersion::V1,
        host.clone(),
        port,
    ).await?;

    // Initial connection works
    client.subscribe().await?;
    client.authorize("test.worker1", "x").await?;

    // Wait for notifications
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        client.handle_notifications().await?;
        if let Ok(target) = client.get_target().await {
            assert_eq!(target.difficulty, 1.0);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Create a channel to signal when server is ready
    let (tx, rx) = tokio::sync::oneshot::channel();

    // Start new server 2 before first one finishes
    let (listener, host, port) = setup_test_server(1.0).await;

    // Store connection info for client
    let new_host = host.clone();
    let new_port = port;

    // Spawn new test server 2 and keep handle
    let server2 = tokio::spawn(async move {
        // Signal that we have server 2 address
        let _ = tx.send((host, port));
        let (mut socket, _) = listener.accept().await.unwrap();
        let (read_half, write_half) = socket.into_split();
        let mut reader = BufReader::new(read_half);
        let mut writer = write_half;
        let mut buf = String::new();
        
        // Handle subscribe request
        reader.read_line(&mut buf).await.unwrap();
        
        let subscribe_response = json!({
            "id": 3,
            "result": [
                [
                    ["mining.set_difficulty", "1"],
                    ["mining.notify", "1"]
                ],
                "extranonce1",
                4
            ],
            "error": null
        });
        writer.write_all(format!("{}\n", subscribe_response).as_bytes()).await.unwrap();

        // Handle authorize request
        buf.clear();
        reader.read_line(&mut buf).await.unwrap();
        let auth_response = json!({
            "id": 4,
            "result": true,
            "error": null
        });
        writer.write_all(format!("{}\n", auth_response).as_bytes()).await.unwrap();

        // Send difficulty notification
        let difficulty_notify = json!({
            "method": "mining.set_difficulty",
            "params": [1.0]
        });
        writer.write_all(format!("{}\n", difficulty_notify).as_bytes()).await.unwrap();

        // Keep server alive
        tokio::time::sleep(Duration::from_secs(1)).await;
    });

    // Wait for first server to finish and get new server info
    server1.await?;
    let (_, _) = rx.await.unwrap();

    // Update client with new server info
    client = create_client(
        StratumVersion::V1,
        new_host,
        new_port,
    ).await?;

    // Verify connection works
    client.subscribe().await?;
    client.authorize("test.worker1", "x").await?;

    // Wait for notifications
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        client.handle_notifications().await?;
        if let Ok(target) = client.get_target().await {
            assert_eq!(target.difficulty, 1.0);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Wait for second server to finish
    server2.await?;

    Ok(())
}

#[tokio::test]
async fn test_error_handling() -> Result<(), Box<dyn Error>> {
    // Try connecting to invalid address
    let result = create_client(
        StratumVersion::V1,
        "invalid".to_string(),
        1234,
    ).await;
    assert!(result.is_err());

    // Try connecting to non-existent server with minimal timeout
    use std::time::Duration;
    use tokio::time::timeout;
    
    let result = timeout(
        Duration::from_millis(100),
        create_client(
            StratumVersion::V1,
            "127.0.0.1".to_string(),
            1234,
        )
    ).await;
    assert!(result.is_err() || result.unwrap().is_err());

    // Connect to valid server
    let (listener, host, port) = setup_test_server(1.0).await;

    // Spawn test server and keep handle
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (read_half, write_half) = socket.into_split();
        let mut reader = BufReader::new(read_half);
        let mut writer = write_half;
        let mut buf = String::new();
        
        // Handle subscribe request
        reader.read_line(&mut buf).await.unwrap();
        println!("Server received subscribe request: {}", buf);

        let error_response = json!({
            "id": 1,
            "result": null,
            "error": ["Invalid request", -1, null]
        });
        writer.write_all(format!("{}\n", error_response).as_bytes()).await.unwrap();

        // Keep server alive
        tokio::time::sleep(Duration::from_secs(1)).await;
    });

    // Try subscribing - should fail due to error response
    let mut client = create_client(
        StratumVersion::V1,
        host,
        port,
    ).await?;

    // Try subscribing - should fail due to error response
    // Add a small delay before subscribing
    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = client.subscribe().await;
    assert!(result.is_err());

    // Wait for server to finish
    server.await?;

    Ok(())
}
