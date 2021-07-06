use crate::RootPath;
use log::LevelFilter;
use std::{fmt::Debug, fs, io::Write, path::PathBuf};
use structopt::StructOpt;
use trillium::{FileSystem, Handler};
use trillium_client::Connector;
use trillium_logger::Logger;
use trillium_native_tls::NativeTlsAcceptor;
use trillium_proxy::Proxy;
use trillium_rustls::RustlsAcceptor;

use trillium_static::StaticFileHandler;

#[derive(StructOpt, Debug)]
#[structopt(
    setting = structopt::clap::AppSettings::DeriveDisplayOrder
)]
pub struct StaticCli {
    /// Filesystem path to serve
    ///
    /// Defaults to the current working directory
    #[structopt(parse(from_os_str), default_value)]
    root: RootPath,

    /// Local host or ip to listen on
    #[structopt(short = "o", long, env, default_value = "localhost")]
    host: String,

    /// Local port to listen on
    #[structopt(short, long, env, default_value = "8080")]
    port: u16,

    /// Path to a tls certificate for tide_rustls
    ///
    /// This will panic unless rustls_key is also provided. Providing
    /// both rustls_key and rustls_cert enables tls.
    ///
    /// Example: `--rustls_cert ./cert.pem --rustls_key ./key.pem`
    /// For development, try using mkcert
    #[structopt(long, env, parse(from_os_str))]
    rustls_cert: Option<PathBuf>,

    /// The path to a tls key file for tide_rustls
    ///
    /// This will panic unless rustls_cert is also provided. Providing
    /// both rustls_key and rustls_cert enables tls.
    ///
    /// Example: `--rustls_cert ./cert.pem --rustls_key ./key.pem`
    /// For development, try using mkcert
    #[structopt(long, env, parse(from_os_str))]
    rustls_key: Option<PathBuf>,

    #[structopt(long, env, parse(from_os_str))]
    native_tls_identity: Option<PathBuf>,

    #[structopt(long, env)]
    native_tls_password: Option<String>,

    /// Host to forward (reverse proxy) not-found requests to
    ///
    /// This forwards any request that would otherwise be a 404 Not
    /// Found to the specified listener spec.
    ///
    /// Examples:
    ///    `--forward localhost:8081`
    ///    `--forward http://localhost:8081`
    ///    `--forward https://localhost:8081`
    ///
    /// Note: http+unix:// schemes are not yet supported
    #[structopt(short, long, env = "FORWARD")]
    forward: Option<String>,

    #[structopt(short, long, env)]
    index: Option<String>,

    /// set the log level. add more flags for more verbosity
    ///
    #[structopt(short, long, parse(from_occurrences))]
    verbose: u8,
}

impl StaticCli {
    pub fn root(&self) -> &RootPath {
        &self.root
    }

    pub fn forward(&self) -> Option<&str> {
        self.forward.as_deref()
    }

    pub fn index(&self) -> Option<&str> {
        self.index.as_deref()
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn rustls_acceptor(&self) -> Option<RustlsAcceptor> {
        match &self {
            StaticCli {
                rustls_cert: Some(_),
                rustls_key: None,
                ..
            }
            | StaticCli {
                rustls_cert: None,
                rustls_key: Some(_),
                ..
            } => {
                panic!("rustls_cert_path must be combined with rustls_key_path");
            }

            StaticCli {
                rustls_cert: Some(x),
                rustls_key: Some(y),
                native_tls_identity: None,
                ..
            } => Some(RustlsAcceptor::from_pkcs8(
                &fs::read(x).unwrap(),
                &fs::read(y).unwrap(),
            )),

            StaticCli {
                rustls_cert: Some(_),
                rustls_key: Some(_),
                native_tls_identity: Some(_),
                ..
            } => {
                panic!("sorry, i'm not sure what to do when provided with both native tls and rustls info. please pick one or the other")
            }

            _ => None,
        }
    }

    pub fn native_tls_acceptor(&self) -> Option<NativeTlsAcceptor> {
        match &self {
            StaticCli {
                native_tls_identity: Some(_),
                native_tls_password: None,
                ..
            }
            | StaticCli {
                native_tls_identity: None,
                native_tls_password: Some(_),
                ..
            } => {
                panic!("native_tls_identity_path and native_tls_identity_password must be used together");
            }

            StaticCli {
                rustls_cert: None,
                rustls_key: None,
                native_tls_identity: Some(x),
                native_tls_password: Some(y),
                ..
            } => Some(NativeTlsAcceptor::from_pkcs12(&fs::read(x).unwrap(), y)),

            StaticCli {
                rustls_cert: Some(_),
                rustls_key: Some(_),
                native_tls_identity: Some(_),
                ..
            } => {
                panic!("sorry, i'm not sure what to do when provided with both native tls and rustls info. please pick one or the other")
            }

            _ => None,
        }
    }

    pub fn run(self) {
        env_logger::Builder::new()
            .filter_level(match self.verbose {
                0 => LevelFilter::Info,
                1 => LevelFilter::Debug,
                _ => LevelFilter::Trace,
            })
            .format(|buf, record| writeln!(buf, "{}", record.args()))
            .init();

        let config = trillium_smol::config()
            .with_port(self.port())
            .with_host(self.host());

        if let Some(x) = self.native_tls_acceptor() {
            config.with_acceptor(x).run(self.handler());
        } else if let Some(x) = self.rustls_acceptor() {
            config.with_acceptor(x).run(self.handler());
        } else {
            config.run(self.handler());
        }
    }

    fn handler<T>(&self) -> impl Handler<T>
    where
        T: Connector + FileSystem,
    {
        let path = self.root().clone();
        let mut static_file_handler = StaticFileHandler::new(path);
        if let Some(index) = self.index() {
            static_file_handler = static_file_handler.with_index_file(index);
        }

        (
            Logger::new(),
            self.forward().map(Proxy::new),
            static_file_handler,
        )
    }
}
