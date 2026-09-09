//! A small blocking HTTP/1.1 client.
//!
//! Hand-written rather than pulled in, for two reasons. The contract this
//! adapter implements is synchronous, so an async client would need a runtime
//! the rest of the system does not have; and the failure modes W5 has to
//! classify — connect refused, header timeout, body timeout, truncated body —
//! are exactly the ones a wrapper would flatten into one error type.
//!
//! `https://` is supported through [`crate::tls`], which delegates the
//! handshake and every certificate and hostname check to `rustls`. Nothing in
//! this crate implements TLS.
//!
//! Deliberate limits, because pretending otherwise would be worse than saying
//! so: HTTP/1.1, `Content-Length` or chunked responses, no keep-alive reuse —
//! one connection per attempt, closed after.
//!
//! Socket operations are bounded by one attempt deadline. OS DNS resolution
//! is synchronous and cannot be interrupted by this deadline. Responses are
//! buffered with explicit wire/header limits and must have complete framing;
//! an EOF alone is not proof that a response is complete.

use std::fmt;
use std::io::{Read, Write};
use std::net::{Ipv6Addr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::tls::{TlsFailureKind, Transport, TrustAnchors};

/// Where a request is going. Parsed once from a base URL so a malformed
/// endpoint fails at configuration time, not on the first call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// Path with a leading slash, query included.
    pub path: String,
    /// Whether this endpoint is reached over TLS. Derived from the scheme, so
    /// an `http://` URL can never be silently upgraded, nor an `https://` one
    /// silently downgraded.
    pub tls: bool,
}

impl Endpoint {
    /// Parse the supported URL subset. Credentials, queries, fragments and
    /// control characters are rejected without reflecting the URL in errors.
    pub fn parse(base_url: &str, suffix: &str) -> Result<Self, String> {
        if !safe_url_text(base_url) || !safe_path(suffix) {
            return Err("endpoint contains unsupported URL syntax".to_owned());
        }
        let (rest, tls) = match base_url.strip_prefix("https://") {
            Some(rest) => (rest, true),
            None => (
                base_url
                    .strip_prefix("http://")
                    .ok_or_else(|| "base URL must start with http:// or https://".to_owned())?,
                false,
            ),
        };
        let (authority, base_path) = match rest.find('/') {
            Some(index) => (&rest[..index], rest[index..].trim_end_matches('/')),
            None => (rest, ""),
        };
        let default_port = if tls { 443 } else { 80 };
        let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
            let (host, tail) = bracketed
                .split_once(']')
                .ok_or_else(|| "IPv6 host is not bracketed correctly".to_owned())?;
            host.parse::<Ipv6Addr>()
                .map_err(|_| "IPv6 host is invalid".to_owned())?;
            let port = if tail.is_empty() {
                default_port
            } else {
                parse_port(
                    tail.strip_prefix(':')
                        .ok_or_else(|| "unexpected text after IPv6 host".to_owned())?,
                )?
            };
            (host, port)
        } else {
            match authority.split_once(':') {
                Some((host, port)) => (host, parse_port(port)?),
                None => (authority, default_port),
            }
        };
        let endpoint = Self {
            host: host.to_owned(),
            port,
            path: format!("{base_path}{suffix}"),
            tls,
        };
        endpoint.validate()?;
        Ok(endpoint)
    }

    /// Fields are public for compatibility, so dispatch validates them again.
    fn validate(&self) -> Result<(), String> {
        let valid_host = !self.host.is_empty()
            && (self.host.parse::<Ipv6Addr>().is_ok()
                || self
                    .host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b)));
        if !valid_host || self.port == 0 || !safe_path(&self.path) {
            return Err("endpoint host, port or path is invalid".to_owned());
        }
        Ok(())
    }

    fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

fn parse_port(port: &str) -> Result<u16, String> {
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err("endpoint port is invalid".to_owned());
    }
    port.parse::<u16>()
        .map_err(|_| "endpoint port is invalid".to_owned())
}

fn safe_url_text(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_graphic() && !b"@?#\\".contains(&b))
}

fn safe_path(value: &str) -> bool {
    value.starts_with('/') && safe_url_text(value)
}

fn token_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn safe_header_value(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b == b'\t' || (b' '..=b'~').contains(&b))
}

