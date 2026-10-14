//! A pretend game on localhost for the CLI's tests: it speaks JSON-RPC over HTTP the way BRP
//! does, and answers each request from a closure.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use serde_json::{json, Value};

/// Serves `handler(method, params)` on a free port until the test process ends; returns the
/// port. The handler returns the whole JSON-RPC response ([`reply`], [`error`]), or `None`
/// to drop the connection without answering (a game that went away).
pub fn serve(handler: impl Fn(&str, &Value) -> Option<Value> + Send + 'static) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut length = 0usize;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
                    break;
                }
                if let Some(n) = header.to_lowercase().strip_prefix("content-length:") {
                    length = n.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; length];
            if reader.read_exact(&mut body).is_err() {
                continue;
            }
            let request: Value = serde_json::from_slice(&body).unwrap();
            if let Some(response) = handler(request["method"].as_str().unwrap(), &request["params"]) {
                respond(reader.into_inner(), &response);
            }
        }
    });
    port
}

/// A successful JSON-RPC response.
pub fn reply(result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "result": result})
}

/// A JSON-RPC error response, as BRP answers a refused call.
pub fn error(message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32602, "message": message}})
}

fn respond(mut stream: TcpStream, response: &Value) {
    let body = response.to_string();
    let head = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}
