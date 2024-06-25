use async_net::{TcpListener, TcpStream};
use futures_lite::prelude::*;
use std::thread;
use trillium_http::{Conn, Swansong};

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
    let swansong = Swansong::new();
    let (send, receive) = async_channel::unbounded();
    let core_ids = core_affinity::get_core_ids().unwrap();
    let handles = core_ids
        .into_iter()
        .map(|id| {
            let swansong = swansong.clone();
            let receive = receive.clone();
            thread::spawn(move || {
                if !core_affinity::set_for_current(id) {
                    log::warn!("unable to set core affinity");
                }
                let executor = async_executor::LocalExecutor::new();

                futures_lite::future::block_on(executor.run(async {
                    while let Ok(transport) = receive.recv().await {
                        let swansong = swansong.clone();

                        let future = async move {
                            match Conn::map(transport, swansong, handler).await {
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
