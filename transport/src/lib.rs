//! Authenticated, pinned TLS WebSocket binding for RUI's headless host.
//! Credentials require explicit local approval; the network cannot grant access.
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/rui.session.v1.rs"));
}
mod client;
pub mod codec;
pub mod pairing;
mod server;
mod source_delivery;
mod tls;
mod websocket;
pub use client::{ClientEvent, NetworkClient};
pub use server::{ServerConfig, ServerHandle};
use thiserror::Error;
pub use tls::HostIdentity;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransportError {
    #[error("TLS identity verification or handshake failed")]
    Tls,
    #[error("pinned host identity differs")]
    Pin,
    #[error("network I/O failed")]
    Io,
    #[error("WebSocket protocol failed")]
    WebSocket,
    #[error("message or allocation limit exceeded")]
    Limit,
    #[error("authentication or pairing approval failed")]
    Authentication,
    #[error("cryptographic randomness unavailable")]
    Entropy,
    #[error("invalid or unsupported required protocol message")]
    Protocol,
    #[error("message belongs to another session, epoch, or view")]
    Binding,
    #[error("peer closed the connection")]
    Closed,
    #[error("operation deadline exceeded")]
    Deadline,
    #[error("host authority rejected operation")]
    Authority,
    #[error("receiver rejected publication or resource")]
    Receiver,
}

impl std::fmt::Debug for proto::ClientHello {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClientHello([REDACTED])")
    }
}
impl std::fmt::Debug for proto::Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Packet")
            .field("kind", &self.kind)
            .field("payload", &"[REDACTED]")
            .finish()
    }
}

// Clear owned credentials and opaque private commands on errors/cancellation too.
impl Drop for proto::ClientHello {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.credential.zeroize();
    }
}
impl Drop for proto::Packet {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.payload.zeroize();
    }
}
