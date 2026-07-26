//! qana-lsp: stdio transport around the testable server core.
//! Content-Length framed JSON-RPC on stdin/stdout; a 500 ms ticker
//! drives the grammar-config hot-reload check between messages.

use qana_lsp::server;
use serde_json::Value;
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::Duration;

enum Event {
    Msg(Value),
    Tick,
    Eof,
}

fn main() {
    let (tx, rx) = mpsc::channel::<Event>();

    // stdin reader thread.
    let tx_in = tx.clone();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = std::io::BufReader::new(stdin.lock());
        loop {
            match read_message(&mut reader) {
                Some(v) => {
                    if tx_in.send(Event::Msg(v)).is_err() {
                        break;
                    }
                }
                None => {
                    let _ = tx_in.send(Event::Eof);
                    break;
                }
            }
        }
    });

    // Reload ticker.
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(500));
        if tx.send(Event::Tick).is_err() {
            break;
        }
    });

    let mut server = server::Server::new();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    loop {
        match rx.recv() {
            Ok(Event::Msg(msg)) => {
                let is_exit = msg.get("method").and_then(|m| m.as_str()) == Some("exit");
                for reply in server.handle(&msg) {
                    write_message(&mut out, &reply);
                }
                if is_exit {
                    break;
                }
            }
            Ok(Event::Tick) => {
                for reply in server.check_reload() {
                    write_message(&mut out, &reply);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
        }
    }
}

fn read_message<R: BufRead>(reader: &mut R) -> Option<Value> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().ok();
        }
    }
    let len = content_length?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

fn write_message<W: Write>(out: &mut W, msg: &Value) {
    let body = serde_json::to_vec(msg).expect("serializable");
    let _ = write!(out, "Content-Length: {}\r\n\r\n", body.len());
    let _ = out.write_all(&body);
    let _ = out.flush();
}
