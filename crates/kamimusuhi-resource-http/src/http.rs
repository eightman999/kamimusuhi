//! A small blocking HTTP/1.1 client.
//!
//! Hand-written rather than pulled in, for two reasons. The contract this
//! adapter implements is synchronous, so an async client would need a runtime
//! the rest of the system does not have; and the failure modes W5 has to
//! classify — connect refused, header timeout, body timeout, truncated body —
//! are exactly the ones a wrapper would flatten into one error type.
//!
//! Deliberate limits, because pretending otherwise would be worse than saying
//! so:
//!
//! - **Plain HTTP only.** There is no TLS here, so this reaches local and
//!   in-cluster endpoints (llama.cpp, Ollama, LM Studio, a fixture server) and
//!   not `https://` providers. TLS is a real gap and is noted as such rather
//!   than half-implemented.
//! - HTTP/1.1, `Content-Length` or `identity` chunked responses, no keep-alive
//!   reuse: one connection per attempt, closed after.
//!
//! Every read and write is bounded by a deadline derived from one overall
//! timeout, so a server that accepts a connection and then says nothing is a
//! timeout rather than a hang.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Where a request is going. Parsed once from a base URL so a malformed
/// endpoint fails at configuration time, not on the first call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// Path with a leading slash, query included.
    pub path: String,
}

impl Endpoint {
    /// Parse `http://host[:port][/path]`, appending `suffix` to the path.
    pub fn parse(base_url: &str, suffix: &str) -> Result<Self, String> {
        let rest = base_url.strip_prefix("http://").ok_or_else(|| {
            match base_url.strip_prefix("https://") {
                // Say the actual reason rather than "invalid URL".
                Some(_) => {
                    "https is not supported by this adapter (no TLS); use http://".to_owned()
                }
                None => format!("base url {base_url:?} must start with http://"),
            }
        })?;
        let (authority, base_path) = match rest.find('/') {
            Some(index) => (&rest[..index], rest[index..].trim_end_matches('/')),
            None => (rest, ""),
        };
        if authority.is_empty() {
            return Err(format!("base url {base_url:?} has no host"));
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (
                host,
                port.parse::<u16>()
                    .map_err(|_| format!("base url {base_url:?} has a non-numeric port"))?,
            ),
            None => (authority, 80),
        };
        if host.is_empty() {
            return Err(format!("base url {base_url:?} has no host"));
        }
        Ok(Self {
            host: host.to_owned(),
            port,
            path: format!("{base_path}{suffix}"),
        })
    }

    fn authority(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// What went wrong at the transport layer.
///
/// Carries no response body: the adapter above decides what, if anything, is
/// worth reporting, and a body may contain the user's own prompt echoed back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// The deadline passed. `elapsed_ms` is measured, not assumed.
    Timeout {
        elapsed_ms: u64,
        phase: &'static str,
    },
    /// Could not resolve, connect, write or read. Not a timeout: something
    /// broke rather than took too long.
    Transport(String),
    /// A reply arrived but is not HTTP the client can parse.
    Malformed(String),
}

/// One HTTP response, already read into memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    pub const fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }
}

/// One header to send. Values are never logged by this module.
#[derive(Debug, Clone)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// Smallest socket timeout worth setting.
///
/// A `SO_RCVTIMEO` that rounds to zero means *no timeout* on POSIX, and some
/// platforms reject a sub-millisecond value outright. Either way, handing the
/// kernel an almost-expired deadline turns a timeout into an unbounded wait —
/// so a deadline with less than this left is already expired.
const MIN_SOCKET_TIMEOUT: Duration = Duration::from_millis(1);

