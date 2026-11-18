//! A tiny HTTP server on the loopback address for tests: each connection
//! gets one answer, made from the request's headers by the test.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

/// A running test server and the requests it has seen.
pub struct TestServer {
    pub url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl TestServer {
    /// Serves up to `answers` connections; `answer` gets the request's header
    /// lines (lowercase) and returns the whole raw response.
    pub fn start(answers: usize, answer: impl Fn(&str) -> Vec<u8> + Send + 'static) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/pkg", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        thread::spawn(move || {
            for stream in listener.incoming().take(answers) {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line.to_ascii_lowercase());
                }
                seen.lock().unwrap().push(head.clone());
                // The client may hang up early (a refused body): that is fine.
                let _ = stream.write_all(&answer(&head));
                let _ = stream.flush();
            }
        });
        TestServer { url, requests }
    }

    /// The header lines of every request so far, in order.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// A raw response with `status`, extra header lines and a body. Without a
/// `content-length` among the headers the body runs until the connection closes.
pub fn response(status: &str, headers: &[String], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\nconnection: close\r\n");
    for header in headers {
        out.push_str(header);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}
