use super::protocol::{JsonRpcRequest, JsonRpcResponse, DEFAULT_TIMEOUT, MAX_RETRIES};
use crate::stratum::error::StratumError;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
    sync::Mutex,
    time::{sleep, timeout},
};

/// Configuration for connection behavior
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// Timeout for network operations in seconds
    pub timeout: u64,
    /// Maximum number of retries for failed operations
    pub max_retries: u32,
    /// Delay between retries in seconds (exponential backoff multiplier)
    pub retry_delay: u64,
    /// Whether to enable TCP keepalive
    pub keepalive: bool,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_retries: MAX_RETRIES,
            retry_delay: 1,
            keepalive: true,
        }
    }
}

/// Statistics for the connection
#[derive(Debug, Default, Clone)]
pub struct ConnectionStats {
    pub messages_sent: u64,
    pub messages_received: u64,
    pub errors: u64,
    pub retries: u64,
    pub last_message_at: Option<Instant>,
    pub connected_since: Option<Instant>,
}

/// Handles the low-level network connection and message passing
pub struct StratumConnection {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    reader: Arc<Mutex<BufReader<OwnedReadHalf>>>,
    id_counter: AtomicU64,
    host: String,
    port: u16,
    config: ConnectionConfig,
    stats: Arc<Mutex<ConnectionStats>>,
}

impl StratumConnection {
    /// Create a new connection with default configuration
    pub async fn new(host: String, port: u16) -> Result<Self, StratumError> {
        Self::with_config(host, port, ConnectionConfig::default()).await
    }

    /// Create a new connection with custom configuration
    pub async fn with_config(
        host: String,
        port: u16,
        config: ConnectionConfig,
    ) -> Result<Self, StratumError> {
        let addr = format!("{}:{}", host, port);
        let stream = TcpStream::connect(&addr).await.map_err(|e| {
            StratumError::Connection(format!("Failed to connect to {} - {}", addr, e))
        })?;

        if config.keepalive {
            stream
                .set_nodelay(true)
                .map_err(|e| StratumError::Connection(format!("Failed to set nodelay - {}", e)))?;
        }

        let (read_half, write_half) = stream.into_split();

        let connection = Self {
            writer: Arc::new(Mutex::new(write_half)),
            reader: Arc::new(Mutex::new(BufReader::new(read_half))),
            id_counter: AtomicU64::new(1),
            host,
            port,
            config,
            stats: Arc::new(Mutex::new(ConnectionStats {
                connected_since: Some(Instant::now()),
                ..Default::default()
            })),
        };

        Ok(connection)
    }

    /// Get current connection statistics
    pub async fn stats(&self) -> ConnectionStats {
        self.stats.lock().await.clone()
    }

