//! Connections to mail servers. A connection is encrypted unless the server is on this machine or the local network,
//! where plain text is allowed so a server of one's own can be reached (and so tests can run without certificates).

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
pub type Stream = Box<dyn Io>;

pub const STEP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    /// TLS from the first byte (IMAP 993, SMTP 465).
    Tls,
    /// Plain first, then upgraded with STARTTLS (SMTP 587).
    StartTls,
    Plain,
}

impl Security {
    pub fn parse(text: &str) -> Option<Security> {
        match text {
            "tls" => Some(Security::Tls),
            "starttls" => Some(Security::StartTls),
            "plain" => Some(Security::Plain),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub security: Security,
}

pub fn is_local_host(host: &str) -> bool {
    host == "localhost"
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
            std::net::IpAddr::V6(v6) => v6.is_loopback(),
        })
}

/// A server name that can't be anything but a host: letters, digits, dots, dashes.
pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.chars().all(|c| c.is_ascii_alphanumeric() || ".-:_".contains(c))
        && !host.starts_with('-')
}

impl Endpoint {
    pub fn check(&self) -> Result<(), String> {
        if !valid_host(&self.host) || self.port == 0 {
            return Err("The server address isn't valid.".into());
        }
        if self.security == Security::Plain && !is_local_host(&self.host) {
            return Err("An unencrypted connection is only allowed to a server on this network.".into());
        }
        Ok(())
    }
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = rustls_platform_verifier::Verifier::new(provider.clone()).map_err(|e| e.to_string())?;
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

pub async fn upgrade(stream: Stream, host: &str) -> Result<Stream, String> {
    let connector = tokio_rustls::TlsConnector::from(tls_config()?);
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|_| "The server name isn't valid.".to_string())?;
    let tls = tokio::time::timeout(STEP_TIMEOUT, connector.connect(name, stream))
        .await
        .map_err(|_| "The secure connection timed out.".to_string())?
        .map_err(|_| "The secure connection to the server failed. Check the address and that the server's certificate is valid.".to_string())?;
    Ok(Box::new(tls))
}

/// Opens the connection. For `StartTls` the plain stream is returned: the protocol decides when to upgrade.
pub async fn connect(endpoint: &Endpoint) -> Result<Stream, String> {
    endpoint.check()?;
    let tcp = tokio::time::timeout(STEP_TIMEOUT, TcpStream::connect((endpoint.host.as_str(), endpoint.port)))
        .await
        .map_err(|_| "The mail server didn't answer in time.".to_string())?
        .map_err(|_| "The mail server couldn't be reached.".to_string())?;
    let stream: Stream = Box::new(tcp);
    match endpoint.security {
        Security::Tls => upgrade(stream, &endpoint.host).await,
        Security::StartTls | Security::Plain => Ok(stream),
    }
}
