mod config;
mod i3;
mod ipc;

use std::{collections::HashSet, fmt::Write, thread, time::Duration};

use cfgen::{prelude::*, ConfigLoad};
use crossbeam_channel as chan;
use crossbeam_channel::select;
use i3ipc::{I3Connection, I3EventListener, Subscription};
use serde_derive::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use structopt::StructOpt;

use crate::{
    config::{Config, Opacity},
    i3::{I3Ext, PROBABLE_AMOUNT_OF_WINDOWS},
    ipc::IpcServer,
};

fn run() -> Result<(), Error> {
    let opt = Opt::from_args();
    match opt.cmd {
        None => Daemon::new()?.run()?,
        Some(cmd) => {
            ipc::send_cmd(cmd).context(Ipc)?;
        }
    }
    Ok(())
}

#[derive(Snafu, Debug)]
enum Error {
    #[snafu(display("Can't load config: {}", source))]
    ConfigErr { source: cfgen::Error },

    #[snafu(display("Can't connect to i3: {}", source))]
    I3Connect { source: i3ipc::EstablishError },

    #[snafu(display("Can't communicate with i3: {}", source))]
    I3Comm { source: i3ipc::MessageError },

    #[snafu(display("Error in ipc: {}", source))]
    Ipc { source: ipc::Error },
}

impl From<i3ipc::MessageError> for Error {
    fn from(source: i3ipc::MessageError) -> Self {
        Error::I3Comm { source }
    }
}

#[derive(StructOpt)]
struct Opt {
    #[structopt(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(StructOpt, Serialize, Deserialize, Debug, Copy, Clone)]
pub enum Cmd {
    /// Disable opacity changes of unfocused windows
    #[structopt(name = "disable")]
    Disable,

    /// Enable opacity changes of unfocused windows
    #[structopt(name = "enable")]
    Enable,

    /// Toggle opacity changes of unfocused windows
    #[structopt(name = "toggle")]
    Toggle,

    /// Never apply opacity changes to currently focused window
    #[structopt(name = "focus-blacklist")]
    FocusBlacklist,

    /// Remove currently focused window from list of opacity excluded windows
    #[structopt(name = "focus-blacklist-remove")]
    FocusBlacklistRemove,
}

struct Daemon {
    transparency_active: bool,
    transparency: Opacity,
    blacklist: HashSet<i64>,
}

fn set_windows_opacity_to<I>(
    i3_conn: &mut I3Connection,
    windows: I,
    opacity: Opacity,
) -> Result<(), i3ipc::MessageError>
where
    I: IntoIterator<Item = i64>,
{
    let mut cmd = String::new();
    for id in windows {
        write!(cmd, "[con_id={}] opacity {};", id, opacity).unwrap();
    }
    i3_conn.run_command(&cmd)?;
    Ok(())
}

fn remove_all_transparency(i3_conn: &mut I3Connection) -> Result<(), i3ipc::MessageError> {
    let all_window_ids = i3_conn.iter_windows()?.map(|node| node.id);

    set_windows_opacity_to(i3_conn, all_window_ids, Opacity::max())?;

    Ok(())
}

impl Daemon {
    fn new() -> Result<Self, Error> {
        let (load, config) = Config::load_or_write_default().context(ConfigErr)?;
        if let ConfigLoad::DefaultWritten = load {
            println!("Default config written to {}", Config::path().display())
        }

        Ok(Self {
            transparency_active: config.transparency_at_start,
            transparency: config.opacity,
            blacklist: HashSet::new(),
        })
    }

    fn make_unfocused_windows_transparent(
        &self,
        i3_conn: &mut I3Connection,
    ) -> Result<(), i3ipc::MessageError> {
        if !self.transparency_active {
            return Ok(());
        }

        let mut unfocused = Vec::with_capacity(PROBABLE_AMOUNT_OF_WINDOWS);
        let mut focused = None;
        for node in i3_conn.iter_windows()? {
            if node.focused {
                focused = Some(node.id);
            } else if !self.blacklist.contains(&node.id) {
                unfocused.push(node.id);
            }
        }
        if let Some(id) = focused {
            i3_conn.run_command(&format!("[con_id={}] opacity {}", id, Opacity::max()))?;
        }

        set_windows_opacity_to(i3_conn, unfocused, self.transparency)?;
        Ok(())
    }

