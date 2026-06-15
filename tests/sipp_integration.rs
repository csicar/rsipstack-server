//! Integration tests using sipp to test the SIP server
//!
//! These tests require sipp to be installed. Set the SIPP_PATH environment
//! variable if sipp is not in your PATH.
//!
//! **Important**: Run these tests with `--test-threads=1` to avoid port conflicts.
//!
//! ```bash
//! SIPP_PATH=$(which sipp) cargo test --test sipp_integration -- --test-threads=1
//! ```
//!
//! If sipp is not available, tests will be skipped.

use rsipstack_server::{
    async_trait, mpsc, AudioFrame, AudioHandler, CancellationToken, ServerConfig, SipHeaders,
    SipServer,
};
use std::net::UdpSocket;
use std::process::Command;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

// Global port counter to avoid conflicts between tests
static PORT_COUNTER: AtomicU16 = AtomicU16::new(17000);

/// Echo handler for testing
struct EchoHandler;

#[async_trait]
impl AudioHandler for EchoHandler {
    async fn process(
        &self,
        mut audio_in: mpsc::UnboundedReceiver<AudioFrame>,
        audio_out: mpsc::UnboundedSender<AudioFrame>,
        cancel_token: CancellationToken,
        _headers: SipHeaders,
    ) {
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                frame = audio_in.recv() => {
                    match frame {
                        Some(f) => {
                            if audio_out.send(f).is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    }
}

/// Allocate a UDP port that's guaranteed to be free
fn allocate_udp_port() -> u16 {
    // Try ports from our counter
    loop {
        let port = PORT_COUNTER.fetch_add(200, Ordering::SeqCst);
        if port > 60000 {
            panic!("Ran out of ports");
        }

        // Try to bind to verify it's available
        if UdpSocket::bind(format!("0.0.0.0:{}", port)).is_ok() {
            return port;
        }
    }
}

/// Get the sipp command path
fn get_sipp_command() -> Option<String> {
    // Check SIPP_PATH environment variable first
    if let Ok(path) = std::env::var("SIPP_PATH") {
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }
    }

    // Try to find sipp in PATH
    if Command::new("sipp").arg("-v").output().is_ok() {
        return Some("sipp".to_string());
    }

    None
}

/// Get the local IP address (first non-loopback)
fn get_local_ip() -> String {
    for iface in if_addrs::get_if_addrs().unwrap() {
        if !iface.is_loopback() {
            if let if_addrs::IfAddr::V4(ref addr) = iface.addr {
                return addr.ip.to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

/// Run sipp with given arguments
fn run_sipp(sipp_cmd: &str, args: &[&str]) -> std::process::Output {
    Command::new(sipp_cmd)
        .args(args)
        .output()
        .expect("Failed to execute sipp")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_echo_server_with_sipp() {
    let sipp_cmd = match get_sipp_command() {
        Some(cmd) => cmd,
        None => {
            eprintln!("Skipping test: sipp not available (set SIPP_PATH env var)");
            return;
        }
    };

    // Use dedicated ports for this test
    let sip_port = allocate_udp_port();
    let min_port = allocate_udp_port();
    let max_port = min_port + 99;

    let local_ip = get_local_ip();
    let server_addr = format!("{}:{}", local_ip, sip_port);

    eprintln!(
        "Starting server on {} (RTP from {})",
        server_addr, min_port
    );

    // Start the server
    let config = ServerConfig {
        port: sip_port,
        bind_addr: Some(local_ip.parse().unwrap()),
        external_ip: None,
        min_port,
        max_port,
    };

    let server = SipServer::new(config, || EchoHandler).await.unwrap();

    // Run server in background task
    let server_handle = tokio::spawn(async move {
        let _ = server.run().await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_secs(1)).await;
    eprintln!("Server should be ready, running sipp...");

    // Run sipp test
    let scenario_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test")
        .join("uac_pcap.xml");

    let sipp_result = run_sipp(
        &sipp_cmd,
        &[
            "-sf",
            scenario_path.to_str().unwrap(),
            &server_addr,
            "-m",
            "3", // 3 calls
            "-r",
            "1", // 1 call per second
            "-l",
            "1", // max 1 concurrent call
            "-d",
            "500", // 500ms call duration
            "-timeout",
            "30s", // 30 second timeout
            "-timeout_error",
        ],
    );

    let stdout = String::from_utf8_lossy(&sipp_result.stdout);
    let stderr = String::from_utf8_lossy(&sipp_result.stderr);

    // Cleanup: abort the server
    server_handle.abort();

    // Print output for debugging
    eprintln!("sipp exit code: {:?}", sipp_result.status.code());
    if !sipp_result.status.success() {
        eprintln!("sipp stdout:\n{}", stdout);
        eprintln!("sipp stderr:\n{}", stderr);
    }

    // Verify sipp succeeded
    assert!(
        sipp_result.status.success(),
        "sipp failed with exit code: {:?}",
        sipp_result.status.code()
    );

    // Parse sipp output to verify successful calls
    assert!(
        stdout.contains("Successful call") || stderr.contains("Successful call"),
        "Expected successful calls in sipp output"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multiple_concurrent_calls() {
    let sipp_cmd = match get_sipp_command() {
        Some(cmd) => cmd,
        None => {
            eprintln!("Skipping test: sipp not available (set SIPP_PATH env var)");
            return;
        }
    };

    let sip_port = allocate_udp_port();
    let min_port = allocate_udp_port();
    let max_port = min_port + 100;

    let local_ip = get_local_ip();
    let server_addr = format!("{}:{}", local_ip, sip_port);

    eprintln!(
        "Starting server on {} (RTP from {})",
        server_addr, min_port
    );

    let config = ServerConfig {
        port: sip_port,
        bind_addr: Some(local_ip.parse().unwrap()),
        external_ip: None,
        min_port,
        max_port
    };

    let server = SipServer::new(config, || EchoHandler).await.unwrap();

    let server_handle = tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_secs(1)).await;

    let scenario_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test")
        .join("uac_pcap.xml");

    // Test with multiple concurrent calls
    let sipp_result = run_sipp(
        &sipp_cmd,
        &[
            "-sf",
            scenario_path.to_str().unwrap(),
            &server_addr,
            "-m",
            "5", // 5 total calls
            "-r",
            "2", // 2 calls per second
            "-l",
            "3", // max 3 concurrent calls
            "-d",
            "500", // 500ms call duration
            "-timeout",
            "30s",
            "-timeout_error",
        ],
    );

    let stdout = String::from_utf8_lossy(&sipp_result.stdout);
    let stderr = String::from_utf8_lossy(&sipp_result.stderr);

    server_handle.abort();

    if !sipp_result.status.success() {
        eprintln!("sipp stdout:\n{}", stdout);
        eprintln!("sipp stderr:\n{}", stderr);
    }

    assert!(
        sipp_result.status.success(),
        "sipp failed with exit code: {:?}",
        sipp_result.status.code()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_server_handles_rapid_calls() {
    let sipp_cmd = match get_sipp_command() {
        Some(cmd) => cmd,
        None => {
            eprintln!("Skipping test: sipp not available (set SIPP_PATH env var)");
            return;
        }
    };

    let sip_port = allocate_udp_port();
    let min_port = allocate_udp_port();
    let max_port = min_port + 99;

    let local_ip = get_local_ip();
    let server_addr = format!("{}:{}", local_ip, sip_port);

    eprintln!(
        "Starting server on {} (RTP from {})",
        server_addr, min_port
    );

    let config = ServerConfig {
        port: sip_port,
        bind_addr: Some(local_ip.parse().unwrap()),
        external_ip: None,
        min_port,
        max_port
    };

    let server = SipServer::new(config, || EchoHandler).await.unwrap();

    let server_handle = tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_secs(1)).await;

    let scenario_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test")
        .join("uac_pcap.xml");

    // Rapid fire short calls
    let sipp_result = run_sipp(
        &sipp_cmd,
        &[
            "-sf",
            scenario_path.to_str().unwrap(),
            &server_addr,
            "-m",
            "10", // 10 total calls
            "-r",
            "5", // 5 calls per second
            "-l",
            "5", // max 5 concurrent calls
            "-d",
            "200", // 200ms call duration (short)
            "-timeout",
            "30s",
            "-timeout_error",
        ],
    );

    let stdout = String::from_utf8_lossy(&sipp_result.stdout);
    let stderr = String::from_utf8_lossy(&sipp_result.stderr);

    server_handle.abort();

    if !sipp_result.status.success() {
        eprintln!("sipp stdout:\n{}", stdout);
        eprintln!("sipp stderr:\n{}", stderr);
    }

    assert!(
        sipp_result.status.success(),
        "sipp failed with exit code: {:?}",
        sipp_result.status.code()
    );
}
