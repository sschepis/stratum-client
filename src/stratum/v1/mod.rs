mod connection;
pub mod jobs;
mod protocol;

use crate::stratum::miner::Miner;
use crate::stratum::{error::StratumError, types::*, StratumClient};
use async_trait::async_trait;
use connection::StratumConnection;
use jobs::JobManager;
use protocol::{
    CLIENT_VERSION, MINING_AUTHORIZE, MINING_NOTIFY, MINING_SET_DIFFICULTY, MINING_SUBMIT,
    MINING_SUBSCRIBE,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

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
}

impl StratumV1Client {
    /// Creates a new Stratum V1 client and connects to the specified mining pool
    pub async fn new<M: Miner>(host: String, port: u16, miner: M) -> Result<Self, StratumError> {
        Ok(Self {
            connection: Arc::new(Mutex::new(StratumConnection::new(host, port).await?)),
            job_manager: JobManager::new(miner),
            server_info: Arc::new(Mutex::new(None)),
        })
    }

    pub async fn take_result_receiver(
        &self,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<Result<(u32, MiningJob), StratumError>>> {
        self.job_manager.result_receiver.lock().await.take()
    }

    /// Convenience method to connect and authenticate with a mining pool in one call
    pub async fn connect_and_auth<M: Miner>(
        host: String,
        port: u16,
        username: &str,
        password: &str,
        miner: M,
    ) -> Result<Self, StratumError> {
        let mut client = Self::new(host, port, miner).await?;

        // Subscribe first
        client.subscribe().await?;

        // Then authorize
        let auth = client.authorize(username, password).await?;
        if !auth.authorized {
            return Err(StratumError::AuthenticationFailed(format!(
                "Pool rejected credentials for user {}",
                username
            )));
        }

        Ok(client)
    }

    /// Helper method to generate a unique extranonce2 value
    pub fn generate_extranonce2(&self, size: usize) -> String {
        JobManager::generate_extranonce2(size)
    }
}

#[async_trait]
impl StratumClient for StratumV1Client {
    /// Subscribe to the mining pool
    ///
    /// This is typically the first step when connecting to a pool. The pool will respond
    /// with a subscription ID and extranonce1 value that will be used for mining.
    async fn subscribe(&mut self) -> Result<SubscribeResponse, StratumError> {
        let response = self
            .connection
            .lock()
            .await
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

        if subscription.len() < 2 {
            return Err(StratumError::SubscriptionFailed(
                "Incomplete subscription data".into(),
            ));
        }

        let subscription_details = subscription[0].as_array().ok_or_else(|| {
            StratumError::SubscriptionFailed("Invalid subscription details format".into())
        })?;

        if subscription_details.is_empty() {
            return Err(StratumError::SubscriptionFailed(
                "Empty subscription details".into(),
            ));
        }

        let first_detail = subscription_details[0].as_array().ok_or_else(|| {
            StratumError::SubscriptionFailed("Invalid subscription detail format".into())
        })?;

        if first_detail.len() < 2 {
            return Err(StratumError::SubscriptionFailed(
                "Invalid subscription detail length".into(),
            ));
        }

        let subscription_id = first_detail[1]
            .as_str()
            .ok_or_else(|| {
                StratumError::SubscriptionFailed("Invalid subscription ID format".into())
            })?
            .to_string();

        log::info!("Subscription data: {subscription:?}");

        let extranonce1 = subscription[1].as_str().unwrap_or_default().to_string();

        let extranonce2_size = match &subscription[2] {
            Value::Number(n) => n.as_u64().unwrap_or(0) as usize,
            Value::Null => 0,
            _ => subscription[2].as_u64().unwrap_or(0) as usize,
        };

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
    async fn authorize(
        &mut self,
        username: &str,
        password: &str,
    ) -> Result<AuthResponse, StratumError> {
        let response = self
            .connection
            .lock()
            .await
            .send_request(MINING_AUTHORIZE, vec![json!(username), json!(password)])
            .await?;

        let authorized = response
            .result
            .unwrap_or(json!(false))
            .as_bool()
            .unwrap_or(false);

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
        let response = self
            .connection
            .lock()
            .await
            .send_request(
                MINING_SUBMIT,
                vec![
                    json!(share.job_id),
                    json!(share.extranonce2),
                    json!(share.ntime),
                    json!(share.nonce),
                ],
            )
            .await?;

        Ok(response
            .result
            .unwrap_or(json!(false))
            .as_bool()
            .unwrap_or(false))
    }

    /// Get the current mining job if one is available
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
        log::info!(target: "stratum", "Received raw notification: {notification:?}");
        if let Some(method) = notification.get("method").and_then(Value::as_str) {
            match method {
                MINING_NOTIFY => {
                    if let Some(params) = notification.get("params").and_then(Value::as_array) {
                        self.job_manager.handle_job_notification(params).await?;
                    }
                }
                MINING_SET_DIFFICULTY => {
                    if let Some(params) = notification.get("params").and_then(Value::as_array) {
                        self.job_manager
                            .handle_difficulty_notification(params)
                            .await?;
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
        self.server_info
            .lock()
            .await
            .clone()
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
    use crate::stratum::v1::jobs::TestMiner;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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
                    10,
                ],
                "error": null
            });

            socket
                .write_all(format!("{}\n", response).as_bytes())
                .await
                .unwrap();
        });

        let mut client = StratumV1Client::new(host, port, TestMiner).await.unwrap();
        let response = client.subscribe().await.unwrap();

        assert_eq!(response.subscription_id, "1");
        assert_eq!(response.extranonce1, "extranonce1");
        assert_eq!(response.extranonce2_size, 10);
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

            socket
                .write_all(format!("{}\n", response).as_bytes())
                .await
                .unwrap();
        });

        let mut client = StratumV1Client::new(host, port, TestMiner).await.unwrap();
        let response = client.authorize("user", "pass").await.unwrap();

        assert!(response.authorized);
    }
}
