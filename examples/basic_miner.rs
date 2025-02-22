use rust_stratum::stratum::v1::jobs::TestMiner;
use rust_stratum::stratum::{
    create_client,
    types::{Share, StratumVersion},
};
use std::error::Error;
use tokio::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let host = std::env::var("POOL_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("POOL_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3333);
    let username = std::env::var("POOL_USER").unwrap_or_else(|_| "test.worker1".to_string());
    let password = std::env::var("POOL_PASS").unwrap_or_else(|_| "x".to_string());

    println!("Connecting to stratum+tcp://{}:{}", host, port);

    // Create and connect a client
    let mut client = create_client(StratumVersion::V1, host.clone(), port, TestMiner).await?;

    // Subscribe to the pool
    let subscription = client.subscribe().await?;
    println!("Connected to pool!");
    println!("Subscription ID: {}", subscription.subscription_id);
    println!("Extranonce1: {}", subscription.extranonce1);
    println!("Extranonce2 size: {} bytes", subscription.extranonce2_size);

    // Authorize with pool credentials
    let auth = client.authorize(&username, &password).await?;
    if !auth.authorized {
        return Err("Authorization failed".into());
    }
    println!("Successfully authorized worker!");

    // Main mining loop
    println!("Starting mining loop...");
    let mut shares_accepted = 0u64;
    let mut shares_rejected = 0u64;
    let mut last_job_id = String::new();

    loop {
        // Handle any new notifications (new jobs, difficulty changes)
        if let Err(e) = client.handle_notifications().await {
            eprintln!("Error handling notifications: {}", e);
            // Try to reconnect
            println!("Attempting to reconnect...");
            if let Err(e) = client.reconnect().await {
                eprintln!("Reconnection failed: {}", e);
                return Err("Failed to reconnect to pool".into());
            }
            continue;
        }

        // Get current job
        if let Some(job) = client.get_current_job().await? {
            // Only process new jobs
            if job.job_id != last_job_id {
                last_job_id = job.job_id.clone();

                // Get current target
                if let Ok(target) = client.get_target().await {
                    println!("Mining at difficulty {}", target.difficulty);
                    println!("Job ID: {}", job.job_id);
                    println!("Previous block hash: {}", job.prev_hash);

                    // Generate a random share for testing
                    let share = Share {
                        job_id: job.job_id,
                        extranonce2: format!("{:08x}", rand::random::<u32>()),
                        ntime: job.ntime,
                        nonce: format!("{:08x}", rand::random::<u32>()),
                    };

                    match client.submit_share(share).await {
                        Ok(accepted) => {
                            if accepted {
                                shares_accepted += 1;
                                println!(
                                    "Share accepted! ({} accepted, {} rejected)",
                                    shares_accepted, shares_rejected
                                );
                            } else {
                                shares_rejected += 1;
                                println!(
                                    "Share rejected ({} accepted, {} rejected)",
                                    shares_accepted, shares_rejected
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to submit share: {}", e);
                            shares_rejected += 1;
                        }
                    }
                }
            }
        }

        // Sleep briefly to avoid hammering the pool
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
