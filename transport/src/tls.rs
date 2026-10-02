use crate::TransportError;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

pub struct HostIdentity {
    config: Arc<ServerConfig>,
    certificate: CertificateDer<'static>,
}
impl HostIdentity {
    pub fn from_der(
        certificate: Vec<u8>,
        pkcs8_private_key: Vec<u8>,
    ) -> Result<Self, TransportError> {
        let certificate = CertificateDer::from(certificate);
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|_| TransportError::Tls)?
            .with_no_client_auth()
            .with_single_cert(
                vec![certificate.clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8_private_key)),
            )
            .map_err(|_| TransportError::Tls)?;
        Ok(Self {
            config: Arc::new(config),
            certificate,
        })
    }
    pub fn pinned_certificate(&self) -> Vec<u8> {
        self.certificate.as_ref().to_vec()
    }
    pub fn fingerprint(&self) -> [u8; 32] {
        Sha256::digest(self.certificate.as_ref()).into()
    }
    pub(crate) fn acceptor(&self) -> TlsAcceptor {
        TlsAcceptor::from(self.config.clone())
    }
}
/// Pinned certificate plus normal rustls chain, validity, signature and hostname
/// checks. There is no insecure certificate verifier or accept-any fallback.
pub(crate) async fn connect_pinned(
    address: std::net::SocketAddr,
    name: String,
    pinned_certificate: Vec<u8>,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, TransportError> {
    let certificate = CertificateDer::from(pinned_certificate);
    let mut roots = RootCertStore::empty();
    roots
        .add(certificate.clone())
        .map_err(|_| TransportError::Tls)?;
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|_| TransportError::Tls)?
            .with_root_certificates(roots)
            .with_no_client_auth();
    let tcp = TcpStream::connect(address)
        .await
        .map_err(|_| TransportError::Io)?;
    tcp.set_nodelay(true).map_err(|_| TransportError::Io)?;
    let server_name = ServerName::try_from(name).map_err(|_| TransportError::Tls)?;
    let stream = TlsConnector::from(Arc::new(config))
        .connect(server_name, tcp)
        .await
        .map_err(|_| TransportError::Tls)?;
    if stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|chain| chain.first())
        != Some(&certificate)
    {
        return Err(TransportError::Pin);
    }
    Ok(stream)
}