    /// Send a request and wait for response with automatic retries
    pub async fn send_request(
        &self,
        method: &str,
        params: Vec<Value>,
    ) -> Result<JsonRpcResponse, StratumError> {
        let mut retry_count = 0;
        let mut last_error = None;

        while retry_count < self.config.max_retries {
            let id = self.id_counter.fetch_add(1, Ordering::SeqCst);
            let request = JsonRpcRequest {
                id,
                method: method.to_string(),
                params: params.clone(),
            };

            let json = serde_json::to_string(&request).map_err(|e| {
                StratumError::Protocol(format!("Failed to serialize request - {}", e))
            })?;

            // Try to acquire locks with timeout
            let writer_lock = timeout(Duration::from_secs(self.config.timeout), self.writer.lock())
                .await
                .map_err(|_| {
                    let err = StratumError::Protocol("Writer lock timeout".into());
                    last_error = Some(err.clone());
                    err
                })?;

            let mut writer = writer_lock;

            // Send with timeout
            match timeout(
                Duration::from_secs(self.config.timeout),
                writer.write_all(format!("{}\n", json).as_bytes()),
            )
            .await
            {
                Ok(Ok(_)) => {
                    // Update stats
                    let mut stats = self.stats.lock().await;
                    stats.messages_sent += 1;
                    stats.last_message_at = Some(Instant::now());
                }
                Ok(Err(e)) => {
                    let err = StratumError::Protocol(format!("Write error: {}", e));
                    last_error = Some(err.clone());
                    retry_count += 1;
                    if retry_count == self.config.max_retries {
                        return Err(err);
                    }
                    sleep(Duration::from_secs(self.config.retry_delay << retry_count)).await;
                    continue;
                }
                Err(e) => {
                    let err = StratumError::Protocol(format!("Write timeout: {}", e));
                    last_error = Some(err.clone());
                    retry_count += 1;
                    if retry_count == self.config.max_retries {
                        return Err(err);
                    }
                    sleep(Duration::from_secs(self.config.retry_delay << retry_count)).await;
                    continue;
                }
            }

            let reader_lock = timeout(Duration::from_secs(self.config.timeout), self.reader.lock())
                .await
                .map_err(|_| {
                    let err = StratumError::Protocol("Reader lock timeout".into());
                    last_error = Some(err.clone());
                    err
                })?;

            let mut reader = reader_lock;
            let mut line = String::new();

            // Read with timeout
            match timeout(
                Duration::from_secs(self.config.timeout),
                reader.read_line(&mut line),
            )
            .await
            {
                Ok(Ok(0)) => {
                    let err = StratumError::Protocol("Empty response from server".into());
                    last_error = Some(err.clone());
                    retry_count += 1;
                    if retry_count == self.config.max_retries {
                        return Err(err);
                    }
                    sleep(Duration::from_secs(self.config.retry_delay << retry_count)).await;
                    continue;
                }
                Ok(Ok(_)) => {
                    match serde_json::from_str(&line) {
                        Ok(response) => {
                            let response: JsonRpcResponse = response;
                            println!("Client received response: {}", line.trim());
                            println!("Parsed response: {:?}", response);

                            // Update stats
                            let mut stats = self.stats.lock().await;
                            stats.messages_received += 1;
                            stats.last_message_at = Some(Instant::now());

                            if let Some(error) = response.error.as_ref() {
                                let err = StratumError::Protocol(
                                    serde_json::to_string(error)
                                        .unwrap_or_else(|_| error.to_string()),
                                );
                                return Err(err);
                            }
                            return Ok(response);
                        }
                        Err(e) => {
                            println!("Error parsing response: {}", e);
                            println!("Raw response line: {}", line);
                            let err =
                                StratumError::Protocol(format!("Invalid JSON response: {}", e));
                            last_error = Some(err.clone());
                            retry_count += 1;

                            // Update error stats
                            let mut stats = self.stats.lock().await;
                            stats.errors += 1;
                            stats.retries += 1;

                            if retry_count == self.config.max_retries {
                                return Err(err);
                            }
                            sleep(Duration::from_secs(self.config.retry_delay << retry_count))
                                .await;
                            continue;
                        }
                    }
                }
                Ok(Err(e)) => {
                    let err = StratumError::Protocol(format!("Read error: {}", e));
                    last_error = Some(err.clone());
                    retry_count += 1;
                    if retry_count == self.config.max_retries {
                        return Err(err);
                    }
                    sleep(Duration::from_secs(self.config.retry_delay << retry_count)).await;
                    continue;
                }
                Err(e) => {
                    let err = StratumError::Protocol(format!("Read timeout: {}", e));
                    last_error = Some(err.clone());
                    retry_count += 1;
                    if retry_count == self.config.max_retries {
                        return Err(err);
                    }
                    sleep(Duration::from_secs(self.config.retry_delay << retry_count)).await;
                    continue;
                }
            }
        }

        // Update error stats
        let mut stats = self.stats.lock().await;
        stats.errors += 1;
        stats.retries += retry_count as u64;

        // Return last error or generic max retries error
        Err(last_error.unwrap_or_else(|| {
            StratumError::Protocol(format!(
                "Max retries ({}) exceeded",
                self.config.max_retries
            ))
        }))
    }

    /// Read a single notification from the server
    pub async fn read_notification(&self) -> Result<Value, StratumError> {
        let reader_lock = timeout(Duration::from_secs(self.config.timeout), self.reader.lock())
            .await
            .map_err(|_| StratumError::Protocol("Reader lock timeout in notifications".into()))?;

        // Update error stats if we got a timeout
        {
            let mut stats = self.stats.lock().await;
            stats.errors += 1;
        }

        let mut reader = reader_lock;
        let mut line = String::new();

        loop {
            match timeout(
                Duration::from_secs(self.config.timeout),
                reader.read_line(&mut line),
            )
            .await
            {
                Ok(Ok(0)) => return Ok(json!(null)), // No data available
                Ok(Ok(_)) => {
                    return match serde_json::from_str(&line.trim()) {
                        Ok(value) => {
                            // Update stats
                            let mut stats = self.stats.lock().await;
                            stats.messages_received += 1;
                            stats.last_message_at = Some(Instant::now());
                            Ok(value)
                        }
                        Err(e) => {
                            if line.trim().is_empty() {
                                Ok(json!(null))
                            } else {
                                let err = StratumError::Protocol(format!(
                                    "Invalid JSON notification: {}",
                                    e
                                ));
                                let mut stats = self.stats.lock().await;
                                stats.errors += 1;
                                Err(err)
                            }
                        }
                    };
                }
                Ok(Err(e)) => {
                    let err = StratumError::Protocol(format!("Read error in notifications: {}", e));
                    let mut stats = self.stats.lock().await;
                    stats.errors += 1;
                    return Err(err);
                }
                Err(e) => {
                    log::warn!(target: "stratum", "Read timeout in notifications, retrying ...: {e}");
                    continue;
                }
            }
        }
    }

