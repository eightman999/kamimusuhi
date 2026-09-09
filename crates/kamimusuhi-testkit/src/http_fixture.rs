//! A scripted HTTP server on a real socket.
//!
//! W5 tests must not depend on an external service, but they also must not
//! prove the adapter works by stubbing out the part that does the work. So
//! this is a real `TcpListener` on loopback speaking real HTTP/1.1: the
//! adapter connects, writes bytes, and reads bytes back, and a timeout here is
//! a genuine expired deadline rather than a mocked return value.
//!
//! The server answers from a script, one entry per request, repeating the last
//! entry once the script runs out. That is what makes "attempt 1 → 500,
//! attempt 2 → 500, attempt 3 → 200" expressible as data.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// What the server should do with one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixtureResponse {
    /// Exact wire bytes, for framing and boundary regression tests.
    RawHttp { response: String },
    /// A well-formed chat completion carrying `content`.
    Ok { content: String },
    /// A non-success status with a short JSON body.
    Status { code: u16 },
    /// 200 with a body that is not JSON at all.
    NotJson,
    /// 200 with JSON that lacks the expected shape.
    WrongShape,
    /// 200 carrying the provider's own error object.
    ProviderError { code: String },
    /// Accept the connection, then say nothing for `ms` and close. The client
    /// should hit its read deadline.
    Silence { ms: u64 },
    /// Accept, wait `ms`, then answer normally. Long enough exceeds the
    /// client's timeout; short enough does not.
    Delayed { ms: u64, content: String },
    /// Close the connection without answering.
    Hangup,
}

impl FixtureResponse {
    pub fn ok(content: impl Into<String>) -> Self {
        Self::Ok {
            content: content.into(),
        }
    }

    fn render(&self) -> Option<String> {
        let body = match self {
            Self::RawHttp { response } => return Some(response.clone()),
            Self::Ok { content } | Self::Delayed { content, .. } => completion_body(content),
            Self::Status { code } => {
                let body =
                    serde_json::json!({ "error": { "message": "fixture failure" } }).to_string();
                return Some(http_response(*code, &body));
            }
            Self::NotJson => return Some(http_response(200, "<html>not json</html>")),
            Self::WrongShape => serde_json::json!({ "choices": [] }).to_string(),
            Self::ProviderError { code } => serde_json::json!({
                "error": { "code": code, "message": "fixture provider error" }
            })
            .to_string(),
            Self::Silence { .. } | Self::Hangup => return None,
        };
        Some(http_response(200, &body))
    }

    fn delay(&self) -> Duration {
        match self {
            Self::Silence { ms } | Self::Delayed { ms, .. } => Duration::from_millis(*ms),
            _ => Duration::ZERO,
        }
    }
}

fn completion_body(content: &str) -> String {
    serde_json::json!({
        "model": "fixture-model",
        "choices": [{ "message": { "role": "assistant", "content": content } }],
    })
    .to_string()
}

fn http_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        403 => "Forbidden",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Fixture",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// What one request looked like, minus anything secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    pub path: String,
    pub body: String,
    /// Present only so a test can assert the header *was* sent. Tests must not
    /// print it.
    pub authorization: Option<String>,
}

struct Shared {
    script: Mutex<Vec<FixtureResponse>>,
    requests: Mutex<Vec<RecordedRequest>>,
    served: AtomicUsize,
    stop: AtomicBool,
}

/// A running fixture server. Stops when dropped.
pub struct FixtureServer {
    port: u16,
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for FixtureServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixtureServer")
            .field("port", &self.port)
            .field("served", &self.request_count())
            .finish_non_exhaustive()
    }
}

impl FixtureServer {
    /// Start on an ephemeral loopback port, answering from `script`.
    ///
    /// The last entry repeats, so a one-entry script answers every request the
    /// same way.
    pub fn start(script: Vec<FixtureResponse>) -> std::io::Result<Self> {
        assert!(!script.is_empty(), "a fixture server needs a script");
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        // A short accept timeout lets the loop notice the stop flag without a
        // second channel.
        listener.set_nonblocking(true)?;

        let shared = Arc::new(Shared {
            script: Mutex::new(script),
            requests: Mutex::new(Vec::new()),
            served: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        });
        let worker = Arc::clone(&shared);
        let handle = std::thread::spawn(move || serve(&listener, &worker));

        Ok(Self {
            port,
            shared,
            handle: Some(handle),
        })
    }

