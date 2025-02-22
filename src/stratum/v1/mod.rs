mod protocol;
mod connection;
mod jobs;

use crate::stratum::{error::StratumError, types::*, StratumClient, miner::Miner};
use async_trait::async_trait;
use connection::StratumConnection;
use jobs::JobManager;
use protocol::{CLIENT_VERSION, MINING_AUTHORIZE, MINING_NOTIFY, MINING_SET_DIFFICULTY, MINING_SUBSCRIBE, MINING_SUBMIT};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

/// A Stratum V1 protocol client implementation
/// 
/// This client handles all the low-level details of the Stratum V1 protocol including:
/// - Connection management with automatic reconnection
/// - JSON-RPC message formatting and parsing
/// - Subscription and authorization
/// - Job notifications and difficulty updates
/// - Share submission
#[derive(Clone)]
pub struct StratumV1Client {
    connection: Arc<Mutex<StratumConnection>>,
    job_manager: JobManager,
    server_info: Arc<Mutex<Option<ServerInfo>>>,
    result_sender: Arc<Mutex<Option<mpsc::Sender<Result<u32, StratumError>>>>>,
    result_receiver: Arc<Mutex<Option<mpsc::Receiver<Result<u32, StratumError>>>>>,
}

impl StratumV1Client {
    /// Creates a new Stratum V1 client and connects to the specified mining pool
    pub async fn new(host: String, port: u16) -> Result<Self, StratumError> {
        let (sender, receiver) = mpsc::channel(100);
        Ok(Self {
            connection: Arc::new(Mutex::new(StratumConnection::new(host, port).await?)),
            job_manager: JobManager::new(),
            server_info: Arc::new(Mutex::new(None)),
            result_sender: Arc::new(Mutex::new(Some(sender))),
            result_receiver: Arc::new(Mutex::new(Some(receiver))),
        })
    }

