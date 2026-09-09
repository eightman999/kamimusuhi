//! TLS for the adapter, delegated to `rustls`.
//!
//! **Nothing in this file implements TLS.** Certificate chain validation,
//! expiry, hostname matching and the handshake itself are `rustls`'s job.
//! What lives here is the small amount of glue that decides *which* trust
//! anchors apply and translates a `rustls` failure into the vocabulary the
//! rest of Kamimusuhi classifies with. Hand-rolling certificate verification
//! would be the single worst thing this repository could do, and there is no
//! code path here that could.
//!
//! Trust anchors come from one of two places, both operator-chosen:
//!
//! - the Mozilla root set bundled by `webpki-roots`, which makes trust
//!   reproducible rather than dependent on whatever the host happens to have;
//! - a PEM file the operator points at, for a private CA.
//!
//! There is deliberately no "skip verification" option. A knob that disables
//! certificate checking is a knob that ends up on in production.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

/// How a TLS attempt failed, as a class rather than a transcript.
///
/// A certificate's contents are not carried anywhere: an operator needs to
/// know *which* check failed, and a subject or chain dumped into a log is
/// noise at best.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TlsFailureKind {
    /// The chain did not validate: unknown issuer, expired, revoked, malformed.
    Certificate,
    /// The certificate is valid but is not for the host we asked for.
    HostnameMismatch,
    /// The handshake failed for another reason: protocol, version, alert.
    Handshake,
    /// The trust anchors themselves could not be loaded.
    TrustAnchors,
}

impl TlsFailureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Certificate => "certificate",
            Self::HostnameMismatch => "hostname_mismatch",
            Self::Handshake => "handshake",
            Self::TrustAnchors => "trust_anchors",
        }
    }
}

impl std::fmt::Display for TlsFailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where trust anchors come from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TrustAnchors {
    /// The bundled Mozilla root set. Reproducible across machines.
    #[default]
    Webpki,
    /// A PEM file of one or more root certificates, for a private CA.
    /// Additional to nothing: these roots replace the default set, so a
    /// private-CA deployment does not silently keep trusting the public web.
    PemFile(std::path::PathBuf),
}

