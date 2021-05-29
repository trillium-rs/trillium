use bat::{Input, PagingMode, PrettyPrinter};
use blocking::Unblock;
use futures_lite::io::BufReader;
use std::{borrow::Cow, io::ErrorKind, path::PathBuf, str::FromStr};
use structopt::StructOpt;
use trillium::http_types::{url, Body, Method, Url};
use trillium_client::{ClientTransport, Conn, Error, NativeTls, Rustls, TcpStream};

#[derive(StructOpt, Debug)]
pub struct ClientCli {
    #[structopt(parse(try_from_str = parse_method_case_insensitive))]
    method: Method,

    #[structopt(parse(try_from_str = parse_url))]
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
    #[structopt(short, long, parse(from_os_str), verbatim_doc_comment)]
    file: Option<PathBuf>,

    /// provide a request body on the command line
    ///
    /// example:
    /// trillium client post http://httpbin.org/post -b '{"hello": "world"}'
    #[structopt(short, long, verbatim_doc_comment)]
    body: Option<String>,

    /// provide headers in the form -h KEY1=VALUE1 KEY2=VALUE2
    ///
    /// example:
    /// trillium client get http://httpbin.org/headers -h Accept=application/json Authorization="Basic u:p"
    #[structopt(short, long, parse(try_from_str = parse_header), verbatim_doc_comment)]
    headers: Vec<(String, String)>,

    /// tls implementation. options: rustls, native-tls, none
    ///
    /// requests to https:// urls with `none` will fail
    #[structopt(short, long, default_value = "rustls", verbatim_doc_comment)]
    tls: TlsType,

    /// set the log level. add more flags for more verbosity
    ///
    /// example:
    /// trillium client get https://www.google.com -vvv # `trace` verbosity level
    #[structopt(short, long, parse(from_occurrences))]
    verbose: u64,
}

impl ClientCli {
    async fn build<T: ClientTransport>(&self) -> Conn<'_, T> {
        let mut conn = Conn::<T>::new(self.method, self.url.clone());
        for (name, value) in &self.headers {
            conn.request_headers().append(&name[..], &value[..]);
        }

        if let Some(path) = &self.file {
            let metadata = async_fs::metadata(path)
                .await
                .unwrap_or_else(|e| panic!("could not read file {:?} ({})", path, e));

            let file = async_fs::File::open(path)
                .await
                .unwrap_or_else(|e| panic!("could not read file {:?} ({})", path, e));

            conn.with_request_body(Body::from_reader(
                BufReader::new(file),
                Some(metadata.len()),
            ))
        } else if let Some(body) = &self.body {
            conn.with_request_body(&**body)
        } else if atty::isnt(atty::Stream::Stdin) {
            conn.with_request_body(Body::from_reader(
                BufReader::new(Unblock::new(std::io::stdin())),
                None,
            ))
        } else {
            conn
        }
    }

    fn run_with_transport<T: ClientTransport>(self) {
        futures_lite::future::block_on(async move {
            env_logger::Builder::new()
                .filter_level(match self.verbose {
                    0 => log::LevelFilter::Info,
                    1 => log::LevelFilter::Debug,
                    _ => log::LevelFilter::Trace,
                })
                .init();

            let mut conn = self.build::<T>().await;

            if let Err(e) = conn.send().await {
                match e {
                    Error::Io(io) if io.kind() == ErrorKind::ConnectionRefused => {
                        log::error!("could not reach {}", self.url)
                    }

                    _ => log::error!("protocol error:\n\n{}", e),
                }

                return;
            }

            if atty::is(atty::Stream::Stdout) {
                let body = conn.response_body().read_string().await.unwrap();

                let request_headers_as_string = format!("{:#?}", conn.request_headers());
                let headers = conn.response_headers();
                let response_headers_as_string = format!("{:#?}", headers);
                let content_type = headers.get("content-type").map(|c| c.as_str());
                let filename = match content_type.as_deref() {
                    Some("application/json") => "body.json", // bat can't sniff json for some reason
                    _ => self.url.path(),
                };

                let status_string = conn.status().unwrap().to_string();

                PrettyPrinter::new()
                    .paging_mode(PagingMode::QuitIfOneScreen)
                    .header(true)
                    .grid(true)
                    .inputs(vec![
                        Input::from_bytes(request_headers_as_string.as_bytes())
                            .name("request_headers.rs")
                            .title("request headers"),
                        Input::from_bytes(response_headers_as_string.as_bytes())
                            .name("response_headers.rs")
                            .title("response headers"),
                        Input::from_bytes(status_string.as_bytes())
                            .name("status")
                            .title("status"),
                        Input::from_bytes(body.as_bytes()).name(filename).title(
                            if let Some(content_type) = content_type {
                                Cow::Owned(format!("response body ({})", content_type))
                            } else {
                                Cow::Borrowed("response body")
                            },
                        ),
                    ])
                    .print()
                    .unwrap();
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

    pub fn run(self) {
        match self.tls {
            TlsType::None => {
                self.run_with_transport::<TcpStream>();
            }
            TlsType::Rustls => {
                self.run_with_transport::<Rustls<TcpStream>>();
            }
            TlsType::NativeTls => {
                self.run_with_transport::<NativeTls<TcpStream>>();
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
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
