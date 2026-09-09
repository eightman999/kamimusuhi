//! The adapter over TLS, against a real TLS server on a real socket.
//!
//! Every case here completes a genuine handshake (or genuinely fails to). The
//! certificates are generated per test by `rcgen` and the server side is
//! `rustls`, so "the certificate is not trusted" means webpki actually
//! rejected a chain — not that a mock returned an error enum.
//!
//! What is being tested is *our* boundary: that we delegate verification, that
//! we classify the outcome correctly, that a TLS failure is not retried, and
//! that nothing about a certificate or a credential leaks into an error. The
//! correctness of TLS itself is `rustls`'s problem and is not re-litigated
//! here.
//!
//! Covers the W6 acceptance items for valid HTTPS, invalid certificate,
//! hostname mismatch, handshake failure, timeout, secret non-recording and
//! error attribution.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use kamimusuhi_core::ids::{IndividualId, ResourceId};
use kamimusuhi_core::resources::{CognitiveResource, ResourceRequest};
use kamimusuhi_resource_http::{OpenAiCompatibleConfig, OpenAiCompatibleResource, TrustAnchors};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

const RESOURCE: ResourceId = ResourceId::from_u128(0x0B01);

/// A private CA plus one certificate it signed.
struct Authority {
    ca_pem: String,
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

/// Mint a CA and a server certificate for `host`.
///
/// Two authorities generated separately share no trust, which is how the
/// "untrusted certificate" case is produced without any special casing.
fn authority(host: &str) -> Authority {
    let mut ca_params = CertificateParams::new(Vec::new()).expect("ca params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "kamimusuhi test ca");
    let ca_key = KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("self-signed ca");
    let ca_pem = ca_cert.pem();

    let issuer = Issuer::new(ca_params, ca_key);
    let server_params = CertificateParams::new(vec![host.to_owned()]).expect("server params");
    let server_key = KeyPair::generate().expect("server key");
    let server_cert = server_params
        .signed_by(&server_key, &issuer)
        .expect("signed server cert");

    Authority {
        ca_pem,
        chain: vec![server_cert.der().clone(), ca_cert.der().clone()],
        key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key.serialize_der())),
    }
}

/// How the TLS fixture should behave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    /// Complete the handshake and answer a chat completion.
    Answer,
    /// Complete the handshake, then stall past the client's deadline.
    StallAfterHandshake,
    /// Not TLS at all: reply to the ClientHello with plain HTTP bytes.
    NotTls,
}

struct TlsFixture {
    port: u16,
    stop: Arc<AtomicBool>,
    served: Arc<AtomicUsize>,
    handle: Option<JoinHandle<()>>,
}

impl TlsFixture {
    fn start(authority: &Authority, behaviour: Behaviour) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        listener.set_nonblocking(true).expect("nonblocking");

        let config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(authority.chain.clone(), authority.key.clone_key())
                .expect("server config"),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let served = Arc::new(AtomicUsize::new(0));
        let (worker_stop, worker_served) = (Arc::clone(&stop), Arc::clone(&served));

        let handle = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        worker_served.fetch_add(1, Ordering::SeqCst);
                        let config = Arc::clone(&config);
                        std::thread::spawn(move || serve(stream, config, behaviour));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            port,
            stop,
            served,
            handle: Some(handle),
        }
    }

    /// Reached by name, not by address: a certificate is issued for a name, so
    /// hostname verification only means anything if we ask for one.
    fn base_url(&self) -> String {
        format!("https://localhost:{}/v1", self.port)
    }

    fn connections(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }
}

impl Drop for TlsFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve(mut stream: TcpStream, config: Arc<ServerConfig>, behaviour: Behaviour) {
    // Accepted sockets inherit the listener's non-blocking flag on some
    // platforms; a non-blocking TLS read returns EAGAIN and aborts the
    // handshake for no reason.
    stream.set_nonblocking(false).expect("blocking mode");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    if behaviour == Behaviour::NotTls {
        // Answer a ClientHello with plain HTTP. The client must fail the
        // handshake rather than try to read it as a response.
        let mut scratch = [0_u8; 1024];
        let _ = stream.read(&mut scratch);
        let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
        let _ = stream.shutdown(Shutdown::Both);
        return;
    }

