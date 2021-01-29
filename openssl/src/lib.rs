use async_net::TcpListener;
use myco::{BoxedTransport, Conn, Grain};
use myco_http::Conn as HttpConn;
use smol::prelude::*;
use std::{
    net::{SocketAddr, ToSocketAddrs},
    sync::Arc,
};

pub struct Server {
    bind: Vec<SocketAddr>,
    key: Vec<u8>,
    password: String,
}

impl Server {
    pub fn new(bind: impl ToSocketAddrs, key: Vec<u8>, password: String) -> Self {
        Self {
            bind: bind.to_socket_addrs().expect("could not bind").collect(),
            key,
            password,
        }
    }

    fn spawn<F: Future<Output = ()> + Send + 'static>(&self, f: F) {
        smol::spawn(f).detach();
    }

    pub async fn run_async(self, mut grain: impl Grain) {
        let listener = TcpListener::bind(&self.bind[..]).await.unwrap();
        let mut incoming = listener.incoming();
        grain.init().await;
        let grain = Arc::new(grain);

        while let Some(Ok(stream)) = incoming.next().await {
            let grain = grain.clone();
            let key = smol::io::Cursor::new(self.key.clone());
            let password = self.password.clone();
            self.spawn(async move {
                match async_native_tls::accept(key, password, stream).await {
                    Ok(stream) => {
                        let result = HttpConn::map(BoxedTransport::new(stream), &|conn| async {
                            let conn = Conn::new(conn);
                            let conn = grain.run(conn).await;
                            let conn = grain.before_send(conn).await;
                            conn.into_inner()
                        })
                        .await;

                        match result {
                            Ok(Some(upgrade)) => {
                                if grain.has_upgrade(&upgrade) {
                                    log::debug!("upgrading");
                                    grain.upgrade(upgrade).await;
                                } else {
                                    log::error!(
                                        "upgrade specified but no upgrade handler provided"
                                    );
                                }
                            }

                            Ok(None) => {
                                log::info!("closing");
                            }

                            Err(e) => {
                                log::error!("{:?}", e);
                            }
                        };
                    }
                    Err(e) => log::error!("tls error: {:?}", e),
                }
            });
        }
    }

    pub fn run(self, grain: impl Grain) {
        smol::block_on(async move { self.run_async(grain).await })
    }
}
