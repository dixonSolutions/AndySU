//! `andysu serve`: a tiny static HTTP/1.1 server for local remotes and tests. Supports
//! `Range: bytes=N-`, `ETag`/`If-None-Match` (304), and fault injection: dropping the connection
//! after N body bytes (once per path) to simulate network loss, and failing paths with 404/500.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Faults {
    /// Close the connection after this many body bytes, the first time each path is served.
    pub drop_after: Option<u64>,
    /// Only drop paths starting with this prefix (empty = all).
    pub drop_prefix: String,
    /// Paths (prefix match, relative to the root) answered with this status.
    pub fail: Vec<(String, u16)>,
    /// Limit each response body to about this many bytes per second.
    pub throttle_bps: Option<u64>,
}

#[derive(Clone)]
pub struct Server {
    pub port: u16,
    pub log: Arc<Mutex<Vec<String>>>,
    pub faults: Arc<Mutex<Faults>>,
}

impl Server {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    pub fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    pub fn set_faults(&self, f: Faults) {
        *self.faults.lock().unwrap() = f;
    }
}

fn etag_of(p: &Path) -> Option<String> {
    let md = std::fs::metadata(p).ok()?;
    let mt = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("\"{:x}-{:x}\"", md.len(), mt))
}

fn handle(
    mut s: TcpStream,
    root: &Path,
    log: &Mutex<Vec<String>>,
    faults: &Mutex<Faults>,
    dropped: &Mutex<HashSet<String>>,
) -> std::io::Result<()> {
    let mut r = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/").to_string());
    let (mut range, mut inm) = (None, None);
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        let (k, v) = h.split_once(':').unwrap_or((&h, ""));
        match k.trim().to_ascii_lowercase().as_str() {
            "range" => {
                range = v
                    .trim()
                    .strip_prefix("bytes=")
                    .and_then(|x| x.strip_suffix('-'))
                    .and_then(|x| x.parse::<u64>().ok())
            }
            "if-none-match" => inm = Some(v.trim().to_string()),
            _ => {}
        }
    }
    let path = target.split('?').next().unwrap_or("/").trim_start_matches('/').to_string();
    let f = faults.lock().unwrap().clone();
    let reply = |s: &mut TcpStream, code: u16, msg: &str| -> std::io::Result<()> {
        write!(s, "HTTP/1.1 {code} {msg}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    };
    let mut entry = format!("{method} /{path}");
    if let Some(o) = range {
        entry.push_str(&format!(" range={o}"));
    }
    if let Some((_, code)) = f.fail.iter().find(|(pre, _)| path.starts_with(pre.as_str())) {
        log.lock().unwrap().push(format!("{entry} -> {code}"));
        return reply(&mut s, *code, "Injected");
    }
    let fp: PathBuf = root.join(&path);
    if path.split('/').any(|c| c == "..") || !fp.is_file() {
        log.lock().unwrap().push(format!("{entry} -> 404"));
        return reply(&mut s, 404, "Not Found");
    }
    let tag = etag_of(&fp).unwrap_or_default();
    if inm.as_deref() == Some(tag.as_str()) {
        log.lock().unwrap().push(format!("{entry} -> 304"));
        return write!(s, "HTTP/1.1 304 Not Modified\r\nETag: {tag}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    }
    let mut file = std::fs::File::open(&fp)?;
    let len = file.metadata()?.len();
    let start = range.unwrap_or(0).min(len);
    let mut body = vec![];
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(start))?;
    file.read_to_end(&mut body)?;
    let code = if range.is_some() { 206 } else { 200 };
    log.lock().unwrap().push(format!("{entry} -> {code}"));
    let mut head = format!(
        "HTTP/1.1 {code} OK\r\nContent-Length: {}\r\nETag: {tag}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n",
        body.len()
    );
    if code == 206 {
        head.push_str(&format!("Content-Range: bytes {start}-{}/{len}\r\n", len.saturating_sub(1)));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes())?;
    if method == "HEAD" {
        return Ok(());
    }
    if let Some(n) = f.drop_after {
        if (n as usize) < body.len()
            && path.starts_with(f.drop_prefix.as_str())
            && dropped.lock().unwrap().insert(path.clone())
        {
            s.write_all(&body[..n as usize])?;
            s.flush()?;
            log.lock().unwrap().push(format!("{entry} dropped after {n} bytes"));
            return s.shutdown(std::net::Shutdown::Both);
        }
    }
    match f.throttle_bps {
        Some(bps) if bps > 0 => {
            let chunk = (bps / 20).max(1024) as usize;
            for c in body.chunks(chunk) {
                s.write_all(c)?;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Ok(())
        }
        _ => s.write_all(&body),
    }
}

/// Serve `root` on `addr` (use port 0 for an ephemeral port) in background threads.
pub fn spawn(root: &Path, addr: &str, faults: Faults) -> std::io::Result<Server> {
    let l = TcpListener::bind(addr)?;
    let port = l.local_addr()?.port();
    let srv = Server { port, log: Arc::new(Mutex::new(vec![])), faults: Arc::new(Mutex::new(faults)) };
    let (root, s2) = (root.to_path_buf(), srv.clone());
    let dropped = Arc::new(Mutex::new(HashSet::new()));
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            let (root, s3, d) = (root.clone(), s2.clone(), dropped.clone());
            std::thread::spawn(move || {
                let _ = handle(c, &root, &s3.log, &s3.faults, &d);
            });
        }
    });
    Ok(srv)
}