/// POST a JSON body and read the response, all within `timeout`.
pub fn post_json(
    endpoint: &Endpoint,
    body: &str,
    headers: &[Header],
    timeout: Duration,
) -> Result<HttpResponse, HttpError> {
    let started = Instant::now();
    let remaining = |phase: &'static str| -> Result<Duration, HttpError> {
        let left = timeout
            .checked_sub(started.elapsed())
            .unwrap_or(Duration::ZERO);
        if left < MIN_SOCKET_TIMEOUT {
            return Err(HttpError::Timeout {
                elapsed_ms: elapsed_ms(started),
                phase,
            });
        }
        Ok(left)
    };

    let address = endpoint
        .authority()
        .to_socket_addrs()
        .map_err(|source| HttpError::Transport(format!("resolve {}: {source}", endpoint.host)))?
        .next()
        .ok_or_else(|| HttpError::Transport(format!("{} resolved to nothing", endpoint.host)))?;

    let mut stream = TcpStream::connect_timeout(&address, remaining("connect")?)
        .map_err(|source| classify_io(&source, started, "connect"))?;
    stream
        .set_nodelay(true)
        .map_err(|source| HttpError::Transport(format!("set_nodelay: {source}")))?;

    let mut request = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n",
        endpoint.path,
        endpoint.authority(),
        body.len()
    );
    for header in headers {
        request.push_str(&format!("{}: {}\r\n", header.name, header.value));
    }
    request.push_str("\r\n");
    request.push_str(body);

    stream
        .set_write_timeout(Some(remaining("write")?))
        .map_err(|source| HttpError::Transport(format!("set_write_timeout: {source}")))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|source| classify_io(&source, started, "write"))?;
    stream
        .flush()
        .map_err(|source| classify_io(&source, started, "write"))?;

    // Read to EOF: `Connection: close` means the server closes when done, so
    // this doubles as the end-of-body signal for both framings.
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        stream
            .set_read_timeout(Some(remaining("read")?))
            .map_err(|source| HttpError::Transport(format!("set_read_timeout: {source}")))?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&chunk[..read]),
            Err(source) => return Err(classify_io(&source, started, "read")),
        }
    }

    parse_response(&raw)
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// A read/write that expired is a timeout; anything else is transport.
fn classify_io(source: &std::io::Error, started: Instant, phase: &'static str) -> HttpError {
    match source.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => HttpError::Timeout {
            elapsed_ms: elapsed_ms(started),
            phase,
        },
        _ => HttpError::Transport(format!("{phase}: {source}")),
    }
}

fn parse_response(raw: &[u8]) -> Result<HttpResponse, HttpError> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| HttpError::Malformed("no header terminator in response".to_owned()))?;
    let head = std::str::from_utf8(&raw[..split])
        .map_err(|_| HttpError::Malformed("response headers are not UTF-8".to_owned()))?;
    let body_bytes = &raw[split + 4..];

    let mut lines = head.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| HttpError::Malformed("empty response".to_owned()))?;
    let mut parts = status_line.split(' ');
    let version = parts
        .next()
        .ok_or_else(|| HttpError::Malformed("no HTTP version".to_owned()))?;
    if !version.starts_with("HTTP/1.") {
        return Err(HttpError::Malformed(format!(
            "unsupported response version {version:?}"
        )));
    }
    let status: u16 = parts
        .next()
        .ok_or_else(|| HttpError::Malformed("no status code".to_owned()))?
        .parse()
        .map_err(|_| HttpError::Malformed("status code is not a number".to_owned()))?;

    let chunked = lines.any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
        })
    });
    let body = if chunked {
        decode_chunked(body_bytes)?
    } else {
        String::from_utf8_lossy(body_bytes).into_owned()
    };
    Ok(HttpResponse { status, body })
}

