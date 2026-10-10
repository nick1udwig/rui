//! Authenticated, pinned TLS WebSocket binding for RUI's headless host.
//! Credentials require explicit local approval; the network cannot grant access.
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/rui.session.v1.rs"));
}
#[cfg(feature = "native")]
mod client;
pub mod codec;
#[cfg(feature = "native")]
pub mod diagnostics;
#[cfg(feature = "native")]
pub mod pairing;
#[cfg(feature = "native")]
pub mod producer;
#[cfg(feature = "native")]
mod server;
#[cfg(feature = "native")]
mod source_delivery;
#[cfg(feature = "native")]
mod tls;
#[cfg(feature = "native")]
mod websocket;
#[cfg(feature = "native")]
pub use client::{ClientEvent, NetworkClient, OpenedView, RetainedClientView};
#[cfg(feature = "native")]
pub use server::{ProducerServerHandle, ServerConfig, ServerHandle};
use thiserror::Error;
#[cfg(feature = "native")]
pub use tls::HostIdentity;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransportError {
    #[error("registered producer still has a connection closing")]
    ProducerBusy,
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
