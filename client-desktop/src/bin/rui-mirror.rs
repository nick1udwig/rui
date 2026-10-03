use rui_client_desktop::desktop::{gtk, Options};
use rui_transport::pairing::Credential;
use std::{fs::File, io::Read};
fn read(path: &str, limit: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err("identity file exceeds limit".into());
    }
    Ok(bytes)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() < 6 || args.len() > 10 {
        return Err("usage: rui-mirror ADDRESS TLS_NAME PINNED_CERT.der CREDENTIAL CLIENT_ID_HEX [--control] [--smoke] [--session] [--scale=VALUE]".into());
    }
    if args[5].len() != 32 || !args[5].is_ascii() {
        return Err("client identity must be 16 bytes of hex".into());
    }
    let mut id = [0; 16];
    for (i, byte) in id.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&args[5][i * 2..i * 2 + 2], 16)?;
    }
    if args[6..].iter().any(|v| {
        v != "--control" && v != "--smoke" && v != "--session" && !v.starts_with("--scale=")
    }) {
        return Err("unknown option".into());
    }
    let scales = args[6..]
        .iter()
        .filter_map(|v| v.strip_prefix("--scale="))
        .collect::<Vec<_>>();
    if scales.len() > 1 {
        return Err("local display scale must be specified once".into());
    }
    let initial_scale = scales
        .first()
        .map_or(Ok(1.0), |value| value.parse::<f64>())?;
    if !initial_scale.is_finite() || !(0.125..=8.0).contains(&initial_scale) {
        return Err("local display scale must be between 0.125 and 8".into());
    }
    let mut bytes = read(&args[4], 32)?;
    let exact: Result<[u8; 32], _> = bytes.as_slice().try_into();
    bytes.fill(0);
    let credential =
        Credential::from_bytes(exact.map_err(|_| "credential must be exactly 32 bytes")?);
    gtk::run(Options {
        address: args[1].parse()?,
        server_name: args[2].clone(),
        certificate: read(&args[3], 65536)?,
        credential,
        client_id: id,
        view_selection: if args[6..].iter().any(|s| s == "--session") {
            rui_client_desktop::desktop::ViewSelection::WholeSession
        } else {
            rui_client_desktop::desktop::ViewSelection::Fixture
        },
        initial_scale,
        auto_control: args[6..].iter().any(|s| s == "--control"),
        smoke_frames: args[6..].iter().any(|s| s == "--smoke").then_some(1),
        title: "RUI Mirror",
        observations: None,
        shutdown_signal: None,
    })
}
