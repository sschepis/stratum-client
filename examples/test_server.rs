use serde_json::{json, Value};
use std::{error::Error, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Mutex},
    time::Duration,
};
use rand::{thread_rng, Rng};

struct MiningSession {
    difficulty: f64,
    extranonce1: String,
    subscription_id: String,
    authorized: bool,
    current_job_id: u64,
}

impl MiningSession {
    fn new(difficulty: f64) -> Self {
        let mut rng = thread_rng();
        let subscription_id = format!("{:016x}", rng.gen::<u64>());
        let extranonce1 = format!("{:08x}", rng.gen::<u32>());
        
        Self {
            difficulty,
            extranonce1,
            subscription_id,
            authorized: false,
            current_job_id: 0,
        }
    }

    fn generate_job(&mut self) -> Value {
        let mut rng = thread_rng();
        self.current_job_id += 1;
        
        json!({
            "method": "mining.notify",
            "params": [
                format!("{}", self.current_job_id), // job_id
                format!("{:064x}", rng.gen::<u64>()), // prev_hash
                format!("{:064x}", rng.gen::<u64>()), // coinbase1
                format!("{:064x}", rng.gen::<u64>()), // coinbase2
                vec![format!("{:064x}", rng.gen::<u64>())], // merkle_branch
                format!("{:08x}", 0x20000000), // version
                format!("{:08x}", rng.gen::<u32>()), // nbits
                format!("{:08x}", chrono::Utc::now().timestamp()), // ntime
                true // clean_jobs
            ]
        })
    }
}

async fn handle_client(
    stream: TcpStream,
    difficulty: f64,
) -> Result<(), Box<dyn Error>> {
    let (reader, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));
    let mut reader = BufReader::new(reader);
    let session = Arc::new(Mutex::new(MiningSession::new(difficulty)));
    
    // Send initial difficulty
    let difficulty_notify = json!({
        "method": "mining.set_difficulty",
        "params": [difficulty]
    });
    writer.lock().await.write_all(format!("{}\n", difficulty_notify).as_bytes()).await?;

    // Spawn job generation task
    let session_clone = session.clone();
    let writer_clone = writer.clone();
    tokio::spawn(async move {
        loop {
            let job = session_clone.lock().await.generate_job();
            if let Ok(job_str) = serde_json::to_string(&job) {
                let _ = writer_clone.lock().await.write_all(format!("{}\n", job_str).as_bytes()).await;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });

    // Handle client messages
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(_) => continue,
        };

        let id = request.get("id").and_then(Value::as_u64).unwrap_or(0);
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");

        let response = match method {
            "mining.subscribe" => {
                let session_guard = session.lock().await;
                json!({
                    "id": id,
                    "result": [
                        [
                            ["mining.set_difficulty", session_guard.subscription_id.clone()],
                            ["mining.notify", session_guard.subscription_id.clone()]
                        ],
                        session_guard.extranonce1.clone(),
                        4 // extranonce2_size
                    ],
                    "error": null
                })
            }
            "mining.authorize" => {
                let mut session_guard = session.lock().await;
                session_guard.authorized = true;
                json!({
                    "id": id,
                    "result": true,
                    "error": null
                })
            }
            "mining.submit" => {
                // For testing, accept all shares
                json!({
                    "id": id,
                    "result": true,
                    "error": null
                })
            }
            _ => json!({
                "id": id,
                "result": null,
                "error": ["Unknown method", -1, null]
            }),
        };

        writer.lock().await.write_all(format!("{}\n", response).as_bytes()).await?;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let addr = "127.0.0.1:3333";
    let difficulty = std::env::var("DIFFICULTY")
        .ok()
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(1.0);

    println!("Starting test mining pool on {} with difficulty {}", addr, difficulty);
    let listener = TcpListener::bind(addr).await?;

    loop {
        let (socket, addr) = listener.accept().await?;
        println!("New connection from {}", addr);
        
        tokio::spawn(async move {
            if let Err(e) = handle_client(socket, difficulty).await {
                eprintln!("Client error: {}", e);
            }
        });
    }
}
