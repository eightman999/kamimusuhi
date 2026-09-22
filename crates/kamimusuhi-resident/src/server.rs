//! Minimal HTTP/1.1 server (one thread per connection, `Connection: close`).
//!
//! Endpoints:
//! * `GET /health` — unauthenticated summary for peers and monitors.
//! * `GET /status` — full status (token unless loopback).
//! * `GET /v1/models`, `POST /v1/chat/completions` — routing proxy
//!   (token unless loopback). `X-Kamimusuhi-Route: local` restricts routing
//!   to node-local tiers so the calling peer keeps its own fallback.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use crate::probes::ROUTE_HEADER;
use crate::router::{self, AUTO_MODELS, RouteRequest};
use crate::state::Shared;
use crate::util::secret_eq;

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 64;

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub fn serve(shared: &Arc<Shared>) -> std::io::Result<()> {
    let listener = TcpListener::bind(&shared.config.node.listen)?;
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            let _ = respond(stream, 503, "application/json", br#"{"error":"busy"}"#, &[]);
            continue;
        }
        active.fetch_add(1, Ordering::SeqCst);
        let shared = Arc::clone(shared);
        let active = Arc::clone(&active);
        let _ = thread::Builder::new()
            .name("http".to_owned())
            .spawn(move || {
                handle(&shared, stream);
                active.fetch_sub(1, Ordering::SeqCst);
            });
    }
    Ok(())
}

fn read_request(stream: &TcpStream) -> Result<Request, (u16, &'static str)> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|_| (500, "clone"))?);
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = reader
            .read_until(b'\n', &mut line)
            .map_err(|_| (400, "read error"))?;
        if n == 0 {
            return Err((400, "unexpected eof"));
        }
        head.extend_from_slice(&line);
        if head.len() > MAX_HEADER_BYTES {
            return Err((431, "headers too large"));
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    let text = String::from_utf8(head).map_err(|_| (400, "non-utf8 header"))?;
    let mut lines = text.lines();
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default().to_owned();
    let path = first.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .map_or(Ok(0), |(_, v)| v.parse::<usize>())
        .map_err(|_| (400, "bad content-length"))?;
    if length > MAX_BODY_BYTES {
        return Err((413, "body too large"));
    }
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|_| (400, "short body"))?;
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}