/// Build a client configuration.
///
/// The returned config verifies certificates and hostnames, because that is
/// `rustls`'s default and this function does not turn it off.
pub fn client_config(
    anchors: &TrustAnchors,
) -> Result<Arc<ClientConfig>, (TlsFailureKind, String)> {
    let mut roots = RootCertStore::empty();
    match anchors {
        TrustAnchors::Webpki => {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
        TrustAnchors::PemFile(path) => {
            // PEM parsing is rustls-pki-types' job too. Decoding certificate
            // framing by hand is exactly the class of code this module exists
            // not to contain.
            let certificates: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(path)
                .map_err(|source| {
                    (
                        TlsFailureKind::TrustAnchors,
                        format!("read {}: {source}", path.display()),
                    )
                })?
                .filter_map(Result::ok)
                .collect();
            let mut added = 0;
            for certificate in certificates {
                if roots.add(certificate).is_ok() {
                    added += 1;
                }
            }
            if added == 0 {
                return Err((
                    TlsFailureKind::TrustAnchors,
                    format!("{} contains no usable certificate", path.display()),
                ));
            }
        }
    }

    Ok(Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}

/// A connected TLS stream over a plain socket.
pub type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// Wrap an established TCP connection in TLS.
///
/// The handshake is driven eagerly so that a certificate problem surfaces here,
/// as a classified failure, rather than later as a confusing read error.
pub fn connect(
    config: Arc<ClientConfig>,
    host: &str,
    stream: TcpStream,
) -> Result<TlsStream, (TlsFailureKind, String)> {
    let server_name = ServerName::try_from(host.to_owned()).map_err(|_| {
        (
            TlsFailureKind::HostnameMismatch,
            format!("{host} is not a valid server name"),
        )
    })?;
    let connection =
        ClientConnection::new(config, server_name).map_err(|source| classify(&source))?;
    let mut tls = StreamOwned::new(connection, stream);

    // Drive the handshake to completion here, so a rejected certificate is
    // reported as the certificate problem it is rather than surfacing later as
    // a confusing read error in the middle of a request.
    while tls.conn.is_handshaking() {
        if let Err(source) = tls.conn.complete_io(&mut tls.sock) {
            return Err(from_io(&source));
        }
    }
    Ok(tls)
}

/// Turn an IO error raised during TLS into a classified failure, consulting
/// the connection for the underlying `rustls` error when there is one.
fn from_io(source: &std::io::Error) -> (TlsFailureKind, String) {
    if let Some(inner) = source
        .get_ref()
        .and_then(|e| e.downcast_ref::<rustls::Error>())
    {
        return classify(inner);
    }
    (TlsFailureKind::Handshake, source.to_string())
}

/// Map a `rustls` error onto a class. The message is `rustls`'s own; no
/// certificate content is added to it.
pub fn classify(error: &rustls::Error) -> (TlsFailureKind, String) {
    use rustls::CertificateError;
    let kind = match error {
        rustls::Error::InvalidCertificate(CertificateError::NotValidForName)
        | rustls::Error::InvalidCertificate(CertificateError::NotValidForNameContext { .. }) => {
            TlsFailureKind::HostnameMismatch
        }
        rustls::Error::InvalidCertificate(_) => TlsFailureKind::Certificate,
        _ => TlsFailureKind::Handshake,
    };
    (kind, error.to_string())
}

/// Classify an IO error that may wrap a `rustls` error.
pub fn classify_io(source: &std::io::Error) -> Option<(TlsFailureKind, String)> {
    source
        .get_ref()
        .and_then(|e| e.downcast_ref::<rustls::Error>())
        .map(classify)
}

/// Read and write halves the transport can use uniformly.
pub enum Transport {
    Plain(TcpStream),
    Tls(Box<TlsStream>),
}

impl Transport {
    pub fn socket(&self) -> &TcpStream {
        match self {
            Self::Plain(stream) => stream,
            // The named field, not `get_ref`: `StreamOwned` derefs to the
            // connection, so `get_ref` there resolves to something else
            // entirely and hands back a socket the deadlines never reach.
            Self::Tls(tls) => &tls.sock,
        }
    }
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buf),
            Self::Tls(tls) => tls.read(buf),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buf),
            Self::Tls(tls) => tls.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(tls) => tls.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_trust_anchors_load() {
        assert!(client_config(&TrustAnchors::Webpki).is_ok());
    }

    #[test]
    fn a_missing_or_empty_pem_is_refused_rather_than_falling_back() {
        // Falling back to the public root set because a private CA file was
        // missing would silently widen who is trusted.
        let missing = client_config(&TrustAnchors::PemFile("/nonexistent/ca.pem".into()));
        assert_eq!(missing.unwrap_err().0, TlsFailureKind::TrustAnchors);

        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, b"not a certificate").unwrap();
        assert_eq!(
            client_config(&TrustAnchors::PemFile(empty)).unwrap_err().0,
            TlsFailureKind::TrustAnchors
        );
    }

    #[test]
    fn rustls_errors_map_to_distinct_classes() {
        use rustls::CertificateError;
        assert_eq!(
            classify(&rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName
            ))
            .0,
            TlsFailureKind::HostnameMismatch
        );
        assert_eq!(
            classify(&rustls::Error::InvalidCertificate(
                CertificateError::UnknownIssuer
            ))
            .0,
            TlsFailureKind::Certificate
        );
        assert_eq!(
            classify(&rustls::Error::InvalidCertificate(
                CertificateError::Expired
            ))
            .0,
            TlsFailureKind::Certificate
        );
        assert_eq!(
            classify(&rustls::Error::NoCertificatesPresented).0,
            TlsFailureKind::Handshake
        );
    }

    #[test]
    fn failure_kind_vocabulary_is_stable() {
        for kind in [
            TlsFailureKind::Certificate,
            TlsFailureKind::HostnameMismatch,
            TlsFailureKind::Handshake,
            TlsFailureKind::TrustAnchors,
        ] {
            assert!(!kind.as_str().is_empty());
        }
    }
}
