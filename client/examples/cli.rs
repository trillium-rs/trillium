use blocking::Unblock;
use clap::Parser;
use std::{
    io::{ErrorKind, IsTerminal},
    path::PathBuf,
    str::FromStr,
};
use trillium::{Body, Method};
use trillium_client::{Client, Conn, Error, FollowRedirects};
use trillium_native_tls::NativeTlsConfig;
use trillium_rustls::RustlsConfig;
use trillium_smol::ClientConfig;
use url::{self, Url};

pub fn main() {
    ClientCli::parse().run()
}

#[derive(Parser, Debug)]
pub struct ClientCli {
    #[arg(value_parser = parse_method_case_insensitive)]
    method: Method,

    #[arg(value_parser = parse_url)]
    url: Url,

    /// provide a file system path to a file to use as the request body
    ///
    /// alternatively, you can use an operating system pipe to pass a file in
    ///
    /// three equivalent examples:
    ///
    /// trillium client post http://httpbin.org/anything -f ./body.json
    /// trillium client post http://httpbin.org/anything < ./body.json
    /// cat ./body.json | trillium client post http://httpbin.org/anything
    #[arg(short, long, verbatim_doc_comment)]
    file: Option<PathBuf>,

    /// provide a request body on the command line
    ///
    /// example:
    /// trillium client post http://httpbin.org/post -b '{"hello": "world"}'
    #[arg(short, long, verbatim_doc_comment)]
    body: Option<String>,

    /// provide headers in the form -h KEY1=VALUE1 KEY2=VALUE2
    ///
    /// example:
    /// trillium client get http://httpbin.org/headers -H Accept=application/json Authorization="Basic u:p"
    #[arg(short = 'H', long, value_parser = parse_header, verbatim_doc_comment)]
    headers: Vec<(String, String)>,

    /// tls implementation. options: rustls, native-tls, none
    ///
    /// requests to https:// urls with `none` will fail
    #[arg(short, long, default_value = "rustls", verbatim_doc_comment)]
    tls: TlsType,

    /// set the log level. add more flags for more verbosity
    ///
    /// example:
    /// trillium client get https://www.google.com -vvv # `trace` verbosity level
    #[command(flatten)]
    verbose: clap_verbosity_flag::Verbosity,
}

impl ClientCli {
    async fn build(&self) -> Conn {
        let client = match self.tls {
            TlsType::None => Client::new(ClientConfig::default()),
            TlsType::Rustls => Client::new(RustlsConfig::<ClientConfig>::default()),
            TlsType::NativeTls => Client::new(NativeTlsConfig::<ClientConfig>::default()),
        };

        let client = client
            .with_handler(FollowRedirects::new())
            .with_default_pool();

        let mut conn = client.build_conn(self.method, self.url.clone());
        for (name, value) in &self.headers {
            conn.request_headers().append(name.clone(), value.clone());
        }

        if let Some(path) = &self.file {
            let metadata = async_fs::metadata(path)
                .await
                .unwrap_or_else(|e| panic!("could not read file {:?} ({})", path, e));

            let file = async_fs::File::open(path)
                .await
                .unwrap_or_else(|e| panic!("could not read file {:?} ({})", path, e));

            conn.with_body(Body::new_streaming(file, Some(metadata.len())))
        } else if let Some(body) = &self.body {
            conn.with_body(body.clone())
        } else if !std::io::stdin().is_terminal() {
            conn.with_body(Body::new_streaming(Unblock::new(std::io::stdin()), None))
        } else {
            conn
        }
    }

    pub fn run(self) {
        trillium_smol::async_global_executor::block_on(async move {
            env_logger::Builder::new()
                .filter_module("trillium_client", self.verbose.log_level_filter())
                .init();

            let mut conn = self.build().await;

            if let Err(e) = (&mut conn).await {
                match e {
                    Error::Io(io) if io.kind() == ErrorKind::ConnectionRefused => {
                        log::error!("could not reach {}", self.url)
                    }

                    _ => log::error!("protocol error:\n\n{}", e),
                }

                return;
            }

            if std::io::stdout().is_terminal() {
                let body = conn.response_body().read_string().await.unwrap();

                let _request_headers_as_string = format!("{:#?}", conn.request_headers());
                let headers = conn.response_headers();
                let _response_headers_as_string = format!("{:#?}", headers);
                let _status_string = conn.status().unwrap().to_string();
                println!("{conn:#?}");
                println!("{body}");
            } else {
                futures_lite::io::copy(
                    &mut conn.response_body(),
                    &mut Unblock::new(std::io::stdout()),
                )
                .await
                .unwrap();
            }
        });
    }
}

#[derive(clap::ValueEnum, Debug, Eq, PartialEq, Clone)]
enum TlsType {
    None,
    Rustls,
    NativeTls,
}

fn parse_method_case_insensitive(src: &str) -> Result<Method, String> {
    src.to_uppercase()
        .parse()
        .map_err(|_| format!("unrecognized method {}", src))
}

fn parse_url(src: &str) -> Result<Url, url::ParseError> {
    if src.starts_with("http") {
        src.parse()
    } else {
        format!("http://{}", src).parse()
    }
}

impl FromStr for TlsType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match &*s.to_ascii_lowercase() {
            "none" => Ok(Self::None),
            "rustls" => Ok(Self::Rustls),
            "native" | "native-tls" => Ok(Self::NativeTls),
            _ => Err(format!("unrecognized tls {}", s)),
        }
    }
}

fn parse_header(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
    Ok((String::from(&s[..pos]), String::from(&s[pos + 1..])))
}
