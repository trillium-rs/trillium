use std::convert::TryInto;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use structopt::StructOpt;

#[derive(StructOpt, Debug)]
pub struct DevServer {
    /// Local host or ip to listen on
    #[structopt(short = "o", long, env, default_value = "localhost")]
    host: String,

    /// Local port to listen on
    #[structopt(short, long, env, default_value = "8080")]
    port: u16,

    #[structopt(short, long, env, parse(from_os_str))]
    watch: Option<Vec<PathBuf>>,

    #[structopt(short, long, env, parse(from_os_str))]
    bin: PathBuf,

    #[structopt(short, long)]
    cwd: Option<PathBuf>,

    #[structopt(short, long)]
    release: bool,
}

use signal_hook::consts::signal::{SIGHUP, SIGUSR1};
use signal_hook::iterator::Signals;

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;

use std::process::Command;

use notify::{DebouncedEvent, RecommendedWatcher, RecursiveMode, Watcher};

#[derive(Debug)]
enum Event {
    Signal,
    Rebuild,
}

impl DevServer {
    pub fn run(self) {
        env_logger::init();

        let cwd = self.cwd.unwrap_or_else(|| std::env::current_dir().unwrap());
        let mut run = Command::new(self.bin.canonicalize().unwrap());
        run.current_dir(&cwd);

        let mut build = Command::new("cargo");
        let mut args = vec!["build", "--color=always"];
        if self.release {
            args.push("--release");
        }
        build.args(&args[..]);
        build.current_dir(&cwd);

        let mut child = run.spawn().unwrap();
        let child_id = Arc::new(Mutex::new(child.id()));

        let (tx, rx) = std::sync::mpsc::channel();

        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut signals = Signals::new(&[SIGHUP, SIGUSR1]).unwrap();

                loop {
                    for signal in signals.pending() {
                        if let SIGHUP = signal as libc::c_int {
                            tx.send(Event::Signal).unwrap();
                        }
                    }
                }
            });
        }

        if let Some(watches) = self.watch {
            let bin = self.bin.clone().canonicalize().unwrap();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let (t, r) = std::sync::mpsc::channel::<DebouncedEvent>();
                let mut watcher = RecommendedWatcher::new(t, Duration::from_secs(1)).unwrap();

                for watch in watches {
                    watcher
                        .watch(watch.canonicalize().unwrap(), RecursiveMode::Recursive)
                        .unwrap();
                }

                watcher.watch(&bin, RecursiveMode::NonRecursive).unwrap();

                while let Ok(m) = r.recv() {
                    let path = match &m {
                        DebouncedEvent::Create(p) => Some(p),
                        DebouncedEvent::Write(p) => Some(p),
                        DebouncedEvent::Chmod(p) => Some(p),
                        DebouncedEvent::Remove(p) => Some(p),
                        DebouncedEvent::Rename(_, p) => Some(p),
                        _ => None,
                    };

                    if let Some(path) = path {
                        let path = path.canonicalize().unwrap();

                        if path == bin {
                            tx.send(Event::Signal).unwrap();
                        } else {
                            tx.send(Event::Rebuild).unwrap();
                        }
                    }
                }
            });
        }

        {
            let child_id = child_id.clone();
            std::thread::spawn(move || loop {
                child.wait().unwrap();
                println!("shut down, restarting");
                child = run.spawn().unwrap();
                *child_id.lock().unwrap() = child.id();
            });
        }

        loop {
            match rx.recv().unwrap() {
                Event::Signal => {
                    signal::kill(
                        Pid::from_raw((*child_id.lock().unwrap()).try_into().unwrap()),
                        Signal::SIGTERM,
                    )
                    .unwrap();
                }
                Event::Rebuild => {
                    println!("building...");
                    let output = build.output();
                    while let Ok(x) = rx.try_recv() {
                        println!("discarding {:?}", x);
                    }
                    match output {
                        Ok(ok) => {
                            if ok.status.success() {
                                // std::io::stdout().write_all(&ok.stdout).unwrap();
                                // std::io::stderr().write_all(&ok.stderr).unwrap();
                            } else {
                                std::io::stderr().write_all(&ok.stderr).unwrap();
                            }
                        }
                        Err(e) => {
                            eprintln!("{:?}", e);
                        }
                    }
                }
            }
        }
    }
}
