use crate::command::{Command, Status};
use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, RwLock,
    },
    thread,
    time::{Duration, Instant},
};

const LIMIT: usize = 16 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);

pub fn default_socket() -> PathBuf {
    let uid = unsafe { libc::geteuid() };
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .map(|p| p.join("xr-cinema"))
        .unwrap_or_else(|| std::env::temp_dir().join(format!("xr-cinema-{uid}")));
    dir.join("control.sock")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    id: Value,
    command: Command,
}

pub struct Pending {
    pub command: Command,
    pub expires: Instant,
    pub reply: mpsc::Sender<Result<Value, String>>,
}

fn read_line(stream: &UnixStream) -> Result<Vec<u8>> {
    let mut line = Vec::new();
    BufReader::new(stream.take((LIMIT + 1) as u64)).read_until(b'\n', &mut line)?;
    ensure!(line.len() <= LIMIT, "message exceeds 16 KiB");
    ensure!(
        line.last() == Some(&b'\n'),
        "expected newline-terminated JSON"
    );
    Ok(line)
}

fn serve(
    stream: &mut UnixStream,
    queue: &mpsc::SyncSender<Pending>,
    latest: &RwLock<Status>,
) -> Result<()> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut id = Value::Null;
    let result: Result<Value> = (|| {
        let value: Value = serde_json::from_slice(&read_line(stream)?)?;
        if let Some(v) = value.get("id") {
            ensure!(
                v.is_null() || v.is_number() || v.as_str().is_some_and(|s| s.len() <= 128),
                "invalid id"
            );
            id = v.clone();
        }
        let request: Request = serde_json::from_value(value)?;
        ensure!(request.version == 1, "unsupported protocol version");
        debug_assert_eq!(id, request.id);
        {
            let state = latest
                .read()
                .map_err(|_| anyhow::anyhow!("player offline"))?;
            if matches!(request.command, Command::Status) {
                return Ok(serde_json::to_value(&*state)?);
            }
            ensure!(state.ready, "player is starting; wait for ready");
        }
        if let Command::Open { path } = &request.command {
            ensure!(path.is_absolute(), "open requires an absolute local path");
        }
        let (reply, result) = mpsc::channel();
        queue
            .try_send(Pending {
                command: request.command,
                expires: Instant::now() + TIMEOUT,
                reply,
            })
            .context("player busy or offline")?;
        result.recv_timeout(TIMEOUT)?.map_err(anyhow::Error::msg)
    })();
    let response = match result {
        Ok(state) => json!({"version":1, "id":id, "ok":true, "state":state}),
        Err(err) => json!({"version":1, "id":id, "ok":false, "error":err.to_string()}),
    };
    serde_json::to_writer(&mut *stream, &response)?;
    stream.write_all(b"\n")?;
    Ok(())
}

pub struct Server {
    pub requests: mpsc::Receiver<Pending>,
    path: PathBuf,
    _lock: File,
    stopping: Arc<AtomicBool>,
    listener: Option<thread::JoinHandle<()>>,
    latest: Arc<RwLock<Status>>,
}

impl Server {
    pub fn start(path: &Path) -> Result<Self> {
        let parent = path.parent().context("socket directory missing")?;
        match fs::DirBuilder::new().mode(0o700).create(parent) {
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e.into()),
            _ => (),
        }
        let meta = fs::symlink_metadata(parent)?;
        ensure!(
            meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
            "control socket needs a private, user-owned directory"
        );
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path.with_extension("lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        match fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_socket() => fs::remove_file(path)?,
            Ok(_) => bail!("unexpected file at {}", path.display()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let listener = UnixListener::bind(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        let (queue, requests) = mpsc::sync_channel(16);
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = stopping.clone();
        let latest = Arc::new(RwLock::new(Status::initial()));
        let snapshot = latest.clone();
        let worker = thread::spawn(move || {
            let clients = Arc::new(AtomicUsize::new(0));
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                if stop.load(Ordering::Acquire) {
                    break;
                }
                if clients.load(Ordering::Relaxed) >= 8 {
                    continue;
                }
                clients.fetch_add(1, Ordering::Relaxed);
                let clients = clients.clone();
                let queue = queue.clone();
                let snapshot = snapshot.clone();
                thread::spawn(move || {
                    let _ = serve(&mut stream, &queue, &snapshot);
                    clients.fetch_sub(1, Ordering::Relaxed);
                });
            }
        });
        Ok(Self {
            requests,
            path: path.into(),
            _lock: lock,
            stopping,
            listener: Some(worker),
            latest,
        })
    }

    pub fn publish(&self, state: &Status) {
        self.latest
            .write()
            .expect("status snapshot")
            .clone_from(state);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if UnixStream::connect(&self.path).is_ok() {
            if let Some(worker) = self.listener.take() {
                let _ = worker.join();
            }
        }
        let _ = fs::remove_file(&self.path);
    }
}

pub fn send(path: &Path, command: Command) -> Result<Value> {
    let mut stream =
        UnixStream::connect(path).with_context(|| format!("connecting to {}", path.display()))?;
    stream.set_read_timeout(Some(TIMEOUT + Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    serde_json::to_writer(&mut stream, &json!({"version":1,"id":1,"command":command}))?;
    stream.write_all(b"\n")?;
    let response: Value = serde_json::from_slice(&read_line(&stream)?)?;
    ensure!(
        response["ok"] == true,
        "{}",
        response["error"]
            .as_str()
            .unwrap_or("invalid server response")
    );
    Ok(response)
}