    fn run(&mut self) -> Result<(), Error> {
        let mut i3_conn = I3Connection::connect().context(I3Connect)?;

        // FIXME: these threads aren't shut down cleanly
        // the threads don't use anything except fds and those are closed on proc exit
        // inotify watches are also freed when the notify fd gets closed
        // so _currently_ ok (famous last words)
        let i3_event = spawn_listener_thread()?;
        let ipc = spawn_ipc_thread()?;
        let config_reload = spawn_config_reload_thread();

        log::debug!("Starting event loop");
        loop {
            select! {
                recv(config_reload) -> config => {
                    let config = config.expect("config reload thread died");
                    self.transparency = config.opacity;
                    self.make_unfocused_windows_transparent(&mut i3_conn)?;
                }
                recv(i3_event) -> event => {
                    let event = event.expect("i3 event listener thread died");
                    match event {
                        I3Event::FocusChanged => {
                            self.make_unfocused_windows_transparent(&mut i3_conn)?;
                        }
                        I3Event::Shutdown => {
                            return Ok(());
                        }
                        I3Event::CloseWindow(id) => {
                            log::debug!("Want to remove {} from blacklist", id);
                            log::debug!("Blacklist: {:?}", self.blacklist);
                            self.blacklist.remove(&id);
                        }
                    };
                }
                recv(ipc) -> cmd => {
                    let cmd = cmd.expect("ipc thread died");
                    match cmd {
                        Cmd::Disable => {
                            self.transparency_active = false;
                            remove_all_transparency(&mut i3_conn)?;
                        }
                        Cmd::Enable => {
                            self.transparency_active = true;
                            self.make_unfocused_windows_transparent(&mut i3_conn)?;
                        }
                        Cmd::Toggle => {
                            self.transparency_active = !self.transparency_active;
                            if self.transparency_active {
                                self.make_unfocused_windows_transparent(&mut i3_conn)?;
                            } else {
                                remove_all_transparency(&mut i3_conn)?;
                            }
                        }
                        Cmd::FocusBlacklist => {
                            if let Some(focused) = i3_conn.get_focused_window()? {
                                self.blacklist.insert(focused);
                            }
                        }
                        Cmd::FocusBlacklistRemove => {
                            if let Some(focused) = i3_conn.get_focused_window()? {
                                self.blacklist.remove(&focused);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn spawn_config_reload_thread() -> chan::Receiver<Config> {
    use inotify::{Inotify, WatchMask};

    let (tx, rx) = chan::bounded(1);

    let mut inotify = Inotify::init().unwrap();
    // FIXME: unjoined thread
    thread::spawn(move || {
        let watch_config = |ino: &mut Inotify| {
            ino.add_watch(
                Config::path(),
                WatchMask::CLOSE_WRITE | WatchMask::DELETE_SELF,
            )
        };

        let _ = watch_config(&mut inotify);

        let mut buf = [0u8; 4096];

        let mut on_event = move || -> Result<(), Box<dyn std::error::Error>> {
            let events = inotify.read_events_blocking(&mut buf)?;
            for event in events {
                if event.mask.contains(inotify::EventMask::DELETE_SELF)
                    && watch_config(&mut inotify).is_err()
                {
                    while watch_config(&mut inotify).is_err() {
                        thread::sleep(Duration::new(10, 0));
                    }
                }
            }

            let cfg = Config::load()?;

            tx.send(cfg).unwrap();

            Ok(())
        };

        loop {
            if let Err(e) = on_event() {
                log::warn!("{}", e);
            }
        }
    });

    rx
}

fn spawn_ipc_thread() -> Result<chan::Receiver<Cmd>, Error> {
    let srv = IpcServer::new(std::time::Duration::from_millis(100)).context(Ipc)?;

    let (tx, rx) = chan::bounded(1);

    // FIXME: unjoined thread
    thread::spawn(move || {
        for cmd in srv.incoming() {
            match cmd {
                Ok(cmd) => {
                    tx.send(cmd).unwrap();
                }
                Err(e) => {
                    log::warn!("Error while reading cmd: {}", e);
                }
            }
        }
    });

    Ok(rx)
}

#[derive(Debug)]
enum I3Event {
    FocusChanged,
    Shutdown,
    CloseWindow(i64),
}

fn spawn_listener_thread() -> Result<chan::Receiver<I3Event>, Error> {
    use i3ipc::event::{inner::WindowChange, Event, WindowEventInfo};

    let mut listener = I3EventListener::connect().context(I3Connect)?;
    let (tx, rx) = chan::bounded(1);
    listener
        .subscribe(&[Subscription::Window, Subscription::Shutdown])
        .context(I3Comm)?;

    // FIXME: unjoined thread
    thread::spawn(move || {
        for event in listener.listen().filter_map(|ev| ev.ok()) {
            match event {
                Event::WindowEvent(WindowEventInfo { change, container }) => match change {
                    WindowChange::Close => {
                        tx.send(I3Event::CloseWindow(container.id)).unwrap();
                    }
                    WindowChange::Focus => {
                        tx.send(I3Event::FocusChanged).unwrap();
                    }
                    _ => {}
                },
                Event::ShutdownEvent(_) => {
                    tx.send(I3Event::Shutdown).unwrap();
                }
                _ => {}
            }
        }
    });

    Ok(rx)
}

fn main() {
    env_logger::init();
    if let Err(e) = run() {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