fn respond(
    mut stream: TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra: &[(&str, String)],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn json_response(stream: TcpStream, status: u16, value: &Value, extra: &[(&str, String)]) {
    let body = serde_json::to_vec(value).unwrap_or_default();
    let _ = respond(stream, status, "application/json", &body, extra);
}

fn authorized(shared: &Shared, stream: &TcpStream, request: &Request) -> bool {
    if stream.peer_addr().is_ok_and(|a| a.ip().is_loopback()) {
        return true;
    }
    let Some(token) = &shared.token else {
        // No token configured: only loopback may use privileged endpoints.
        return false;
    };
    request
        .header("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|given| secret_eq(given.trim().as_bytes(), token.as_bytes()))
}

fn token_presented(shared: &Shared, request: &Request) -> bool {
    shared.token.as_ref().is_some_and(|token| {
        request
            .header("authorization")
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|given| secret_eq(given.trim().as_bytes(), token.as_bytes()))
    })
}

fn handle(shared: &Shared, stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let request = match read_request(&stream) {
        Ok(r) => r,
        Err((status, message)) => {
            json_response(stream, status, &json!({"error": message}), &[]);
            return;
        }
    };
    let path = request.path.split('?').next().unwrap_or_default();
    match (request.method.as_str(), path) {
        ("GET", "/health") => json_response(stream, 200, &shared.health(), &[]),
        _ if !authorized(shared, &stream, &request) => json_response(
            stream,
            401,
            &json!({"error": {"message": "bearer token required", "type": "unauthorized"}}),
            &[],
        ),
        ("GET", "/status") => json_response(stream, 200, &shared.status(), &[]),
        ("GET", "/v1/tasks") => {
            let (status, reply) =
                crate::tasks::handle(shared, &json!({"action": "list"}), "operator");
            json_response(stream, status, &reply, &[]);
        }
        ("POST", "/v1/tasks") => {
            let body = serde_json::from_slice::<Value>(&request.body).unwrap_or(Value::Null);
            let (status, reply) = crate::tasks::handle(shared, &body, "operator");
            json_response(stream, status, &reply, &[]);
        }
        ("GET", "/v1/approvals") => {
            let list: Vec<Value> = shared
                .approvals
                .list(false)
                .iter()
                .rev()
                .take(50)
                .map(|a| a.view())
                .collect();
            json_response(stream, 200, &json!({"approvals": list}), &[]);
        }
        ("POST", "/v1/approvals/decide") => {
            // Always token-authenticated, even from loopback: a local process
            // (such as a browser driven by a tool) must not be able to approve.
            if !token_presented(shared, &request) {
                json_response(
                    stream,
                    401,
                    &json!({"error": "approval requires the node token"}),
                    &[],
                );
                return;
            }
            let body = serde_json::from_slice::<Value>(&request.body).unwrap_or(Value::Null);
            let (status, reply) = crate::approvals::decide(shared, &body);
            json_response(stream, status, &reply, &[]);
        }
        ("GET", "/v1/models") => models(shared, stream, &request),
        ("POST", "/v1/chat/completions") => chat(shared, stream, &request),
        ("POST", "/v1/kamimusuhi/talk") => talk(shared, stream, &request),
        ("POST", "/v1/kamimusuhi/history") => {
            let body = serde_json::from_slice::<Value>(&request.body).unwrap_or(Value::Null);
            let (status, reply) = crate::dialogue::history(shared, &body);
            json_response(stream, status, &reply, &[]);
        }
        ("GET", "/v1/tools") => json_response(stream, 200, &crate::tools::list(shared), &[]),
        ("POST", "/v1/tools/list") => {
            let body = serde_json::from_slice::<Value>(&request.body).unwrap_or(Value::Null);
            let local_only = body["local_only"].as_bool().unwrap_or(false);
            json_response(
                stream,
                200,
                &crate::tools::list_with(shared, local_only),
                &[],
            );
        }
        ("POST", "/v1/tools/call") => {
            let body = serde_json::from_slice::<Value>(&request.body).unwrap_or(Value::Null);
            let (status, reply) = crate::tools::call(shared, &body);
            json_response(stream, status, &reply, &[]);
        }
        ("POST", "/v1/library") => {
            let body = serde_json::from_slice::<Value>(&request.body).unwrap_or(Value::Null);
            let (status, reply) = crate::library::handle(shared, &body);
            json_response(stream, status, &reply, &[]);
        }
        _ => json_response(stream, 404, &json!({"error": "not found"}), &[]),
    }
}

fn local_only(request: &Request) -> bool {
    request
        .header(ROUTE_HEADER)
        .is_some_and(|v| v.eq_ignore_ascii_case("local"))
}

fn models(shared: &Shared, stream: TcpStream, request: &Request) {
    let local = local_only(request);
    let mut data = Vec::new();
    for tier in &shared.config.tiers {
        if local && !tier.node_local {
            continue;
        }
        if shared.tier_healthy(&tier.name) {
            data.push(
                json!({"id": tier.name, "object": "model", "owned_by": "kamimusuhi",
                             "backend_model": tier.model}),
            );
        }
    }
    if data.is_empty() {
        // A peer probing us must see "down" when no local tier can serve.
        json_response(
            stream,
            503,
            &json!({"error": {"message": "no healthy tier", "type": "service_unavailable"}}),
            &[],
        );
        return;
    }
    if !local {
        for name in AUTO_MODELS {
            data.insert(
                0,
                json!({"id": name, "object": "model", "owned_by": "kamimusuhi"}),
            );
        }
    }
    json_response(stream, 200, &json!({"object": "list", "data": data}), &[]);
}

fn chat(shared: &Shared, stream: TcpStream, request: &Request) {
    shared.requests.fetch_add(1, Ordering::Relaxed);
    let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
        json_response(
            stream,
            400,
            &json!({"error": {"message": "body is not JSON"}}),
            &[],
        );
        return;
    };
    if !body.get("messages").is_some_and(Value::is_array) {
        json_response(
            stream,
            400,
            &json!({"error": {"message": "messages[] required"}}),
            &[],
        );
        return;
    }
    let wants_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
    let routed = router::route(
        shared,
        &RouteRequest {
            body,
            local_only: local_only(request),
        },
    );
    let extra = vec![(
        "X-Kamimusuhi-Tier",
        routed.tier.clone().unwrap_or_else(|| "none".to_owned()),
    )];
    if wants_stream && routed.status == 200 {
        let sse = router::to_sse(&routed.body);
        let _ = respond(stream, 200, "text/event-stream", sse.as_bytes(), &extra);
    } else {
        json_response(stream, routed.status, &routed.body, &extra);
    }
}

fn talk(shared: &Shared, stream: TcpStream, request: &Request) {
    let Some(config) = &shared.config.dialogue else {
        json_response(
            stream,
            404,
            &json!({"error": "dialogue is served by the continuity node"}),
            &[],
        );
        return;
    };
    let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
        json_response(stream, 400, &json!({"error": "body is not JSON"}), &[]);
        return;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
    let (status, reply) = crate::dialogue::talk(shared, config, &body);
    json_response(stream, status, &reply, &[]);
}
