use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

struct PlannerStallServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl PlannerStallServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stalled ollama");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let addr = listener.local_addr().expect("listener addr");
        let stop = Arc::new(AtomicBool::new(false));
        let generate_count = Arc::new(AtomicUsize::new(0));
        let stop_for_thread = stop.clone();
        let handle = thread::spawn(move || {
            while !stop_for_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let generate_count = generate_count.clone();
                        thread::spawn(move || handle_client(stream, generate_count));
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            addr,
            stop,
            handle: Some(handle),
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for PlannerStallServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_client(mut stream: TcpStream, generate_count: Arc<AtomicUsize>) {
    let request = read_request(&mut stream);
    if request.starts_with("GET /api/tags ") {
        write_json(
            &mut stream,
            r#"{"models":[{"name":"qwen3:8b","details":{}}]}"#,
        );
        return;
    }
    if request.starts_with("POST /api/generate ") {
        let index = generate_count.fetch_add(1, Ordering::SeqCst);
        if index == 0 {
            let ultra_plan = serde_json::json!({
                "goal": "Build app",
                "profile": "generic",
                "style": "default",
                "phases": [
                    {"id": "phase-one", "prompt": "Create marker.txt with hello."},
                    {"id": "phase-two", "prompt": "Verify marker.txt exists."}
                ]
            });
            let body = serde_json::json!({
                "response": ultra_plan.to_string(),
                "done": true
            })
            .to_string();
            write_json(&mut stream, &body);
        } else {
            thread::sleep(Duration::from_secs(5));
        }
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let mut bytes = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                bytes.extend_from_slice(&buf[..n]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn write_json(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
}

#[test]
fn ultra_plan_run_step_planner_timeout_rewrites_terminal_summary() {
    let server = PlannerStallServer::start();
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_anvil"))
        .arg("--engine")
        .arg("minimal")
        .arg("--yes")
        .arg("--ultra-plan-run")
        .arg("Build app")
        .arg("--chat-timeout-secs")
        .arg("1")
        .arg("--cwd")
        .arg(work.path())
        .arg("--state-dir")
        .arg(state.path())
        .arg("--ollama-host")
        .arg(server.url())
        .env("ANVIL_NO_SPINNER", "1")
        .output()
        .expect("run anvil");

    assert!(!output.status.success(), "planner stall must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Status: running"), "{stderr}");
    assert!(
        stderr.contains("Status: phase_step_planner_timeout"),
        "{stderr}"
    );
    assert!(!stderr.trim_end().ends_with("Status: running"), "{stderr}");
    assert!(stderr.contains("phase_step_planner_timeout"), "{stderr}");

    let log = read_single_llm_log(state.path());
    assert!(
        log.lines().any(|line| {
            line.contains(r#""event":"tui_command_stop""#)
                && line.contains(r#""command":"ultra_plan_run""#)
                && line.contains(r#""status":"phase_step_planner_timeout""#)
        }),
        "{log}"
    );
}

fn read_single_llm_log(state_root: &std::path::Path) -> String {
    let sessions = state_root.join("sessions");
    let session_dir = std::fs::read_dir(&sessions)
        .unwrap()
        .next()
        .expect("session dir")
        .unwrap()
        .path();
    std::fs::read_to_string(session_dir.join("logs").join("llm-io.jsonl")).unwrap()
}
