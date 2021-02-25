use std::convert::TryInto;

use async_net::TcpStream;
use http_types::Request;
use myco::http_types::Body;
use myco::{async_trait, Conn, Handler};
use url::Url;

pub struct Proxy {
    target: Url,
}

macro_rules! conn_try {
    ($conn:expr, $expr:expr) => {
        conn_try!($conn, $expr, "error")
    };

    ($conn:expr, $expr:expr, $format_str:literal) => {
        match $expr {
            Ok(value) => value,
            Err(error) => {
                log::error!(concat!($format_str, ": {}"), error);
                return $conn.status(500);
            }
        }
    };
}

#[async_trait]
impl Handler for Proxy {
    async fn run(&self, mut conn: Conn) -> Conn {
        let socket = conn_try!(conn, self.target.socket_addrs(|| Some(80)));
        let request_url = conn_try!(conn, self.target.clone().join(conn.path()));
        let tcp_stream = conn_try!(conn, TcpStream::connect(&socket[..]).await);
        let mut request = Request::new(conn.method().to_string().parse().unwrap(), request_url);

        for (name, value) in conn.headers() {
            for value in value {
                request.append_header(name.as_str(), value.as_str());
            }
        }

        let mut response = conn_try!(conn, async_h1::connect(tcp_stream, request).await);
        if response.status() == 404 {
            conn
        } else {
            let response_body = response.take_body();
            let len = response_body.len().map(|s| s as u64);
            let body = Body::from_reader(response_body.into_reader(), len);

            for (name, value) in response.as_ref() {
                for value in value {
                    conn.headers_mut().append(name.as_str(), value.as_str());
                }
            }

            conn.body(body).status(response.status() as u16).halt()
        }
    }
}

impl Proxy {
    pub fn new(target: impl TryInto<Url>) -> Self {
        Self {
            target: match target.try_into() {
                Ok(url) => url,
                Err(_) => panic!("could not convert proxy target into a url"),
            },
        }
    }
}