/// What went wrong at the transport layer.
///
/// Carries no response body: the adapter above decides what, if anything, is
/// worth reporting, and a body may contain the user's own prompt echoed back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// Rejected before opening a socket. Never contains the supplied value.
    InvalidRequest(String),
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
    /// TLS could not be established. Classified by `rustls`, not by us.
    Tls {
        kind: TlsFailureKind,
        /// `rustls`'s own message. No certificate content is added.
        detail: String,
    },
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
#[derive(Clone)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Header")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// Includes status line, headers, framing and body, not just decoded content.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// Bounds the main response headers and, independently, chunked trailers.
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

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
    anchors: &TrustAnchors,
) -> Result<HttpResponse, HttpError> {
    endpoint.validate().map_err(HttpError::InvalidRequest)?;
    for header in headers {
        if !token_name(&header.name)
            || !safe_header_value(&header.value)
            || [
                "host",
                "content-length",
                "transfer-encoding",
                "connection",
                "content-type",
            ]
            .iter()
            .any(|name| header.name.eq_ignore_ascii_case(name))
        {
            return Err(HttpError::InvalidRequest(
                "invalid or reserved request header".to_owned(),
            ));
        }
    }
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

    remaining("resolve")?;
    let addresses: Vec<_> = endpoint
        .authority()
        .to_socket_addrs()
        .map_err(|source| HttpError::Transport(format!("resolve {}: {source}", endpoint.host)))?
        .collect();
    if addresses.is_empty() {
        return Err(HttpError::Transport(format!(
            "{} resolved to nothing",
            endpoint.host
        )));
    }

    // Try every resolved address, not just the first. A dual-stack name
    // routinely resolves to an AAAA the local host cannot reach and an A it
    // can; taking the first would make reachability depend on resolver order.
    let mut last = None;
    let mut connected = None;
    for address in &addresses {
        match TcpStream::connect_timeout(address, remaining("connect")?) {
            Ok(socket) => {
                connected = Some(socket);
                break;
            }
            Err(source) => last = Some(classify_io(&source, started, "connect")),
        }
    }
    let socket = match connected {
        Some(socket) => socket,
        None => {
            return Err(last.unwrap_or_else(|| {
                HttpError::Transport(format!("{} could not be reached", endpoint.host))
            }));
        }
    };
    socket
        .set_nodelay(true)
        .map_err(|source| HttpError::Transport(format!("set_nodelay: {source}")))?;
    // Deadlines are armed before the handshake: a TLS server that accepts a
    // connection and then stalls has to time out like any other.
    socket
        .set_write_timeout(Some(remaining("handshake")?))
        .map_err(|source| HttpError::Transport(format!("set_write_timeout: {source}")))?;
    socket
        .set_read_timeout(Some(remaining("handshake")?))
        .map_err(|source| HttpError::Transport(format!("set_read_timeout: {source}")))?;

    let mut stream = if endpoint.tls {
        let config = crate::tls::client_config(anchors)
            .map_err(|(kind, detail)| HttpError::Tls { kind, detail })?;
        match crate::tls::connect(config, &endpoint.host, socket) {
            Ok(tls) => Transport::Tls(Box::new(tls)),
            Err((kind, detail)) => {
                // A stalled handshake surfaces from rustls as an IO error;
                // report it as the timeout it is rather than as a TLS fault.
                if remaining("handshake").is_err() {
                    return Err(HttpError::Timeout {
                        elapsed_ms: elapsed_ms(started),
                        phase: "handshake",
                    });
                }
                return Err(HttpError::Tls { kind, detail });
            }
        }
    } else {
        Transport::Plain(socket)
    };

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
        .socket()
        .set_write_timeout(Some(remaining("write")?))
        .map_err(|source| HttpError::Transport(format!("set_write_timeout: {source}")))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|source| classify_io(&source, started, "write"))?;
    stream
        .flush()
        .map_err(|source| classify_io(&source, started, "write"))?;

    // Connection: close is requested. EOF ends transport reading, but only
    // framing validation below can establish that the message is complete.
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut headers_complete = false;
    loop {
        stream
            .socket()
            .set_read_timeout(Some(remaining("read")?))
            .map_err(|source| HttpError::Transport(format!("set_read_timeout: {source}")))?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                if read > MAX_RESPONSE_BYTES.saturating_sub(raw.len()) {
                    return Err(malformed("response exceeds wire byte limit"));
                }
                raw.extend_from_slice(&chunk[..read]);
                if !headers_complete {
                    match raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        Some(end) if end + 4 <= MAX_HEADER_BYTES => headers_complete = true,
                        Some(_) => return Err(malformed("response headers exceed byte limit")),
                        None if raw.len() >= MAX_HEADER_BYTES => {
                            return Err(malformed("response headers exceed byte limit"));
                        }
                        None => {}
                    }
                }
            }
            // A TLS peer that closes without `close_notify` is common enough
            // in the wild that refusing to read such a response would fail
            // against real servers. The cost is that an unclean EOF cannot be
            // told from a clean one here — a truncated body therefore surfaces
            // as a malformed response when it fails to parse, rather than
            // being quietly accepted as complete.
            Err(ref source) if source.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(source) => return Err(classify_io(&source, started, "read")),
        }
    }

    parse_response(&raw)
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// A read/write that expired is a timeout; a wrapped `rustls` error is a TLS
/// failure; anything else is transport.
fn classify_io(source: &std::io::Error, started: Instant, phase: &'static str) -> HttpError {
    match source.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => HttpError::Timeout {
            elapsed_ms: elapsed_ms(started),
            phase,
        },
        _ => match crate::tls::classify_io(source) {
            Some((kind, detail)) => HttpError::Tls { kind, detail },
            None => HttpError::Transport(format!("{phase}: {source}")),
        },
    }
}

