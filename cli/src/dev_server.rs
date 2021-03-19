use serde::Serialize;
use std::convert::TryInto;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use structopt::StructOpt;
#[derive(StructOpt, Debug)]
pub struct DevServer {
    // /// Local host or ip to listen on
    // #[structopt(short = "o", long, env, default_value = "localhost")]
    // host: String,

    // /// Local port to listen on
    // #[structopt(short, long, env, default_value = "8080")]
    // port: u16,
    #[structopt(short, long, env, parse(from_os_str), default_value = "src")]
    watch: Option<Vec<PathBuf>>,

    #[structopt(short, long, env, parse(from_os_str))]
    bin: Option<PathBuf>,

    #[structopt(short, long)]
    cwd: Option<PathBuf>,

    #[structopt(short, long)]
    release: bool,

    #[structopt(short, long)]
    example: Option<String>,

    #[structopt(short, long, default_value = "SIGTERM")]
    signal: Signal,
}

use signal_hook::consts::signal::{SIGHUP, SIGUSR1};
use signal_hook::iterator::Signals;

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;

use std::process::Command;

use notify::{RawEvent, RecommendedWatcher, RecursiveMode, Watcher};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum Event {
    BinaryChanged,
    Rebuild,
    Restarted,
    BuildSuccess,
    CompileError { error: String },
}

impl DevServer {
    fn determine_bin(&self) -> PathBuf {
        if let Some(ref bin) = self.bin {
            bin.canonicalize().unwrap()
        } else {
            let metadata = Command::new("cargo")
                .current_dir(self.cwd.clone().unwrap())
                .args(&["metadata", "--format-version", "1"])
                .output()
                .unwrap();

            let value: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
            let target_dir =
                PathBuf::from(value.get("target_directory").unwrap().as_str().unwrap());

            let root = value
                .get("resolve")
                .unwrap()
                .get("root")
                .unwrap()
                .as_str()
                .unwrap()
                .split(' ')
                .next()
                .unwrap();

            let target_dir = target_dir.join(if self.release { "release" } else { "debug" });
            let target_dir = if let Some(example) = &self.example {
                target_dir.join("examples").join(example)
            } else {
                target_dir.join(root)
            };

            target_dir.canonicalize().unwrap()
        }
    }