    /// Start a server that answers every request the same way.
    pub fn always(response: FixtureResponse) -> std::io::Result<Self> {
        Self::start(vec![response])
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Base URL to configure a provider with, including the `/v1` prefix a
    /// chat-completions endpoint sits under.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    /// How many requests were actually served. This is the ground truth for
    /// "the adapter retried", because it counts bytes on a socket.
    pub fn request_count(&self) -> usize {
        self.shared.served.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.shared.requests.lock().expect("fixture lock").clone()
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Unblock the accept loop with one throwaway connection.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve(listener: &TcpListener, shared: &Arc<Shared>) {
    let mut workers = Vec::new();
    while !shared.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                // One thread per connection: a scripted delay must hold up its
                // own request, not the accept loop or the server's shutdown.
                let shared = Arc::clone(shared);
                workers.push(std::thread::spawn(move || {
                    handle_connection(stream, &shared);
                }));
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
    for worker in workers {
        let _ = worker.join();
    }
}

fn handle_connection(mut stream: TcpStream, shared: &Arc<Shared>) {
    // An accepted socket inherits the listener's non-blocking flag on some
    // platforms, and a non-blocking read returns EAGAIN instead of waiting —
    // which looks exactly like a client that sent nothing.
    stream
        .set_nonblocking(false)
        .expect("fixture blocking mode");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("fixture read timeout");
    let Some(request) = read_request(&mut stream) else {
        return;
    };

    let index = shared.served.fetch_add(1, Ordering::SeqCst);
    shared.requests.lock().expect("fixture lock").push(request);

    let response = {
        let script = shared.script.lock().expect("fixture lock");
        // The last entry repeats: a script describes a sequence, not a quota.
        script
            .get(index)
            .or_else(|| script.last())
            .cloned()
            .expect("script is non-empty")
    };

    let delay = response.delay();
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }
    if let Some(rendered) = response.render() {
        let _ = stream.write_all(rendered.as_bytes());
        let _ = stream.flush();
    }
    // Closing is the end-of-body signal the client is waiting for.
    let _ = stream.shutdown(Shutdown::Both);
}

fn read_request(stream: &mut TcpStream) -> Option<RecordedRequest> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 1024];
    let mut header_end = None;

    while header_end.is_none() {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..read]);
        header_end = raw.windows(4).position(|w| w == b"\r\n\r\n");
    }
    let split = header_end?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();

    let mut lines = head.split("\r\n");
    let path = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or_default()
        .to_owned();

    let header = |name: &str| -> Option<String> {
        head.split("\r\n").find_map(|line| {
            line.split_once(':').and_then(|(key, value)| {
                key.eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_owned())
            })
        })
    };
    let content_length: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let mut body = raw[split + 4..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }

    Some(RecordedRequest {
        path,
        body: String::from_utf8_lossy(&body).into_owned(),
        authorization: header("authorization"),
    })
}

/// A loopback port that reliably refuses.
///
/// Deliberately not an ephemeral port that was bound and released: the OS
/// hands those out again, so a parallel test starting its own server can be
/// given the same number and the "connection refused" case quietly becomes a
/// successful call. Port 1 needs privileges to bind, so no test can take it.
pub const REFUSED_PORT: u16 = 1;

/// Base URL for a provider that cannot be reached.
pub fn refused_base_url() -> String {
    format!("http://127.0.0.1:{REFUSED_PORT}/v1")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port: u16, body: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(
                format!(
                    "POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn the_server_answers_over_a_real_socket() {
        let server = FixtureServer::always(FixtureResponse::ok("hello")).unwrap();
        let response = get(server.port(), "{}");
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("hello"));
        assert_eq!(server.request_count(), 1);
    }

    #[test]
    fn a_script_is_consumed_in_order_and_its_last_entry_repeats() {
        let server = FixtureServer::start(vec![
            FixtureResponse::Status { code: 500 },
            FixtureResponse::ok("second"),
        ])
        .unwrap();
        assert!(get(server.port(), "{}").starts_with("HTTP/1.1 500"));
        assert!(get(server.port(), "{}").contains("second"));
        assert!(get(server.port(), "{}").contains("second"));
        assert_eq!(server.request_count(), 3);
    }

    #[test]
    fn requests_are_recorded_with_their_path_and_body() {
        let server = FixtureServer::always(FixtureResponse::ok("x")).unwrap();
        get(server.port(), "{\"model\":\"m\"}");
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/chat/completions");
        assert_eq!(requests[0].body, "{\"model\":\"m\"}");
        assert_eq!(requests[0].authorization, None);
    }

    #[test]
    fn the_refused_port_refuses() {
        assert!(TcpStream::connect(("127.0.0.1", REFUSED_PORT)).is_err());
        assert!(refused_base_url().contains(&REFUSED_PORT.to_string()));
    }
}
