//! A scripted HTTP server for the tests: answers each request with the next reply and records what it got.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Request {
    /// `PUT /path`
    pub line: String,
    /// Lower-cased header lines.
    pub headers: String,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> String {
        self.headers
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}: ")))
            .unwrap_or_default()
            .to_owned()
    }
}

pub struct Server {
    pub url: String,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    /// `replies` are (status line, extra headers, body); `{url}` in a header is this server.
    pub fn start(replies: &[(&str, &str, &str)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let replies: Vec<String> = replies
            .iter()
            .map(|(status, headers, body)| {
                let headers = headers.replace("{url}", &url);
                let headers = if headers.is_empty() {
                    headers
                } else {
                    format!("{headers}\r\n")
                };
                format!(
                    "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            })
            .collect();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        std::thread::spawn(move || {
            for reply in replies {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let (mut headers, mut length) = (String::new(), 0);
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header == "\r\n" {
                        break;
                    }
                    let header = header.trim_end().to_ascii_lowercase();
                    if let Some(value) = header.strip_prefix("content-length: ") {
                        length = value.parse().unwrap();
                    }
                    headers.push_str(&header);
                    headers.push('\n');
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                seen.lock().unwrap().push(Request {
                    line: line.rsplit_once(' ').unwrap().0.to_owned(),
                    headers,
                    body,
                });
                reader.get_mut().write_all(reply.as_bytes()).unwrap();
            }
        });
        Self { url, requests }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}