    /// Reconnect to the server
    pub async fn reconnect(&mut self) -> Result<(), StratumError> {
        let addr = format!("{}:{}", self.host, self.port);
        let stream = TcpStream::connect(&addr).await.map_err(|e| {
            StratumError::Connection(format!("Failed to connect to {} - {}", addr, e))
        })?;

        if self.config.keepalive {
            stream
                .set_nodelay(true)
                .map_err(|e| StratumError::Connection(format!("Failed to set nodelay - {}", e)))?;
        }

        let (read_half, write_half) = stream.into_split();
        *self.writer.lock().await = write_half;
        *self.reader.lock().await = BufReader::new(read_half);

        // Reset stats
        let mut stats = self.stats.lock().await;
        *stats = ConnectionStats {
            connected_since: Some(Instant::now()),
            ..Default::default()
        };

        Ok(())
    }

    /// Close the connection
    pub async fn close(&mut self) -> Result<(), StratumError> {
        let mut writer = self.writer.lock().await;
        writer.shutdown().await?;

        // Clear stats
        let mut stats = self.stats.lock().await;
        stats.connected_since = None;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn setup_test_server() -> (TcpListener, String, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        (listener, addr.ip().to_string(), addr.port())
    }

    #[tokio::test]
    async fn test_connection_config() {
        let config = ConnectionConfig {
            timeout: 10,
            max_retries: 5,
            retry_delay: 2,
            keepalive: true,
        };

        let (listener, host, port) = setup_test_server().await;

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 1024];
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

        let conn = StratumConnection::with_config(host, port, config)
            .await
            .unwrap();
        assert_eq!(conn.config.timeout, 10);
        assert_eq!(conn.config.max_retries, 5);
        assert_eq!(conn.config.retry_delay, 2);
        assert!(conn.config.keepalive);
    }

    #[tokio::test]
    async fn test_connection_stats() {
        let (listener, host, port) = setup_test_server().await;

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 1024];
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

        let conn = StratumConnection::new(host, port).await.unwrap();

        // Check initial stats
        let stats = conn.stats().await;
        assert_eq!(stats.messages_sent, 0);
        assert_eq!(stats.messages_received, 0);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.retries, 0);
        assert!(stats.connected_since.is_some());

        // Send a request
        conn.send_request("test", vec![]).await.unwrap();

        // Check updated stats
        let stats = conn.stats().await;
        assert_eq!(stats.messages_sent, 1);
        assert_eq!(stats.messages_received, 1);
        assert_eq!(stats.errors, 0);
        assert!(stats.last_message_at.is_some());
    }

    #[tokio::test]
    async fn test_connection_errors() {
        let (listener, host, port) = setup_test_server().await;

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 1024];
            socket.read(&mut buf).await.unwrap();

            // Send invalid JSON to trigger error
            socket.write_all(b"invalid json\n").await.unwrap();
        });

        let conn = StratumConnection::new(host, port).await.unwrap();

        // Send request and expect error
        let result = conn.send_request("test", vec![]).await;
        assert!(result.is_err());

        // Check error stats
        let stats = conn.stats().await;
        assert!(stats.errors > 0);
    }

    #[tokio::test]
    async fn test_reconnection() {
        let (listener, host, port) = setup_test_server().await;

        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            drop(socket); // Force disconnect

            // Accept new connection after reconnect
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 1024];
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

        let mut conn = StratumConnection::new(host, port).await.unwrap();

        // Force a reconnection
        conn.reconnect().await.unwrap();

        // Verify connection works after reconnect
        let result = conn.send_request("test", vec![]).await;
        assert!(result.is_ok());

        // Check stats were reset
        let stats = conn.stats().await;
        assert_eq!(stats.messages_sent, 1);
        assert_eq!(stats.messages_received, 1);
        assert_eq!(stats.errors, 0);
        assert!(stats.connected_since.is_some());
    }
}
