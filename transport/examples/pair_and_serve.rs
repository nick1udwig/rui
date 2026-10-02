//! Explicit local approval. Supply your own certificate and PKCS#8 private key.
use rui_host_broker::{ControlledHost, HostConfig};
use rui_protocol_core::session::{Permissions, VerifiedAuthorization};
use rui_transport::{pairing::PairingAuthority, HostIdentity, ServerConfig, ServerHandle};
use std::{
    error::Error,
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
};
fn bounded_file(path: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut data = Vec::new();
    std::fs::File::open(path)?
        .take(65537)
        .read_to_end(&mut data)?;
    if data.len() > 65536 {
        return Err("identity file too large".into());
    }
    Ok(data)
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err(
            "usage: pair_and_serve CERTIFICATE.der PRIVATE_KEY.pkcs8.der NEW_CREDENTIAL_FILE"
                .into(),
        );
    }
    let identity = HostIdentity::from_der(bounded_file(&args[1])?, bounded_file(&args[2])?)?;
    let fingerprint: String = identity
        .fingerprint()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut pairing = PairingAuthority::new(1)?;
    let request = pairing.request_local_pairing([3; 16])?;
    println!("Host certificate SHA-256: {fingerprint}");
    println!("Local request: client 03030303030303030303030303030303; principal local-controller; view + control; Activate only.");
    println!("Type APPROVE to grant this request and write a new credential file with owner-only permissions:");
    let mut decision = String::new();
    std::io::stdin().read_line(&mut decision)?;
    if decision.trim() != "APPROVE" {
        pairing.deny_local(request.request_id);
        return Err("local pairing denied".into());
    }
    let credential = pairing.approve_local(
        request.request_id,
        VerifiedAuthorization {
            principal: "local-controller".into(),
            policy_version: 1,
            permissions: Permissions {
                can_view: true,
                can_control: true,
                actions: ["Activate".into()].into_iter().collect(),
            },
        },
    )?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    {
        return Err("this example requires Unix owner-only credential file permissions".into());
    }
    let mut file = options.open(Path::new(&args[3]))?;
    file.write_all(credential.expose_for_tls_authentication())?;
    file.sync_all()?;
    drop(file);
    drop(credential);
    let host = ControlledHost::new(HostConfig::default(), "private fixture field".into())?;
    let server = ServerHandle::bind(
        "127.0.0.1:8443".parse()?,
        identity,
        host,
        Arc::new(Mutex::new(pairing)),
        ServerConfig::default(),
    )
    .await?;
    println!(
        "Pinned TLS RUI listener: {}; credential written. Press ENTER to stop.",
        server.address
    );
    tokio::task::spawn_blocking(|| {
        let mut stop = String::new();
        std::io::stdin().read_line(&mut stop)
    })
    .await??;
    server.shutdown().await;
    println!("Stopped; the in-memory approval is now invalid.");
    Ok(())
}