    let Ok(connection) = ServerConnection::new(config) else {
        return;
    };
    let mut tls = StreamOwned::new(connection, stream);
    // Drive the handshake; a client that rejects our certificate fails here.
    if tls.flush().is_err() {
        return;
    }

    if behaviour == Behaviour::StallAfterHandshake {
        std::thread::sleep(Duration::from_millis(3_000));
        return;
    }

    let mut buffer = Vec::new();
    let mut scratch = [0_u8; 1024];
    while !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
        match tls.read(&mut scratch) {
            Ok(0) => return,
            Ok(read) => buffer.extend_from_slice(&scratch[..read]),
            Err(_) => return,
        }
    }
    let body = serde_json::json!({
        "model": "fixture-model",
        "choices": [{ "message": { "role": "assistant", "content": "hello over tls" } }],
    })
    .to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = tls.write_all(response.as_bytes());
    let _ = tls.flush();
    // Close cleanly, the way a well-behaved server does. The client tolerates
    // servers that do not, but the fixture should not be one of them.
    tls.conn.send_close_notify();
    let _ = tls.conn.complete_io(&mut tls.sock);
    let _ = tls.sock.shutdown(Shutdown::Both);
}

fn write_pem(pem: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ca.pem");
    std::fs::write(&path, pem).expect("write ca");
    (dir, path)
}

fn request() -> ResourceRequest {
    ResourceRequest::new(
        IndividualId::from_u128(0xA1),
        "summarize-turn",
        serde_json::json!({ "evidence_id": "e1" }),
    )
}

fn resource(base_url: &str, anchors: TrustAnchors, max_attempts: u32) -> OpenAiCompatibleResource {
    OpenAiCompatibleResource::new(
        OpenAiCompatibleConfig::new(RESOURCE, base_url, "fixture-model")
            .with_timeout_ms(4_000)
            .with_max_attempts(max_attempts)
            .with_retry_backoff_ms(1)
            .with_trust_anchors(anchors),
    )
}

#[test]
fn a_trusted_certificate_completes_the_handshake_and_the_call() {
    let authority = authority("localhost");
    let (_dir, ca_path) = write_pem(&authority.ca_pem);
    let server = TlsFixture::start(&authority, Behaviour::Answer);

    let result = resource(&server.base_url(), TrustAnchors::PemFile(ca_path), 1)
        .invoke(&request())
        .expect("a valid certificate must not block the call");

    assert_eq!(result.content["answer"], "hello over tls");
    assert_eq!(result.resource_id, RESOURCE);
    assert_eq!(result.attempts, 1);
}

#[test]
fn a_certificate_from_an_untrusted_issuer_is_refused() {
    // Two authorities minted independently: the client trusts one, the server
    // presents the other's. Nothing special-cases this — it is simply an
    // unknown issuer.
    let server_authority = authority("localhost");
    let other_authority = authority("localhost");
    let (_dir, other_ca) = write_pem(&other_authority.ca_pem);
    let server = TlsFixture::start(&server_authority, Behaviour::Answer);

    let error = resource(&server.base_url(), TrustAnchors::PemFile(other_ca), 3)
        .invoke(&request())
        .expect_err("an untrusted certificate must not be accepted");

    assert_eq!(error.code(), "TLS");
    assert!(
        error.to_string().contains("certificate"),
        "expected a certificate class, got {error}"
    );
    // Not retried: a certificate that does not validate will not validate on
    // the next attempt, and retrying would blunt a security signal.
    assert_eq!(error.attempts(), 1);
    assert_eq!(server.connections(), 1);
}

#[test]
fn a_certificate_for_the_wrong_host_is_refused_even_though_the_chain_is_trusted() {
    let authority = authority("not-the-host.invalid");
    let (_dir, ca_path) = write_pem(&authority.ca_pem);
    let server = TlsFixture::start(&authority, Behaviour::Answer);

    let error = resource(&server.base_url(), TrustAnchors::PemFile(ca_path), 2)
        .invoke(&request())
        .expect_err("a certificate for another host must not be accepted");

    assert_eq!(error.code(), "TLS");
    assert!(
        error.to_string().contains("hostname_mismatch"),
        "expected a hostname mismatch, got {error}"
    );
    assert_eq!(error.attempts(), 1);
}

