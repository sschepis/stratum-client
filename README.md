# Rust Stratum Client Library

A robust and easy-to-use Rust implementation of the Stratum mining protocol. This library handles all the low-level details of pool communication while providing a simple interface for building mining clients.

## Features

- Full implementation of Stratum V1 protocol
- Automatic connection management and reconnection
- Robust error handling with retries
- Comprehensive documentation and examples
- Thread-safe with async/await support
- Built-in connection pooling and timeout handling

## Quick Start

Add the library to your `Cargo.toml`:

```toml
[dependencies]
rust-stratum = "0.1.0"
```

Basic usage example:

```rust
use rust_stratum::stratum::{create_client, StratumVersion, types::*};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create and connect a client in one step
    let mut client = StratumV1Client::connect_and_auth(
        "pool.example.com".to_string(),
        3333,
        "wallet_address.worker1",
        "x"
    ).await?;

    // Start mining loop
    loop {
        // Handle any new notifications (new jobs, difficulty changes)
        client.handle_notifications().await?;

        // Get current job
        if let Some(job) = client.get_current_job().await? {
            // Get current target
            let target = client.get_target().await?;
            println!("Mining at difficulty {}", target.difficulty);

            // Mine the job and submit shares
            let share = Share {
                job_id: job.job_id,
                extranonce2: client.generate_extranonce2(job.extranonce2_size),
                ntime: job.ntime,
                nonce: "00000000".to_string(),
            };
            
            if client.submit_share(share).await? {
                println!("Share accepted!");
            }
        }
    }
}
```

## Advanced Usage

The library provides fine-grained control over the mining process:

```rust
// Manual connection setup
let mut client = create_client(
    StratumVersion::V1,
    "pool.example.com".to_string(),
    3333
).await?;

// Subscribe to the pool
let subscription = client.subscribe().await?;
println!("Subscribed with ID: {}", subscription.subscription_id);
println!("Extranonce1: {}", subscription.extranonce1);
println!("Extranonce2 size: {}", subscription.extranonce2_size);

// Authorize with pool credentials
let auth = client.authorize("username.worker", "password").await?;
if !auth.authorized {
    return Err("Authorization failed".into());
}

// Get server info
let info = client.get_server_info().await?;
println!("Connected to server version: {}", info.version);

// Handle connection issues
if connection_lost {
    client.reconnect().await?;
}

// Clean shutdown
client.close().await?;
```

## Error Handling

The library provides detailed error types for handling different failure scenarios:

```rust
use rust_stratum::stratum::error::StratumError;

match client.submit_share(share).await {
    Ok(accepted) => {
        if accepted {
            println!("Share accepted!");
        } else {
            println!("Share rejected");
        }
    }
    Err(StratumError::Connection(e)) => {
        println!("Connection error: {}", e);
        client.reconnect().await?;
    }
    Err(StratumError::Protocol(e)) => {
        println!("Protocol error: {}", e);
    }
    Err(e) => println!("Other error: {}", e),
}
```

## Testing

The library includes comprehensive testing capabilities:

### Unit Tests

Each module has unit tests covering its functionality:

```bash
# Run unit tests
cargo test --lib
```

### Integration Tests

Integration tests verify the complete mining workflow:

```bash
# Run integration tests
cargo test --test integration_tests
```

These tests cover:
- Full mining cycle (subscribe, authorize, receive jobs, submit shares)
- Connection handling and reconnection
- Error scenarios and recovery
- Protocol validation

### Test Environment

For development and manual testing, the library provides:

### Test Server

Start the test server with a custom difficulty:

```bash
DIFFICULTY=2.0 cargo run --example test_server
```

This starts a local mining pool that:
- Listens on 127.0.0.1:3333
- Generates new jobs every 10 seconds
- Accepts all submitted shares
- Uses the specified difficulty (defaults to 1.0)

### Test Client

Connect to the test server:

```bash
cargo run --example basic_miner
```

Or connect to a different pool:

```bash
POOL_HOST=pool.example.com POOL_PORT=3333 POOL_USER=wallet.worker1 cargo run --example basic_miner
```

The test client:
- Connects to the specified pool (defaults to localhost:3333)
- Subscribes and authorizes
- Receives and processes jobs
- Submits test shares
- Handles difficulty changes

## Contributing

Contributions are welcome! Here's how you can help:

### Development Setup

1. Fork and clone the repository
2. Install dependencies: `cargo build`
3. Run tests: `cargo test`

### Testing Guidelines

When adding new features or making changes:

1. Add unit tests for new functionality
2. Update integration tests if needed
3. Verify both test server and client work
4. Run the full test suite: `cargo test --all`

### Code Style

- Follow Rust standard formatting: `cargo fmt`
- Ensure no linting errors: `cargo clippy`
- Add documentation for public APIs
- Keep functions focused and modular

### Pull Request Process

1. Create a feature branch
2. Add tests for new functionality
3. Update documentation as needed
4. Run the full test suite
5. Submit a PR with a clear description

### Common Tasks

#### Adding a New Feature

1. Add unit tests in the relevant module
2. Implement the feature
3. Add integration test if needed
4. Update documentation
5. Run tests and linting

#### Fixing a Bug

1. Add a test that reproduces the bug
2. Fix the issue
3. Verify the test passes
4. Add regression test if needed

### Getting Help

Feel free to open an issue for:
- Bug reports
- Feature requests
- Questions about the codebase

## License

This project is licensed under the MIT License - see the LICENSE file for details.
