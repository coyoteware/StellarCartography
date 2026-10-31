//! Byte sources for the dump: a local file, or an HTTP stream that resumes
//! with a Range request when the connection drops.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::blocking::{Client, Response};
use reqwest::header::{CONTENT_LENGTH, LAST_MODIFIED, RANGE};
use reqwest::StatusCode;

use crate::USER_AGENT;

pub struct SourceInfo {
    pub description: String,
    pub total: Option<u64>,
    pub last_modified: Option<String>,
    /// Compressed bytes consumed so far, for progress reporting.
    pub bytes_read: Arc<AtomicU64>,
}

pub type Source = Box<dyn Read + Send>;

pub fn open_file(path: &Path) -> Result<(Source, SourceInfo)> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let meta = file.metadata()?;
    let bytes_read = Arc::new(AtomicU64::new(0));
    let info = SourceInfo {
        description: path.display().to_string(),
        total: Some(meta.len()),
        last_modified: None,
        bytes_read: bytes_read.clone(),
    };
    Ok((Box::new(Counting { inner: io::BufReader::with_capacity(1 << 20, file), count: bytes_read }), info))
}

pub fn open_url(url: &str, tee: Option<&Path>) -> Result<(Source, SourceInfo)> {
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(30))
        // The body takes minutes to arrive; stalls are handled by resuming.
        .timeout(None)
        .build()?;
    let mut stream = HttpStream { client, url: url.to_string(), response: None, pos: 0, total: None, retries: 0 };
    let response = stream.connect()?;
    let total = response.headers().get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse().ok());
    let last_modified = response.headers().get(LAST_MODIFIED).and_then(|v| v.to_str().ok()).map(str::to_string);
    stream.total = total;
    stream.response = Some(response);

    let bytes_read = Arc::new(AtomicU64::new(0));
    let info = SourceInfo { description: url.to_string(), total, last_modified, bytes_read: bytes_read.clone() };
    let inner: Source = match tee {
        Some(path) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            Box::new(Tee { inner: stream, out: BufWriter::with_capacity(1 << 20, File::create(path)?) })
        }
        None => Box::new(stream),
    };
    Ok((Box::new(Counting { inner, count: bytes_read }), info))
}

struct Counting<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

struct Tee<R, W: Write> {
    inner: R,
    out: W,
}

impl<R: Read, W: Write> Read for Tee<R, W> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.out.write_all(&buf[..n])?;
        if n == 0 {
            self.out.flush()?;
        }
        Ok(n)
    }
}

struct HttpStream {
    client: Client,
    url: String,
    response: Option<Response>,
    pos: u64,
    total: Option<u64>,
    retries: u32,
}

const MAX_RETRIES: u32 = 6;

impl HttpStream {
    fn connect(&self) -> Result<Response> {
        let mut req = self.client.get(&self.url);
        if self.pos > 0 {
            req = req.header(RANGE, format!("bytes={}-", self.pos));
        }
        let resp = req.send()?;
        match resp.status() {
            StatusCode::OK if self.pos == 0 => Ok(resp),
            StatusCode::PARTIAL_CONTENT if self.pos > 0 => Ok(resp),
            status => bail!("GET {} at offset {}: {status}", self.url, self.pos),
        }
    }

    fn reconnect(&mut self, why: &str) -> io::Result<()> {
        self.response = None;
        loop {
            self.retries += 1;
            if self.retries > MAX_RETRIES {
                return Err(io::Error::other(format!("giving up after {MAX_RETRIES} retries: {why}")));
            }
            let wait = Duration::from_secs(2u64.pow(self.retries));
            eprintln!("  connection lost at {} bytes ({why}); resuming in {}s", self.pos, wait.as_secs());
            std::thread::sleep(wait);
            match self.connect() {
                Ok(resp) => {
                    self.response = Some(resp);
                    return Ok(());
                }
                Err(e) => eprintln!("  resume failed: {e}"),
            }
        }
    }
}

impl Read for HttpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.response.is_none() {
                self.reconnect("no response")?;
            }
            match self.response.as_mut().unwrap().read(buf) {
                Ok(0) if self.total.is_some_and(|t| self.pos < t) => {
                    let why = format!("body ended early at {} of {}", self.pos, self.total.unwrap());
                    self.reconnect(&why)?;
                }
                Ok(n) => {
                    self.pos += n as u64;
                    if n > 0 {
                        self.retries = 0;
                    }
                    return Ok(n);
                }
                Err(e) => self.reconnect(&e.to_string())?,
            }
        }
    }
}
