use async_compat::Compat;
use myco::{BoxedTransport, Conn, Grain, Sequence};

use myco_http::Conn as HttpConn;
use std::{
    io::Result,
    net::{SocketAddr, ToSocketAddrs},
    ops::Add,
    sync::Arc,
};
use tokio::{net::TcpListener, runtime::Runtime};

pub struct Server<G> {
    bind: Vec<SocketAddr>,
    grain: G,
}

impl<G: Grain> Server<G> {
    pub fn new(bind: impl ToSocketAddrs, grain: G) -> Result<Self> {
        Ok(Self {
            bind: bind.to_socket_addrs()?.collect(),
            grain,
        })
    }

    fn into_grain(self) -> G {
        self.grain
    }
}

impl Server<Sequence> {
    pub fn then<G: Grain>(mut self, rhs: G) -> Self {
        self.grain.then(rhs);
        self
    }

    pub fn sequence(bind: impl ToSocketAddrs) -> Result<Self> {
        Self::new(bind, Sequence::new())
    }
}

impl<G: Grain> Add<G> for Server<Sequence> {
    type Output = Self;

    fn add(self, rhs: G) -> Self::Output {
        self.then(rhs)
    }
}

impl<G: Grain> Server<G> {
    pub fn run(self) {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            let listener = TcpListener::bind(&self.bind[..]).await.unwrap();
            let mut grain = self.into_grain();
            grain.init().await;
            let grain = Arc::new(grain);

            loop {
                let result = listener.accept().await;
                if let Ok((socket, _)) = result {
                    let grain = grain.clone();
                    let result =
                        HttpConn::map(BoxedTransport::new(Compat::new(socket)), &|conn| async {
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
                                log::error!("upgrade specified but no upgrade handler provided");
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
            }
        });
    }
}
