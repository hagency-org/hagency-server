//! `gitfab-board`: serves the read-only GitFab board on 127.0.0.1. GET `/` is the page, GET
//! `/api/snapshot` the data, recomputed from the journals and receipts on each request. Every other
//! method is refused: the board cannot change anything.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

use clap::Parser;
use hagency_gitfab_board::{Probe, Sources, snapshot};
use hagency_gitfab_events::sign::Keyring;

const PAGE: &str = include_str!("page.html");

#[derive(Parser)]
#[command(name = "gitfab-board", about = "Read-only GitFab monitoring board")]
struct Cli {
    /// Directory of pinned `<key-id>.pub` files.
    #[arg(long)]
    keys: PathBuf,
    /// Event journals (task, identity, memory, token events).
    #[arg(long = "journal")]
    journals: Vec<PathBuf>,
    /// Runner receipt journals.
    #[arg(long = "receipts")]
    receipts: Vec<PathBuf>,
    /// Live deployment probe, `PROJECT=PORT` (health /healthz, version /version).
    #[arg(long = "probe")]
    probes: Vec<String>,
    #[arg(long, default_value = "127.0.0.1:8090")]
    listen: String,
}

/// The response for a request line: status, content type and body.
fn respond(method: &str, path: &str, sources: &Sources) -> (u16, &'static str, String) {
    if method != "GET" {
        return (405, "text/plain; charset=utf-8", "read-only board\n".into());
    }
    match path.split('?').next().unwrap_or("") {
        "/" => (200, "text/html; charset=utf-8", PAGE.into()),
        "/api/snapshot" => (200, "application/json", snapshot(sources).to_string()),
        _ => (404, "text/plain; charset=utf-8", "not found\n".into()),
    }
}

fn handle(mut stream: TcpStream, sources: &Sources) {
    let mut raw = Vec::new();
    let mut buf = [0u8; 1024];
    while !raw.windows(4).any(|w| w == b"\r\n\r\n") && raw.len() < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
        }
    }
    let head = String::from_utf8_lossy(&raw);
    let mut words = head.split_whitespace();
    let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    let (status, kind, body) = respond(method, path, sources);
    let reason = match status { 200 => "OK", 404 => "Not Found", _ => "Method Not Allowed" };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAllow: GET\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let keys = match Keyring::load_dir(&cli.keys) {
        Ok(keys) => keys,
        Err(e) => {
            eprintln!("keys: {e}");
            return std::process::ExitCode::from(2);
        }
    };
    let mut probes = Vec::new();
    for p in &cli.probes {
        let Some((project, port)) = p.split_once('=').and_then(|(a, b)| Some((a, b.parse().ok()?))) else {
            eprintln!("--probe must be PROJECT=PORT: {p}");
            return std::process::ExitCode::from(2);
        };
        probes.push(Probe { project: project.into(), port, health_path: "/healthz".into(), version_path: "/version".into() });
    }
    let sources = Sources { journals: cli.journals, receipts: cli.receipts, keys, probes };
    let listener = match TcpListener::bind(&cli.listen) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}: {e}", cli.listen);
            return std::process::ExitCode::from(2);
        }
    };
    eprintln!("gitfab-board on http://{}/", cli.listen);
    for stream in listener.incoming().flatten() {
        handle(stream, &sources);
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_reads_are_served() {
        let sources = Sources { journals: vec![], receipts: vec![], keys: Keyring::default(), probes: vec![] };
        assert_eq!(respond("GET", "/", &sources).0, 200);
        assert_eq!(respond("GET", "/api/snapshot", &sources).0, 200);
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            assert_eq!(respond(method, "/api/snapshot", &sources).0, 405, "{method}");
        }
        assert_eq!(respond("GET", "/api/runs", &sources).0, 404);
    }
}
