mod producer;
mod projection;
use producer::{hex, unhex, BrowserPort, Worker};
use rui_protocol_core::session::{Permissions, VerifiedAuthorization};
use rui_transport::{
    pairing::PairingAuthority, producer::ProducerPort, HostIdentity, ProducerServerHandle,
    ServerConfig,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, BufRead},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
};
type Error = Box<dyn std::error::Error + Send + Sync>;
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.0.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Pending {
    client: [u8; 16],
    deadline: Instant,
    credential: Option<zeroize::Zeroizing<Vec<u8>>>,
}
fn permissions(client: [u8; 16]) -> VerifiedAuthorization {
    VerifiedAuthorization {
        principal: format!("device:{}", hex(&client)),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: true,
            actions: ["Activate", "Focus", "SetValue"]
                .map(str::to_owned)
                .into_iter()
                .collect(),
        },
    }
}
fn save(path: &std::path::Path, pairing: &PairingAuthority) -> Result<(), Error> {
    let data: Vec<_> = pairing
        .approved_records()
        .into_iter()
        .map(|(digest, client, _)| json!({"digest":hex(&digest),"client":hex(&client)}))
        .collect();
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec(&data)?)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    fs::rename(temporary, path)?;
    Ok(())
}
#[tokio::main]
async fn main() -> Result<(), Error> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err(
            "usage: rui-browser-host STATE_DIRECTORY WORKER_SCRIPT (use apps/browser/run.py)"
                .into(),
        );
    }
    let directory = PathBuf::from(&args[1]);
    let approvals_path = directory.join("devices.json");
    let mut authority = PairingAuthority::new(32)?;
    if approvals_path.exists() {
        for record in serde_json::from_slice::<Vec<Value>>(&fs::read(&approvals_path)?)? {
            let client =
                unhex(record["client"].as_str().ok_or("device client")?).ok_or("device client")?;
            let digest =
                unhex(record["digest"].as_str().ok_or("device digest")?).ok_or("device digest")?;
            authority.restore_local(digest, client, permissions(client))?;
        }
    }
    let pairing = Arc::new(Mutex::new(authority));
    let pending: Arc<Mutex<BTreeMap<String, Pending>>> = Default::default();
    let admin_path = directory.join("admin.sock");
    let worker_path = directory.join("worker.sock");
    for path in [&admin_path, &worker_path] {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    let admin = UnixListener::bind(&admin_path)?;
    let child = Command::new("node")
        .arg("--max-old-space-size=256")
        .arg(&args[2])
        .env("RUI_STATE", &directory)
        .spawn()?;
    let mut child = ChildGuard(child);
    let deadline = Instant::now() + Duration::from_secs(30);
    let worker = loop {
        if let Ok(w) = Worker::connect(&worker_path) {
            break w;
        }
        if child.0.try_wait()?.is_some() || Instant::now() > deadline {
            return Err("workspace worker failed to start".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let port = BrowserPort::new(worker)?;
    let origin = std::env::var("RUI_ORIGIN")?;
    let identity = HostIdentity::from_der(
        fs::read(directory.join("host.der"))?,
        fs::read(directory.join("host-key.der"))?,
    )?;
    let server = ProducerServerHandle::bind(
        "127.0.0.1:0".parse()?,
        identity,
        Arc::new(port.clone()),
        pairing.clone(),
        ServerConfig {
            allowed_browser_origins: vec![origin.clone()],
            ..Default::default()
        },
    )
    .await?;
    fs::write(
        directory.join("backend.json"),
        serde_json::to_vec(&json!({"port":server.address.port()}))?,
    )?;
    let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let local_pairing = pairing.clone();
    let local_pending = pending.clone();
    let local_stopped = stopped.clone();
    let local_port = port.clone();
    std::thread::spawn(move || {
        eprintln!(
            "Host commands: invite, approve CODE, deny CODE, devices, revoke CLIENT_ID, quit"
        );
        if std::env::var("RUI_WORKSPACE").as_deref() == Ok("desktop") {
            eprintln!("Desktop commands: apps, share APP_ID, unshare APP_ID, diagnose APP_ID (approvals last until host exit)");
        }
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            let words: Vec<_> = line.split_whitespace().collect();
            let result = (|| -> Result<(), Error> {
                match words.as_slice() {
                    ["invite"] => local_port.invite()?,
                    ["apps"] => {
                        let result = local_port.desktop_command("apps", None)?;
                        eprintln!("{}", serde_json::to_string_pretty(&result)?);
                    }
                    [op @ ("share" | "unshare" | "diagnose"), id] => {
                        let result = local_port.desktop_command(op, Some(id))?;
                        eprintln!("{}", serde_json::to_string_pretty(&result)?);
                    }
                    ["approve", code] => {
                        let mut pending = local_pending.lock().unwrap();
                        let item = pending.get_mut(*code).ok_or("unknown pairing code")?;
                        if item.deadline < Instant::now() {
                            return Err("pairing request expired".into());
                        }
                        let mut pairing = local_pairing.lock().unwrap();
                        let credential = pairing.approve_local(
                            unhex(code).ok_or("pairing code")?,
                            permissions(item.client),
                        )?;
                        item.credential = Some(zeroize::Zeroizing::new(
                            credential.expose_for_tls_authentication().to_vec(),
                        ));
                        save(&approvals_path, &pairing)?;
                        eprintln!("Device approved: {}", hex(&item.client));
                    }
                    ["deny", code] => {
                        local_pending.lock().unwrap().remove(*code);
                        if let Some(id) = unhex(code) {
                            local_pairing.lock().unwrap().deny_local(id);
                        }
                    }
                    ["revoke", client] => {
                        let id = unhex(client).ok_or("client id")?;
                        let mut p = local_pairing.lock().unwrap();
                        p.revoke_local(id);
                        save(&approvals_path, &p)?;
                        eprintln!("Device revoked");
                    }
                    ["devices"] => {
                        for (_, client, _) in local_pairing.lock().unwrap().approved_records() {
                            eprintln!("{}", hex(&client));
                        }
                    }
                    ["quit"] => {
                        local_stopped.store(true, std::sync::atomic::Ordering::Release);
                    }
                    _ => {
                        eprintln!("Use approve CODE, deny CODE, devices, revoke CLIENT_ID, or quit")
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                eprintln!("{error}");
            }
            if local_stopped.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
        }
    });
    let permits = Arc::new(tokio::sync::Semaphore::new(16));
    while !stopped.load(std::sync::atomic::Ordering::Acquire) {
        if child.0.try_wait()?.is_some() {
            break;
        }
        tokio::select! {
            accepted = admin.accept() => {
                let (socket, _) = accepted?;
                let peer_pid = socket.peer_cred().ok().and_then(|c| c.pid()).and_then(|pid| u32::try_from(pid).ok());
                let Ok(permit) = permits.clone().try_acquire_owned() else { continue; };
                let pairing = pairing.clone();
                let pending = pending.clone();
                let port = port.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let (read, mut write) = socket.into_split();
                    let mut reader = BufReader::new(read.take(4097));
                    let mut line = String::new();
                    let read = tokio::time::timeout(Duration::from_secs(3), reader.read_line(&mut line)).await;
                    if !matches!(read, Ok(Ok(n)) if n <= 4096) { return; }
                    let response = tokio::task::spawn_blocking(move || admin_request(&line, &pairing, &pending, &port, peer_pid))
                        .await.unwrap_or(json!({"error":"host unavailable"}));
                    let _ = write.write_all(format!("{}\n",response).as_bytes()).await;
                });
            },
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}

        }
    }
    port.shutdown();
    server.shutdown().await;
    let _ = fs::remove_file(admin_path);
    let _ = fs::remove_file(worker_path);
    Ok(())
}
fn admin_request(
    line: &str,
    pairing: &Mutex<PairingAuthority>,
    pending: &Mutex<BTreeMap<String, Pending>>,
    port: &BrowserPort,
    peer_pid: Option<u32>,
) -> Value {
    let result = (|| -> Result<Value, Error> {
        let request: Value = serde_json::from_str(line)?;
        // This socket lives in the owner's mode-0700 state directory. Public
        // HTTP routes never forward these operations. Protect the picker by its
        // kernel-observed PID before it creates an accessible window.
        if request["op"] == "desktop-protect" {
            let pid = peer_pid.ok_or("owner PID unavailable")?.to_string();
            return Ok(port.desktop_command("protect", Some(&pid))?);
        }
        if request["op"] == "desktop" {
            let op = request["command"].as_str().ok_or("desktop command")?;
            if !matches!(op, "apps" | "share" | "unshare" | "diagnose") {
                return Err("unsupported desktop command".into());
            }
            return Ok(port.desktop_command(op, request["id"].as_str())?);
        }

        let mut entries = pending.lock().unwrap();
        let mut auth = pairing.lock().unwrap();
        let expired: Vec<_> = entries
            .iter()
            .filter(|(_, p)| p.deadline < Instant::now())
            .map(|(k, _)| k.clone())
            .collect();
        for code in expired {
            entries.remove(&code);
            if let Some(id) = unhex(&code) {
                auth.deny_local(id);
            }
        }
        match request["op"].as_str() {
            Some("pair") => {
                let client = unhex(request["client"].as_str().ok_or("client")?).ok_or("client")?;
                let p = auth.request_local_pairing(client)?;
                let code = hex(&p.request_id);
                entries.insert(
                    code.clone(),
                    Pending {
                        client,
                        deadline: Instant::now() + Duration::from_secs(120),
                        credential: None,
                    },
                );
                eprintln!(
                    "\nPhone pairing request {}\nApprove locally: approve {}",
                    code, code
                );
                Ok(json!({"code":code}))
            }
            Some("poll") => {
                let code = request["code"].as_str().ok_or("code")?;
                let item = entries.get_mut(code).ok_or("pairing expired or denied")?;
                if request["client"].as_str() != Some(&hex(&item.client)) {
                    return Err("client mismatch".into());
                }
                Ok(match item.credential.take() {
                    Some(credential) => {
                        let result = json!({"credential":hex(&credential)});
                        entries.remove(code);
                        result
                    }
                    None => json!({"pending":true}),
                })
            }
            Some("auth") | Some("secret") => {
                let client = unhex(request["client"].as_str().ok_or("client")?).ok_or("client")?;
                let credential: [u8; 32] =
                    unhex(request["credential"].as_str().ok_or("credential")?)
                        .ok_or("credential")?;
                auth.authenticated(&credential, client)?;
                drop(auth);
                drop(entries);
                if request["op"] == "secret" {
                    Ok(port.secret_scope(client, request["target"].as_u64().ok_or("target")?)?)
                } else {
                    Ok(json!({"ok":true}))
                }
            }
            _ => Err("unsupported local request".into()),
        }
    })();
    result.unwrap_or_else(|_| json!({"error":"request rejected"}))
}