    pub fn run(mut self) {
        env_logger::Builder::new()
            .filter_level(log::LevelFilter::Debug)
            .init();

        let cwd = self
            .cwd
            .get_or_insert_with(|| std::env::current_dir().unwrap())
            .clone();

        let bin = self.determine_bin();

        let mut run = Command::new(&bin);
        run.current_dir(&cwd);

        let mut build = Command::new("cargo");
        let mut args = vec!["build", "--color=always"];
        if self.release {
            args.push("--release");
        }

        let signal = self.signal;

        if let Some(example) = &self.example {
            args.push("--example");
            args.push(example);
            self.watch
                .get_or_insert_with(Vec::new)
                .push(cwd.join("examples"));
        }

        build.args(&args[..]);
        build.current_dir(&cwd);

        let mut child = run.spawn().unwrap();
        let child_id = Arc::new(Mutex::new(child.id()));

        let (tx, rx) = std::sync::mpsc::channel();
        let broadcaster = broadcaster::BroadcastChannel::new();

        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut signals = Signals::new(&[SIGHUP, SIGUSR1]).unwrap();

                loop {
                    for signal in signals.pending() {
                        if let SIGHUP = signal as libc::c_int {
                            tx.send(Event::BinaryChanged).unwrap();
                        }
                    }
                }
            });
        }

        std::thread::spawn(move || {
            let (t, r) = std::sync::mpsc::channel::<RawEvent>();
            let mut watcher = RecommendedWatcher::new_raw(t).unwrap();

            if let Some(watches) = self.watch {
                for watch in watches {
                    let watch = if watch.is_relative() {
                        cwd.join(watch)
                    } else {
                        watch
                    };

                    let watch = watch.canonicalize().unwrap();
                    log::info!("watching {:?}", &watch);
                    watcher.watch(watch, RecursiveMode::Recursive).unwrap();
                }
            }

            log::info!("watching {:?}", &bin);
            watcher.watch(&bin, RecursiveMode::NonRecursive).unwrap();

            while let Ok(m) = r.recv() {
                if let Some(path) = m.path {
                    if let Ok(path) = path.canonicalize() {
                        if path == bin {
                            tx.send(Event::Signal).unwrap();
                        } else {
                            tx.send(Event::Rebuild).unwrap();
                        }
                    }
                }
            }
        });

        {
            let child_id = child_id.clone();
            let broadcaster = broadcaster.clone();
            std::thread::spawn(move || loop {
                child.wait().unwrap();
                log::info!("shut down, restarting");
                child = run.spawn().unwrap();
                *child_id.lock().unwrap() = child.id();
                std::thread::sleep(Duration::from_millis(500));
                async_io::block_on(broadcaster.send(&Event::Restarted)).ok();
            });
        }
        {
            let broadcaster = broadcaster.clone();
            std::thread::spawn(move || loop {
                let event = rx.recv().unwrap();
                async_io::block_on(broadcaster.send(&event)).unwrap();
                match event {
                    Event::BinaryChanged => {
                        log::info!("attempting to send {}", &signal);
                        signal::kill(
                            Pid::from_raw((*child_id.lock().unwrap()).try_into().unwrap()),
                            signal,
                        )
                        .unwrap();
                    }
                    Event::Rebuild => {
                        log::info!("building...");
                        let output = build.output();
                        match output {
                            Ok(ok) => {
                                if ok.status.success() {
                                    log::debug!("{}", String::from_utf8_lossy(&ok.stdout[..]));
                                    async_io::block_on(broadcaster.send(&Event::BuildSuccess)).ok();
                                } else {
                                    std::io::stderr().write_all(&ok.stderr).unwrap();
                                    async_io::block_on(
                                        broadcaster.send(&Event::CompileError {
                                            error: ansi_to_html::convert_escaped(
                                                &String::from_utf8_lossy(&ok.stderr),
                                            )
                                            .unwrap(),
                                        }),
                                    )
                                    .ok();
                                }
                            }
                            Err(e) => {
                                eprintln!("{:?}", e);
                            }
                        }
                    }
                    _ => {}
                }
            });
        }

        proxy_app::run(format!("http://{}:{}", "localhost", "8080"), broadcaster);
    }
}

mod proxy_app {
    use super::Event;
    use broadcaster::BroadcastChannel;
    use futures_lite::StreamExt;
    use myco::{sequence, Conn};
    use myco_client::Client;
    use myco_html_rewriter::{
        html::{element, html_content::ContentType, Settings},
        HtmlRewriter,
    };
    use myco_proxy::{Proxy, TcpStream};
    use myco_router::Router;
    use myco_websockets::WebSocket;

    pub fn run(proxy: String, rx: BroadcastChannel<Event>) {
        static PORT: u16 = 8082;
        let client = Client::new()
            .with_default_pool()
            .with_config(myco_proxy::TcpConfig {
                nodelay: Some(true),
                ..Default::default()
            });

        myco_smol_server::config()
            .without_signals()
            .with_port(PORT)
            .run(sequence![
                Router::new()
                    .get("/_dev_server.js", |conn: Conn| async move {
                        conn.with_header(("content-type", "application/javascript"))
                            .ok(include_str!("./dev_server.js"))
                    })
                    .get(
                        "/_dev_server.ws",
                        sequence![
                            myco::State::new(rx),
                            WebSocket::new(|mut wsc| async move {
                                let mut rx = wsc.take_state::<BroadcastChannel<Event>>().unwrap();
                                while let Some(message) = rx.next().await {
                                    if let Err(e) = wsc.send_json(&message).await {
                                        log::error!("{:?}", e);
                                        return;
                                    }
                                }
                            })
                        ]
                    ),
                Proxy::<TcpStream>::new(&*proxy).with_client(client),
                HtmlRewriter::new(|| Settings {
                    element_content_handlers: vec![element!("body", |el| {
                        el.append(
                            r#"<script src="/_dev_server.js"></script>"#,
                            ContentType::Html,
                        );
                        Ok(())
                    })],

                    ..Settings::default()
                })
            ]);
    }
}
