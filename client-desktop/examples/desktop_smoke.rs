//! Two real GTK mirrors against the local pinned TLS host, on private Xvfb.
use rui_host_broker::{ControlledHost, HostConfig};
use rui_protocol_core::session::{CleanupReason, Permissions, VerifiedAuthorization};
use rui_transport::{pairing::PairingAuthority, HostIdentity, ServerConfig, ServerHandle};
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
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
struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("RUI_ISOLATED_DISPLAY").as_deref() != Ok("1") {
        return Err("launch through fixtures/render/native_smoke.py on its private display".into());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let mirror = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/debug/rui-mirror"));
    let directory = std::env::temp_dir().join(format!(
        "rui-mirror-smoke-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    std::fs::create_dir(&directory)?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    let _private = PrivateFiles(directory.clone());
    let cert = include_bytes!("../../transport/tests/data/localhost.der");
    std::fs::write(directory.join("host.der"), cert)?;
    let mut pairing = PairingAuthority::new(4)?;
    for id in [3u8, 4] {
        let request = pairing.request_local_pairing([id; 16])?;
        let credential = pairing.approve_local(
            request.request_id,
            VerifiedAuthorization {
                principal: format!("native-mirror-{id}"),
                policy_version: 1,
                permissions: Permissions {
                    can_view: true,
                    can_control: id == 3,
                    actions: if id == 3 {
                        ["Activate".into()].into_iter().collect()
                    } else {
                        Default::default()
                    },
                },
            },
        )?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join(format!("{id}.credential")))?
            .write_all(credential.expose_for_tls_authentication())?;
    }
    let cleanups = Arc::new(Mutex::new(None));
    let cleanup_sink = cleanups.clone();
    let server = ServerHandle::bind(
        "127.0.0.1:0".parse()?,
        HostIdentity::from_der(
            cert.to_vec(),
            include_bytes!("../../transport/tests/data/localhost-key.der").to_vec(),
        )?,
        ControlledHost::new(HostConfig::fresh()?, "private-source".into())?,
        Arc::new(Mutex::new(pairing)),
        ServerConfig {
            cleanup: Arc::new(move |cleanup| {
                let mut slot = cleanup_sink.lock().unwrap();
                assert!(slot.is_none());
                *slot = Some(cleanup);
            }),
            ..ServerConfig::default()
        },
    )
    .await?;
    let mut clients = Vec::new();
    for id in [3u8, 4] {
        let mut command = Command::new(&mirror);
        command.args([
            server.address.to_string(),
            "localhost".into(),
            directory.join("host.der").display().to_string(),
            directory
                .join(format!("{id}.credential"))
                .display()
                .to_string(),
            format!("{id:02x}").repeat(16),
            "--smoke".into(),
        ]);
        if id == 3 {
            command.arg("--control");
        }
        clients.push(Child(
            command.stdin(Stdio::null()).stdout(Stdio::null()).spawn()?,
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let mut all = true;
        for child in &mut clients {
            match child.0.try_wait()? {
                Some(status) => assert!(status.success(), "mirror failed"),
                None => all = false,
            }
        }
        if all {
            break;
        }
        if Instant::now() > deadline {
            return Err("native mirrors exceeded deadline".into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // A source lease is issued only after Applied + actual GUI Presented. Closing
    // the controller proves that grant reached the source and cleanup occurred.
    let mut cleanup = None;
    for _ in 0..100 {
        cleanup = cleanups.lock().unwrap().take();
        if cleanup.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let cleanup =
        cleanup.ok_or("source never granted/cleaned up control after native presentation")?;
    assert_eq!(cleanup.lease.client.client_instance_id, [3; 16]);
    assert_eq!(cleanup.reason, CleanupReason::Disconnect);
    assert!(cleanup.keys.is_empty() && cleanup.buttons.is_empty() && cleanup.touches.is_empty());
    let diagnostics = server.host.lock().await.diagnostics();
    assert_eq!(diagnostics.counter, 0);
    assert_eq!(diagnostics.consumed_commands, 0);
    server.shutdown().await;
    println!("native mirror: two GTK frames submitted; controller grant followed presentation; disconnect cleanup passed");
    Ok(())
}
