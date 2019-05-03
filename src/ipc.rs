use std::{
    fs, io,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    time::Duration,
};

use fs2::FileExt;
use lazy_static::lazy_static;
use snafu::{ResultExt, Snafu};

use crate::Cmd;

#[derive(Snafu, Debug)]
pub enum Error {
    #[snafu(display("Can't connect to server: {}", source))]
    Connect { source: io::Error },

    #[snafu(display("IO error while doing ipc: {}", source))]
    Io { source: io::Error },

    #[snafu(display("transparentd already running {}", source))]
    AlreadyRunning { source: io::Error },

    #[snafu(display("Can't create socket dir: {}", source))]
    Mkdir { source: io::Error },

    #[snafu(display("Failed to serialize/deserialize cbor: {}", source))]
    Cbor { source: serde_cbor::error::Error },
}

lazy_static! {
    static ref RUN_DIR: PathBuf = {
        directories::ProjectDirs::from("org", "foldu", "transparentd")
            .unwrap()
            .runtime_dir()
            .unwrap()
            .to_owned()
    };
    static ref SOCK_PATH: PathBuf = { RUN_DIR.join("ipc.sock") };
    static ref LOCKFILE_PATH: PathBuf = { RUN_DIR.join(".lockfile") };
}

pub struct IpcServer {
    listener: UnixListener,
    timeout: Duration,
    _lock: FileLock,
}

pub struct FileLock {
    _fd: fs::File,
}

impl FileLock {
    pub fn lock<P>(path: P) -> Result<Self, io::Error>
    where
        P: AsRef<Path>,
    {
        let fd = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        fd.try_lock_exclusive()?;
        Ok(Self { _fd: fd })
    }
}

impl IpcServer {
    pub fn new(timeout: Duration) -> Result<Self, Error> {
        fs::create_dir_all(Path::new(&*SOCK_PATH).parent().unwrap()).context(Mkdir)?;
        let lock = FileLock::lock(&*LOCKFILE_PATH).context(AlreadyRunning)?;
        let _ = fs::remove_file(&*SOCK_PATH);
        let listener = UnixListener::bind(&*SOCK_PATH).context(Io)?;

        Ok(Self {
            listener,
            timeout,
            _lock: lock,
        })
    }

    pub fn incoming(&self) -> Incoming<'_> {
        Incoming {
            listener: &self.listener,
            timeout: self.timeout,
        }
    }
}

pub struct Incoming<'a> {
    listener: &'a UnixListener,
    timeout: Duration,
}

type StreamItem = Result<Cmd, Error>;

impl Incoming<'_> {
    fn accept(&mut self) -> StreamItem {
        let (stream, _) = self.listener.accept().context(Io)?;
        stream.set_read_timeout(Some(self.timeout)).context(Io)?;
        serde_cbor::from_reader(stream).eager_context(Cbor)
    }
}

impl Iterator for Incoming<'_> {
    type Item = StreamItem;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.accept())
    }
}

pub fn send_cmd(cmd: Cmd) -> Result<(), Error> {
    let mut sock = UnixStream::connect(&*SOCK_PATH).context(Connect)?;
    serde_cbor::to_writer(&mut sock, &cmd).eager_context(Cbor)
}