fn malformed(detail: &str) -> HttpError {
    HttpError::Malformed(detail.to_owned())
}

fn parse_response(raw: &[u8]) -> Result<HttpResponse, HttpError> {
    if raw.len() > MAX_RESPONSE_BYTES {
        return Err(malformed("response exceeds wire byte limit"));
    }
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| malformed("no header terminator in response"))?;
    if split + 4 > MAX_HEADER_BYTES {
        return Err(malformed("response headers exceed byte limit"));
    }
    let head = std::str::from_utf8(&raw[..split])
        .map_err(|_| malformed("response headers are not UTF-8"))?;
    let body_bytes = &raw[split + 4..];
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or_else(|| malformed("empty response"))?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().ok_or_else(|| malformed("no HTTP version"))?;
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err(malformed("unsupported response version"));
    }
    let status_text = parts.next().ok_or_else(|| malformed("no status code"))?;
    if status_text.len() != 3 || !status_text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed("invalid status code"));
    }
    let status: u16 = status_text
        .parse()
        .map_err(|_| malformed("invalid status code"))?;
    if !(100..=599).contains(&status)
        || parts
            .next()
            .is_some_and(|reason| !safe_header_value(reason))
    {
        return Err(malformed("invalid status line"));
    }
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        let (name, value) = response_header(line)?;
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(malformed("invalid or duplicate content length"));
            }
            length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| malformed("invalid content length"))?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if chunked || !value.eq_ignore_ascii_case("chunked") {
                return Err(malformed("unsupported or duplicate transfer encoding"));
            }
            chunked = true;
        }
    }
    let body = match (length, chunked) {
        (Some(_), true) => return Err(malformed("ambiguous response framing")),
        (Some(length), false) => {
            if body_bytes.len() != length {
                return Err(malformed("response length does not match content length"));
            }
            String::from_utf8(body_bytes.to_vec())
                .map_err(|_| malformed("response body is not UTF-8"))?
        }
        (None, true) => decode_chunked(body_bytes)?,
        (None, false) => {
            return Err(malformed(
                "response needs explicit content length or chunked framing",
            ));
        }
    };
    Ok(HttpResponse { status, body })
}

fn response_header(line: &str) -> Result<(&str, &str), HttpError> {
    let (name, value) = line
        .split_once(':')
        .ok_or_else(|| malformed("invalid response header"))?;
    if !token_name(name) || !safe_header_value(value) {
        return Err(malformed("invalid response header"));
    }
    Ok((name, value.trim_matches([' ', '\t'])))
}