    /// Convenience method to connect and authenticate with a mining pool in one call
    pub async fn connect_and_auth(
        host: String,
        port: u16,
        username: &str,
        password: &str,
        miner: impl Miner + Send + 'static,
    ) -> Result<Self, StratumError> {
        let mut client = Self::new(host, port).await?;
        
        // Subscribe first
        client.subscribe().await?;
        
        // Then authorize
        let auth = client.authorize(username, password).await?;
        if !auth.authorized {
            return Err(StratumError::AuthenticationFailed(
                format!("Pool rejected credentials for user {}", username)
            ));
        }

        // Start mining task
        let mut client_clone = client.clone();
        tokio::spawn(async move {
            let mut current_job_id = String::new();
            
            loop {
                // Check if we have both a job and target
                if let Ok(Some(job)) = client_clone.get_current_job().await {
                    // Get target and check if we need to restart mining
                    let target = match client_clone.get_target().await {
                        Ok(target) => target,
                        Err(e) => {
                            log::warn!("No mining target available: {}", e);
                            continue;
                        }
                    };

                    // Start mining if we have a new job or difficulty changed
                    let should_mine = job.job_id != current_job_id || client_clone.job_manager.should_restart_mining().await;
                    log::info!("Starting mining with difficulty {} and job ID: {}", target.difficulty, job.job_id);

                    if should_mine {
                        current_job_id = job.job_id.clone();
                        
                        // Start mining with new job/target
                        if let Ok(nonce) = miner.on_job_received(job).await {
                            if let Some(sender) = &*client_clone.result_sender.lock().await {
                                let _ = sender.send(Ok(nonce)).await;
                            }
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });
        
        Ok(client)
    }

    /// Helper method to generate a unique extranonce2 value
    pub async fn generate_extranonce2(&self) -> Result<String, StratumError> {
        self.job_manager.generate_extranonce2_with_size().await
    }

    /// Get the server-provided extranonce1
    pub async fn get_extranonce1(&self) -> Result<String, StratumError> {
        self.job_manager.get_extranonce1().await
    }

    /// Take the result receiver channel
    pub async fn take_result_receiver(&mut self) -> Option<mpsc::Receiver<Result<u32, StratumError>>> {
        self.result_receiver.lock().await.take()
    }
}

#[async_trait]
impl StratumClient for StratumV1Client {
    /// Subscribe to the mining pool
    /// 
    /// This is typically the first step when connecting to a pool. The pool will respond
    /// with a subscription ID and extranonce1 value that will be used for mining.
    async fn subscribe(&mut self) -> Result<SubscribeResponse, StratumError> {
        let response = self.connection.lock().await
            .send_request(MINING_SUBSCRIBE, vec![json!(CLIENT_VERSION)])
            .await?;

        if let Some(error) = response.error {
            return Err(StratumError::SubscriptionFailed(error.to_string()));
        }

        let result = response.result.ok_or_else(|| {
            StratumError::SubscriptionFailed("No result in subscription response".into())
        })?;

        let subscription = result.as_array().ok_or_else(|| {
            StratumError::SubscriptionFailed("Invalid subscription format".into())
        })?;

        if subscription.len() < 3 {
            return Err(StratumError::SubscriptionFailed("Incomplete subscription data".into()));
        }

        let subscription_details = subscription[0].as_array()
            .ok_or_else(|| StratumError::SubscriptionFailed("Invalid subscription details format".into()))?;

        if subscription_details.is_empty() {
            return Err(StratumError::SubscriptionFailed("Empty subscription details".into()));
        }

        let first_detail = subscription_details[0].as_array()
            .ok_or_else(|| StratumError::SubscriptionFailed("Invalid subscription detail format".into()))?;

        if first_detail.len() < 2 {
            return Err(StratumError::SubscriptionFailed("Invalid subscription detail length".into()));
        }

        let subscription_id = first_detail[1].as_str()
            .ok_or_else(|| StratumError::SubscriptionFailed("Invalid subscription ID format".into()))?
            .to_string();
        let extranonce1 = subscription[1].as_str()
            .ok_or_else(|| StratumError::SubscriptionFailed("Invalid extranonce1 format".into()))?
            .to_string();
        let extranonce2_size = match &subscription[2] {
            Value::Number(n) => n.as_u64().unwrap_or(0) as usize,
            Value::Null => 0,
            _ => subscription[2].as_u64().unwrap_or(0) as usize,
        };

        // Store subscription data in job manager
        self.job_manager.set_subscription_data(extranonce1.clone(), extranonce2_size).await;

        Ok(SubscribeResponse {
            subscription_id,
            extranonce1,
            extranonce2_size,
        })
    }

    /// Authorize with the mining pool using worker credentials
    /// 
    /// This should be called after subscribing. The username is typically in the format
    /// "wallet_address.worker_name" or "username.worker_name" depending on the pool.
    async fn authorize(&mut self, username: &str, password: &str) -> Result<AuthResponse, StratumError> {
        let response = self.connection.lock().await
            .send_request(MINING_AUTHORIZE, vec![json!(username), json!(password)])
            .await?;

        let authorized = response.result.unwrap_or(json!(false)).as_bool().unwrap_or(false);
        
        Ok(AuthResponse {
            authorized,
            message: None,
        })
    }

    /// Submit a solved share to the mining pool
    /// 
    /// Returns true if the share was accepted, false if it was rejected.
    /// The share should be generated based on the current mining job and target difficulty.
    async fn submit_share(&mut self, share: Share) -> Result<bool, StratumError> {
        // Get the server's extranonce1
        let extranonce1 = self.job_manager.get_extranonce1().await?;
        
        // Generate new extranonce2 if not provided
        let extranonce2 = if share.extranonce2.is_empty() {
            self.job_manager.generate_extranonce2_with_size().await?
        } else {
            share.extranonce2
        };

        // Submit share with proper parameters
        let response = self.connection.lock().await
            .send_request(
                MINING_SUBMIT,
                vec![
                    json!(share.job_id),
                    json!(extranonce2),
                    json!(share.ntime),
                    json!(share.nonce),
                ],
            ).await?;

        Ok(response.result.unwrap_or(json!(false)).as_bool().unwrap_or(false))
    }

    /// Get the current mining job if available
    /// 
    /// New jobs are received through notifications, so this returns None if no job
    /// has been received yet. Use handle_notifications() to process new jobs.
    async fn get_current_job(&mut self) -> Result<Option<MiningJob>, StratumError> {
        self.job_manager.get_current_job().await
    }

    /// Process any pending notifications from the mining pool
    ///
    /// This should be called regularly to receive new jobs and difficulty updates.
    /// It processes one notification at a time, so call it in a loop during mining.
    async fn handle_notifications(&mut self) -> Result<(), StratumError> {
        let notification = self.connection.lock().await.read_notification().await?;
        
        if let Some(method) = notification.get("method").and_then(Value::as_str) {
            match method {
                method if method == MINING_NOTIFY => {
                    if let Some(params) = notification.get("params").and_then(Value::as_array) {
                        log::info!("Received new mining job with ID: {}",
                            params.get(0).and_then(Value::as_str).unwrap_or("unknown"));
                        self.job_manager.handle_job_notification(params).await?;
                    }
                }
                method if method == MINING_SET_DIFFICULTY => {
                    if let Some(params) = notification.get("params").and_then(Value::as_array) {
                        log::info!("Received new difficulty: {}",
                            params.get(0).and_then(Value::as_f64).unwrap_or(0.0));
                        self.job_manager.handle_difficulty_notification(params).await?;
                    }
                }
                _ => {} // Unknown method, ignore
            }
        }

        Ok(())
    }

    /// Get the current mining target
    async fn get_target(&self) -> Result<MiningTarget, StratumError> {
        self.job_manager.get_target().await
    }

    /// Get server information
    async fn get_server_info(&self) -> Result<ServerInfo, StratumError> {
        self.server_info.lock().await.clone()
            .ok_or_else(|| StratumError::Protocol("No server info available".into()))
    }

    /// Reconnect to the mining server
    async fn reconnect(&mut self) -> Result<(), StratumError> {
        self.connection.lock().await.reconnect().await
    }

    /// Close the connection
    async fn close(&mut self) -> Result<(), StratumError> {
        self.connection.lock().await.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn setup_mock_server() -> (TcpListener, String, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        (listener, addr.ip().to_string(), addr.port())
    }

    #[tokio::test]
    async fn test_subscribe() {
        let (listener, host, port) = setup_mock_server().await;
        
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0; 1024];
            socket.read(&mut buf).await.unwrap();
            
            let response = json!({
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
            
            socket.write_all(format!("{}\n", response).as_bytes()).await.unwrap();
        });

        let mut client = StratumV1Client::new(host, port).await.unwrap();
        let response = client.subscribe().await.unwrap();
        
        assert_eq!(response.subscription_id, "1");
        assert_eq!(response.extranonce1, "extranonce1");
        assert_eq!(response.extranonce2_size, 4);
    }

    #[tokio::test]
    async fn test_authorize() {
        let (listener, host, port) = setup_mock_server().await;
        
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0; 1024];
            socket.read(&mut buf).await.unwrap();
            
            let response = json!({
                "id": 1,
                "result": true,
                "error": null
            });
            
            socket.write_all(format!("{}\n", response).as_bytes()).await.unwrap();
        });

        let mut client = StratumV1Client::new(host, port).await.unwrap();
        let response = client.authorize("user", "pass").await.unwrap();
        
        assert!(response.authorized);
    }
}
