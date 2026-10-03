//! Private fixture TLS endpoints forwarding exact WebSocket messages. Both
//! legs use the fixture identity with normal rustls verification. A bounded
//! plan cuts before a selected complete application packet, never treats TCP
//! packet loss as permission to drop an ordered protocol message.
use futures_util::{SinkExt, StreamExt};
use prost::Message;
use rui_transport::proto as p;
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message as WsMessage};

pub const CEILING: usize = 16 * 1024;
pub struct Plan {
    pub seen: Vec<p::Kind>,
    pub remaining: Option<usize>,
    pub cut: bool,
    pub armed: bool,
    pub delay: Duration,
}
impl Default for Plan {
    fn default() -> Self {
        Self {
            seen: Vec::new(),
            remaining: None,
            cut: false,
            armed: true,
            delay: Duration::ZERO,
        }
    }
}
pub struct Proxy {
    pub address: SocketAddr,
    pub plan: Arc<Mutex<Plan>>,
    task: JoinHandle<()>,
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn tracked(kind: p::Kind) -> bool {
    matches!(
        kind,
        p::Kind::SnapshotBegin
            | p::Kind::TransactionBegin
            | p::Kind::PublicationChunk
            | p::Kind::PublicationCommit
            | p::Kind::ResourceOffer
            | p::Kind::ResourceChunk
            | p::Kind::ResourceComplete
            | p::Kind::CommandResult
    )
}
fn ws_config() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .read_buffer_size(4096)
        .write_buffer_size(0)
        .max_write_buffer_size(CEILING * 2 + 128)
        .max_message_size(Some(CEILING))
        .max_frame_size(Some(CEILING))
}
impl Proxy {
    #[allow(clippy::result_large_err)] // Required maintained WebSocket upgrade callback type.
    pub async fn start(upstream: SocketAddr, plan: Plan) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let plan = Arc::new(Mutex::new(plan));
        let shared = plan.clone();
        let task = tokio::spawn(async move {
            let cert = rustls::pki_types::CertificateDer::from(
                include_bytes!("../data/localhost.der").to_vec(),
            );
            let key = rustls::pki_types::PrivateKeyDer::try_from(
                include_bytes!("../data/localhost-key.der").to_vec(),
            )
            .unwrap();
            let server = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], key)
            .unwrap();
            let mut roots = rustls::RootCertStore::empty();
            roots.add(cert.clone()).unwrap();
            let client = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
            let (incoming, _) = listener.accept().await.unwrap();
            incoming.set_nodelay(true).unwrap();
            let incoming = TlsAcceptor::from(Arc::new(server))
                .accept(incoming)
                .await
                .unwrap();
            let mut down=tokio_tungstenite::accept_hdr_async_with_config(incoming, |_request:&tokio_tungstenite::tungstenite::handshake::server::Request,
                mut response:tokio_tungstenite::tungstenite::handshake::server::Response| {
                    response.headers_mut().insert("sec-websocket-protocol","rui.v1".parse().unwrap());
                    response.headers_mut().insert("x-rui-envelope-limit",CEILING.to_string().parse().unwrap());Ok(response)
                }, Some(ws_config())).await.unwrap();
            let tcp = TcpStream::connect(upstream).await.unwrap();
            tcp.set_nodelay(true).unwrap();
            let outgoing = TlsConnector::from(Arc::new(client))
                .connect("localhost".try_into().unwrap(), tcp)
                .await
                .unwrap();
            assert_eq!(
                outgoing.get_ref().1.peer_certificates().unwrap().first(),
                Some(&cert)
            );
            let mut request = format!("wss://localhost:{}/rui", upstream.port())
                .into_client_request()
                .unwrap();
            request
                .headers_mut()
                .insert("sec-websocket-protocol", "rui.v1".parse().unwrap());
            request
                .headers_mut()
                .insert("x-rui-envelope-limit", CEILING.to_string().parse().unwrap());
            let (mut up, _) =
                tokio_tungstenite::client_async_with_config(request, outgoing, Some(ws_config()))
                    .await
                    .unwrap();
            loop {
                tokio::select! {
                    msg=down.next()=>{let Some(Ok(msg))=msg else {break};if up.send(msg).await.is_err(){break}},
                    msg=up.next()=>{
                        let Some(Ok(msg))=msg else {break};
                        let mut delay=Duration::ZERO;
                        if let WsMessage::Binary(bytes)=&msg {
                            assert!(bytes.len()<=CEILING);
                            let packet=p::Packet::decode(bytes.as_ref()).unwrap();
                            let kind=p::Kind::try_from(packet.kind).unwrap();
                            if tracked(kind) {
                                let mut plan=shared.lock().unwrap();
                                if plan.armed {
                                    assert!(plan.seen.len()<1024);plan.seen.push(kind);
                                    if let Some(remaining)=plan.remaining.as_mut() {
                                        if *remaining==0 {plan.cut=true;break;}
                                        *remaining-=1;
                                    }
                                    delay=plan.delay;
                                }
                            }
                        }
                        if !delay.is_zero(){tokio::time::sleep(delay).await;}
                        if down.send(msg).await.is_err(){break}
                    }
                }
            }
        });
        Self {
            address,
            plan,
            task,
        }
    }
}
