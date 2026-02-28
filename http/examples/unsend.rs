use async_net::{TcpListener, TcpStream};
use futures_lite::prelude::*;
use std::{sync::Arc, thread};
use trillium_http::{Conn, ServerConfig, Swansong};

async fn handler(mut conn: Conn<TcpStream>) -> Conn<TcpStream> {
    let rc = std::rc::Rc::new(());
    conn.set_status(200);
    std::future::ready(()).await;
    conn.set_response_body("ok");
    let _ = rc.clone();
    conn
}

pub fn main() {
    env_logger::init();
    let server_config = Arc::new(ServerConfig::new());
    let (send, receive) = async_channel::unbounded();
    let core_ids = core_affinity::get_core_ids().unwrap();

    let swansong = Swansong::new();
    let handles = core_ids
        .into_iter()
        .map(|id| {
            let server_config = server_config.clone();
            let receive = receive.clone();
            thread::spawn(move || {
                if !core_affinity::set_for_current(id) {
                    log::warn!("unable to set core affinity");
                }
                let executor = async_executor::LocalExecutor::new();

                async_io::block_on(executor.run(async {
                    while let Ok(transport) = receive.recv().await {
                        let server_config = server_config.clone();
                        let future = async move {
                            match server_config.run(transport, handler).await {
                                Ok(_) => {}
                                Err(e) => log::error!("{e}"),
                            }
                        };
                        executor.spawn(future).detach();
                    }
                }));
            })
        })
        .collect::<Vec<_>>();

    async_io::block_on(async move {
        let port = std::env::var("PORT")
            .unwrap_or("8080".into())
            .parse::<u16>()
            .unwrap();

        let listener = TcpListener::bind(("0.0.0.0", port)).await.unwrap();
        let mut incoming = swansong.interrupt(listener.incoming());
        while let Some(Ok(stream)) = incoming.next().await {
            send.send(stream).await.unwrap();
        }
    });

    for handle in handles {
        handle.join().unwrap();
    }
}
