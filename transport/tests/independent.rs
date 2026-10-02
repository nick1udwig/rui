//! Real pinned TLS interoperability with the handwritten Python implementation.
use rui_host_broker::{ControlledHost, HostConfig, READ_SECRET_PERMISSION};
use rui_protocol_core::session::{Permissions, VerifiedAuthorization};
use rui_transport::{pairing::PairingAuthority, HostIdentity, ServerConfig, ServerHandle};
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct PrivateFiles(PathBuf);
impl Drop for PrivateFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn int_001_independent_semantic_client_uses_real_tls_and_exact_command_retry() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let directory = std::env::temp_dir().join(format!(
        "rui-independent-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let private = PrivateFiles(directory.clone());
    let mut pairing = PairingAuthority::new(2).unwrap();
    let request = pairing.request_local_pairing([7; 16]).unwrap();
    let credential = pairing
        .approve_local(
            request.request_id,
            VerifiedAuthorization {
                principal: "independent-semantic-fixture".into(),
                policy_version: 1,
                permissions: Permissions {
                    can_view: true,
                    can_control: true,
                    actions: ["Activate", "ReplaceText", READ_SECRET_PERMISSION]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                },
            },
        )
        .unwrap();
    let credential_path = directory.join("credential.bin");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&credential_path)
        .unwrap();
    file.write_all(credential.expose_for_tls_authentication())
        .unwrap();
    drop(file);
    drop(credential);
    let host = ControlledHost::new(HostConfig::default(), "A😀e\u{301}אב".into()).unwrap();
    let server = ServerHandle::bind(
        "127.0.0.1:0".parse().unwrap(),
        HostIdentity::from_der(
            include_bytes!("data/localhost.der").to_vec(),
            include_bytes!("data/localhost-key.der").to_vec(),
        )
        .unwrap(),
        host,
        Arc::new(Mutex::new(pairing)),
        ServerConfig::default(),
    )
    .await
    .unwrap();
    let port = server.address.port();
    let output = tokio::task::spawn_blocking(move || {
        let mut child = Command::new("python3")
            .current_dir(&root)
            .arg("conformance/semantic_client.py")
            .args(["--port", &port.to_string(), "--instance", &"07".repeat(16)])
            .arg("--certificate")
            .arg(root.join("transport/tests/data/localhost.der"))
            .arg("--credential")
            .arg(credential_path)
            .args([
                "--activate-label",
                "Increment",
                "--replace-label",
                "Private field",
                "--replacement",
                "Aé🦀Z",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        drop(private);
        output
    })
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "independent client failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("\"presented_acknowledgements\": 0"));
    assert_eq!(report.matches("\"status\": \"Applied\"").count(), 2);
    let diagnostics = server.host.lock().await.diagnostics();
    assert_eq!(diagnostics.counter, 1);
    assert_eq!(diagnostics.consumed_commands, 2);
    server.shutdown().await;
}
