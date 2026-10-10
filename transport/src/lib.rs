//! Authenticated, pinned TLS WebSocket binding for RUI's headless host.
//! Credentials require explicit local approval; the network cannot grant access.
pub use rui_wire::{codec, proto, TransportError};
#[cfg(feature = "native")]
mod client;
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
#[cfg(feature = "native")]
pub use tls::HostIdentity;

#[cfg(all(test, feature = "native"))]
mod tests {
    #[test]
    fn transport_logging_is_compiled_out() {
        assert_eq!(log::STATIC_MAX_LEVEL, log::LevelFilter::Off);
        log::trace!("plaintext must never be emitted");
    }
}