fn decode_chunked(mut body: &[u8]) -> Result<String, HttpError> {
    let mut decoded = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| HttpError::Malformed("truncated chunk header".to_owned()))?;
        let header = std::str::from_utf8(&body[..line_end])
            .map_err(|_| HttpError::Malformed("chunk header is not UTF-8".to_owned()))?;
        let size = usize::from_str_radix(header.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| HttpError::Malformed(format!("bad chunk size {header:?}")))?;
        body = &body[line_end + 2..];
        if size == 0 {
            break;
        }
        if body.len() < size {
            return Err(HttpError::Malformed("truncated chunk body".to_owned()));
        }
        decoded.extend_from_slice(&body[..size]);
        body = body.get(size + 2..).unwrap_or(&[]);
    }
    Ok(String::from_utf8_lossy(&decoded).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_parsing_keeps_host_port_and_path() {
        let endpoint = Endpoint::parse("http://127.0.0.1:8080/v1", "/chat/completions").unwrap();
        assert_eq!(endpoint.host, "127.0.0.1");
        assert_eq!(endpoint.port, 8080);
        assert_eq!(endpoint.path, "/v1/chat/completions");

        let default_port = Endpoint::parse("http://example.test", "/v1/x").unwrap();
        assert_eq!(default_port.port, 80);
        assert_eq!(default_port.path, "/v1/x");

        // A trailing slash on the base must not double up.
        let trailing = Endpoint::parse("http://example.test/v1/", "/chat").unwrap();
        assert_eq!(trailing.path, "/v1/chat");
    }

    #[test]
    fn https_is_refused_with_the_actual_reason() {
        let error = Endpoint::parse("https://api.example.test/v1", "/chat").unwrap_err();
        assert!(error.contains("TLS"), "{error}");
    }

    #[test]
    fn malformed_base_urls_are_refused() {
        assert!(Endpoint::parse("ftp://example.test", "/x").is_err());
        assert!(Endpoint::parse("http://", "/x").is_err());
        assert!(Endpoint::parse("http://example.test:notaport", "/x").is_err());
    }

    #[test]
    fn responses_parse_with_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n{\"a\":1}";
        let response = parse_response(raw).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "{\"a\":1}");
        assert!(response.is_success());
    }

    #[test]
    fn responses_parse_when_chunked() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n{\"a\r\n4\r\n\":1}\r\n0\r\n\r\n";
        let response = parse_response(raw).unwrap();
        assert_eq!(response.body, "{\"a\":1}");
    }

    #[test]
    fn a_non_success_status_is_parsed_not_rejected() {
        let raw = b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 2\r\n\r\n{}";
        let response = parse_response(raw).unwrap();
        assert_eq!(response.status, 429);
        assert!(!response.is_success());
    }

    #[test]
    fn garbage_is_malformed_rather_than_a_panic() {
        assert!(matches!(
            parse_response(b"not http at all"),
            Err(HttpError::Malformed(_))
        ));
        assert!(matches!(
            parse_response(b"HTTP/1.1 notanumber OK\r\n\r\n"),
            Err(HttpError::Malformed(_))
        ));
        assert!(matches!(
            parse_response(b"GARBAGE/9 200 OK\r\n\r\n"),
            Err(HttpError::Malformed(_))
        ));
    }

    #[test]
    fn an_expired_deadline_is_a_timeout_rather_than_an_unbounded_wait() {
        // Zero left must never reach the kernel as "no timeout".
        let endpoint = Endpoint::parse("http://127.0.0.1:1/v1", "/chat").unwrap();
        let error = post_json(&endpoint, "{}", &[], Duration::ZERO).unwrap_err();
        assert!(
            matches!(error, HttpError::Timeout { .. }),
            "expected a timeout, got {error:?}"
        );
    }

    #[test]
    fn connecting_to_a_closed_port_is_transport_not_timeout() {
        // Port 1 on loopback refuses fast; the distinction matters because a
        // refusal is not worth waiting out and a timeout is.
        let endpoint = Endpoint::parse("http://127.0.0.1:1/v1", "/chat").unwrap();
        let error = post_json(&endpoint, "{}", &[], Duration::from_millis(500)).unwrap_err();
        assert!(
            matches!(error, HttpError::Transport(_)),
            "expected transport, got {error:?}"
        );
    }
}