#[test]
fn a_server_that_does_not_speak_tls_fails_the_handshake() {
    let authority = authority("localhost");
    let (_dir, ca_path) = write_pem(&authority.ca_pem);
    let server = TlsFixture::start(&authority, Behaviour::NotTls);

    let error = resource(&server.base_url(), TrustAnchors::PemFile(ca_path), 1)
        .invoke(&request())
        .expect_err("plain HTTP on an https endpoint is not a response");

    // Handshake or transport, depending on how the peer's bytes are rejected;
    // what matters is that it is not mistaken for a reply.
    assert!(
        matches!(error.code(), "TLS" | "TRANSPORT" | "MALFORMED_RESPONSE"),
        "unexpected class {}",
        error.code()
    );
}

#[test]
fn a_stalled_tls_connection_times_out_rather_than_hanging() {
    let authority = authority("localhost");
    let (_dir, ca_path) = write_pem(&authority.ca_pem);
    let server = TlsFixture::start(&authority, Behaviour::StallAfterHandshake);

    let started = std::time::Instant::now();
    let error = OpenAiCompatibleResource::new(
        OpenAiCompatibleConfig::new(RESOURCE, server.base_url(), "fixture-model")
            .with_timeout_ms(300)
            .with_max_attempts(1)
            .with_trust_anchors(TrustAnchors::PemFile(ca_path)),
    )
    .invoke(&request())
    .expect_err("a stalled server must not look like success");

    assert_eq!(error.code(), "TIMEOUT", "got {error}");
    assert!(
        started.elapsed() < Duration::from_millis(2_500),
        "the deadline was not enforced: waited {:?}",
        started.elapsed()
    );
}

#[test]
fn a_tls_failure_carries_no_certificate_content_and_no_credential() {
    const TOKEN_ENV: &str = "KAMIMUSUHI_W6_TLS_TOKEN";
    const TOKEN_VALUE: &str = "w6-secret-value";

    // As in W5: the only honest way to test a real environment read is a real
    // environment, and `set_var` is unsafe while the workspace forbids unsafe.
    let Ok(token) = std::env::var(TOKEN_ENV) else {
        let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "a_tls_failure_carries_no_certificate_content_and_no_credential",
                "--exact",
                "--nocapture",
            ])
            .env(TOKEN_ENV, TOKEN_VALUE)
            .status()
            .expect("re-run with the variable set");
        assert!(status.success(), "the child run failed");
        return;
    };
    assert_eq!(token, TOKEN_VALUE);

    let server_authority = authority("localhost");
    let other_authority = authority("localhost");
    let (_dir, other_ca) = write_pem(&other_authority.ca_pem);
    let server = TlsFixture::start(&server_authority, Behaviour::Answer);

    let error = OpenAiCompatibleResource::new(
        OpenAiCompatibleConfig::new(RESOURCE, server.base_url(), "fixture-model")
            .with_timeout_ms(2_000)
            .with_trust_anchors(TrustAnchors::PemFile(other_ca))
            .with_auth_env(Some(TOKEN_ENV.to_owned())),
    )
    .invoke(&request())
    .expect_err("an untrusted certificate must fail");

    let rendered = format!("{error}|{error:?}");
    assert!(
        !rendered.contains(TOKEN_VALUE),
        "the token leaked into an error"
    );
    assert!(!rendered.contains("BEGIN CERTIFICATE"));
    // The subject we minted must not be echoed back either.
    assert!(!rendered.contains("kamimusuhi test ca"));
    assert_eq!(server.connections(), 1);
}

#[test]
fn plain_http_still_works_alongside_tls() {
    // TLS support must not have quietly become a TLS requirement: a local
    // endpoint on plain HTTP is the normal case for this project.
    use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};
    let server = FixtureServer::always(FixtureResponse::ok("plain still fine")).unwrap();
    let result = resource(&server.base_url(), TrustAnchors::Webpki, 1)
        .invoke(&request())
        .expect("plain HTTP must keep working");
    assert_eq!(result.content["answer"], "plain still fine");
}
