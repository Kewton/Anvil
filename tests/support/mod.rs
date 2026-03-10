#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone)]
pub struct RequestRecord {
    pub path: String,
    pub body: String,
}

#[derive(Debug)]
pub struct TestServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RequestRecord>>>,
}

impl TestServer {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<RequestRecord> {
        self.requests.lock().unwrap().clone()
    }
}

pub fn spawn_server(responses: Vec<String>) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_for_thread = Arc::clone(&requests);

    thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let size = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..size]).to_string();
            let mut parts = request.split("\r\n\r\n");
            let head = parts.next().unwrap_or_default();
            let body = parts.next().unwrap_or_default().to_string();
            let path = head
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            requests_for_thread
                .lock()
                .unwrap()
                .push(RequestRecord { path, body });
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
    });

    TestServer { addr, requests }
}

pub fn json_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

pub fn ndjson_response(lines: &[&str]) -> String {
    let body = format!("{}\n", lines.join("\n"));
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

pub fn sse_response(events: &[&str]) -> String {
    let body = format!(
        "{}\n\n",
        events
            .iter()
            .map(|event| format!("data: {event}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}