fn decode_chunked(mut body: &[u8]) -> Result<String, HttpError> {
    let mut decoded = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| malformed("truncated chunk header"))?;
        if line_end > MAX_HEADER_BYTES {
            return Err(malformed("chunk header exceeds byte limit"));
        }
        let header = std::str::from_utf8(&body[..line_end])
            .map_err(|_| malformed("chunk header is not UTF-8"))?;
        let size_text = header.split(';').next().unwrap_or("");
        if size_text.is_empty()
            || !size_text.bytes().all(|b| b.is_ascii_hexdigit())
            || !safe_header_value(header)
        {
            return Err(malformed("invalid chunk size"));
        }
        let size =
            usize::from_str_radix(size_text, 16).map_err(|_| malformed("invalid chunk size"))?;
        body = &body[line_end + 2..];
        if size == 0 {
            if body.len() > MAX_HEADER_BYTES {
                return Err(malformed("chunk trailers exceed byte limit"));
            }
            loop {
                let end = body
                    .windows(2)
                    .position(|w| w == b"\r\n")
                    .ok_or_else(|| malformed("truncated chunk trailer"))?;
                if end == 0 {
                    if body.len() != 2 {
                        return Err(malformed("unexpected bytes after chunked response"));
                    }
                    return String::from_utf8(decoded)
                        .map_err(|_| malformed("response body is not UTF-8"));
                }
                let line = std::str::from_utf8(&body[..end])
                    .map_err(|_| malformed("chunk trailer is not UTF-8"))?;
                let (name, _) = response_header(line)?;
                if name.eq_ignore_ascii_case("content-length")
                    || name.eq_ignore_ascii_case("transfer-encoding")
                {
                    return Err(malformed("framing field in chunk trailer"));
                }
                body = &body[end + 2..];
            }
        }
        let framed_size = size
            .checked_add(2)
            .ok_or_else(|| malformed("chunk size overflow"))?;
        if body.len() < framed_size || &body[size..framed_size] != b"\r\n" {
            return Err(malformed("truncated chunk body or missing terminator"));
        }
        if size > MAX_RESPONSE_BYTES.saturating_sub(decoded.len()) {
            return Err(malformed("decoded body exceeds byte limit"));
        }
        decoded.extend_from_slice(&body[..size]);
        body = &body[framed_size..];
    }
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
    fn https_is_parsed_with_its_own_default_port() {
        let endpoint = Endpoint::parse("https://api.example.test/v1", "/chat").unwrap();
        assert!(endpoint.tls);
        assert_eq!(endpoint.port, 443);
        assert_eq!(endpoint.host, "api.example.test");

        // A scheme is never silently changed in either direction.
        assert!(
            !Endpoint::parse("http://api.example.test/v1", "/chat")
                .unwrap()
                .tls
        );
        assert_eq!(
            Endpoint::parse("https://api.example.test:8443/v1", "/chat")
                .unwrap()
                .port,
            8443
        );
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
        let error = post_json(
            &endpoint,
            "{}",
            &[],
            Duration::ZERO,
            &TrustAnchors::default(),
        )
        .unwrap_err();
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
        let error = post_json(
            &endpoint,
            "{}",
            &[],
            Duration::from_millis(500),
            &TrustAnchors::default(),
        )
        .unwrap_err();
        assert!(
            matches!(error, HttpError::Transport(_)),
            "expected transport, got {error:?}"
        );
    }

    #[test]
    fn framing_rejects_truncation_even_when_the_body_is_valid_json() {
        for raw in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\n{}"[..],
            &b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n{}"[..],
            &b"HTTP/1.1 200 OK\r\n\r\n{}"[..],
            &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}"[..],
            &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"
                [..],
            &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n0\r\n\r\n"[..],
        ] {
            assert!(matches!(parse_response(raw), Err(HttpError::Malformed(_))));
        }
    }

    #[test]
    fn chunks_need_complete_terminators_and_cannot_overflow() {
        for body in [
            "2\r\n{}XX0\r\n\r\n",
            "2\r\n{}\r\n0\r\n",
            "0\r\n\r\nextra",
            "ffffffffffffffff\r\n",
            "fffffffffffffffff\r\n",
            "0\r\nContent-Length: 9\r\n\r\n",
        ] {
            assert!(matches!(
                decode_chunked(body.as_bytes()),
                Err(HttpError::Malformed(_))
            ));
        }
    }

    #[test]
    fn complete_chunks_allow_extensions_and_nonframing_trailers() {
        assert_eq!(
            decode_chunked(b"2;fixture=yes\r\n{}\r\n0\r\nX-Receipt: yes\r\n\r\n").unwrap(),
            "{}"
        );
    }

    #[test]
    fn invalid_utf8_is_not_silently_replaced() {
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n\xff").is_err());
        assert!(decode_chunked(b"1\r\n\xff\r\n0\r\n\r\n").is_err());
    }

    #[test]
    fn response_and_header_sizes_are_bounded() {
        assert!(parse_response(&vec![b'x'; MAX_RESPONSE_BYTES + 1]).is_err());
        let raw = format!(
            "HTTP/1.1 200 OK\r\nX-Padding: {}\r\nContent-Length: 2\r\n\r\n{{}}",
            "x".repeat(MAX_HEADER_BYTES)
        );
        assert!(parse_response(raw.as_bytes()).is_err());
    }

    #[test]
    fn errors_do_not_reflect_provider_controlled_headers() {
        let secret = "PRIVATE_PROMPT_OR_TOKEN";
        for raw in [
            format!("{secret} 200 OK\r\nContent-Length: 2\r\n\r\n{{}}"),
            format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{secret}\r\n"),
            format!("HTTP/1.1 200 OK\r\nContent-Length: {secret}\r\n\r\n{{}}"),
        ] {
            assert!(!format!("{:?}", parse_response(raw.as_bytes()).unwrap_err()).contains(secret));
        }
    }

    #[test]
    fn endpoint_validation_never_echoes_credentials_or_allows_injection() {
        for url in [
            "http://user:PRIVATE_TOKEN@example.test/v1",
            "http://example.test/v1?token=PRIVATE_TOKEN",
            "http://example.test/v1#PRIVATE_TOKEN",
            "http://example.test/\r\nPRIVATE_TOKEN",
            "http://example.test:0/v1",
            "http://[::1]suffix/v1",
            "http://::1/v1",
        ] {
            let error = Endpoint::parse(url, "/chat").unwrap_err();
            assert!(!error.contains("PRIVATE_TOKEN"));
        }
        assert!(Endpoint::parse("http://example.test", "/chat\r\nX: value").is_err());
    }

    #[test]
    fn bracketed_ipv6_has_a_valid_authority_and_unbracketed_tls_name() {
        let endpoint = Endpoint::parse("http://[::1]:8080/v1", "/chat").unwrap();
        assert_eq!(endpoint.host, "::1");
        assert_eq!(endpoint.authority(), "[::1]:8080");
        assert_eq!(
            Endpoint::parse("https://[::1]/v1", "/chat").unwrap().port,
            443
        );
    }

    #[test]
    fn invalid_request_headers_are_rejected_before_connecting() {
        let endpoint = Endpoint::parse("http://127.0.0.1:1", "/chat").unwrap();
        for header in [
            Header {
                name: "Authorization".to_owned(),
                value: "Bearer PRIVATE_TOKEN\r\nInjected: yes".to_owned(),
            },
            Header {
                name: "Content-Length".to_owned(),
                value: "100".to_owned(),
            },
            Header {
                name: "X\r\nInjected".to_owned(),
                value: "value".to_owned(),
            },
        ] {
            let error = post_json(
                &endpoint,
                "{}",
                &[header],
                Duration::from_millis(100),
                &TrustAnchors::default(),
            )
            .unwrap_err();
            assert!(matches!(error, HttpError::InvalidRequest(_)));
            assert!(!format!("{error:?}").contains("PRIVATE_TOKEN"));
        }
    }

    #[test]
    fn public_endpoint_fields_cannot_bypass_validation() {
        let endpoint = Endpoint {
            host: "127.0.0.1".to_owned(),
            port: 1,
            path: "/x\r\nPRIVATE_TOKEN".to_owned(),
            tls: false,
        };
        assert!(matches!(
            post_json(
                &endpoint,
                "{}",
                &[],
                Duration::from_millis(100),
                &TrustAnchors::default()
            ),
            Err(HttpError::InvalidRequest(_))
        ));
    }

    #[test]
    fn header_debug_redacts_values() {
        let header = Header {
            name: "Authorization".to_owned(),
            value: "Bearer PRIVATE_TOKEN".to_owned(),
        };
        assert!(!format!("{header:?}").contains("PRIVATE_TOKEN"));
        assert!(format!("{header:?}").contains("<redacted>"));
    }
}
