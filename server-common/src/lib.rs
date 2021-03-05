use atomic_waker::AtomicWaker;
use futures_lite::Future;
use myco::{async_trait, BoxedTransport, Conn, Error, Handler, Transport};
use myco_http::Conn as HttpConn;
use std::marker::PhantomData;
use std::net::ToSocketAddrs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::Poll;

pub use myco_http::Stopper;
pub use myco_tls_common::Acceptor;

pub struct CloneCounterInner {
    count: AtomicUsize,
    waker: AtomicWaker,
}

pub struct CloneCounter(Arc<CloneCounterInner>);
impl CloneCounter {
    pub fn new() -> Self {
        Self(Arc::new(CloneCounterInner {
            count: AtomicUsize::new(0),
            waker: AtomicWaker::new(),
        }))
    }

    pub fn current(&self) -> usize {
        self.0.count.load(Ordering::SeqCst)
    }
}

impl Future for CloneCounter {
    type Output = ();

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if 0 == self.current() {
            return Poll::Ready(());
        } else {
            self.0.waker.register(cx.waker());
            if 0 == self.current() {
                return Poll::Ready(());
            } else {
                return Poll::Pending;
            }
        }
    }
}

impl Clone for CloneCounter {
    fn clone(&self) -> Self {
        self.0
            .count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(self.0.clone())
    }
}
impl Drop for CloneCounter {
    fn drop(&mut self) {
        self.0
            .count
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        self.0.waker.wake();
    }
}

pub struct Config<S, A, T> {
    acceptor: A,
    port: Option<u16>,
    host: Option<String>,
    transport: PhantomData<T>,
    server: PhantomData<S>,
    nodelay: bool,
    stopper: Stopper,
    counter: CloneCounter,
}

impl<S, A: Clone, T> Clone for Config<S, A, T> {
    fn clone(&self) -> Self {
        Self {
            acceptor: self.acceptor.clone(),
            port: self.port.clone(),
            host: self.host.clone(),
            transport: PhantomData,
            server: PhantomData,
            nodelay: self.nodelay,
            stopper: self.stopper.clone(),
            counter: self.counter.clone(),
        }
    }
}

impl<S, T> Default for Config<S, (), T> {
    fn default() -> Self {
        Self {
            acceptor: (),
            port: None,
            host: None,
            transport: PhantomData,
            server: PhantomData,
            nodelay: false,
            stopper: Stopper::new(),
            counter: CloneCounter::new(),
        }
    }
}

impl<S, T> Config<S, (), T> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<S: Server<Transport = T>, A: Acceptor<T>, T: Transport> Config<S, A, T> {
    pub fn acceptor(&self) -> &A {
        &self.acceptor
    }

    pub fn socket_addrs(&self) -> Vec<std::net::SocketAddr> {
        (self.host(), self.port())
            .to_socket_addrs()
            .unwrap()
            .collect()
    }

    pub fn host(&self) -> String {
        self.host
            .as_ref()
            .map(String::from)
            .or_else(|| std::env::var("HOST").ok())
            .unwrap_or_else(|| String::from("localhost"))
    }

    pub fn port(&self) -> u16 {
        self.port
            .or_else(|| std::env::var("PORT").ok().and_then(|p| p.parse().ok()))
            .unwrap_or(8080)
    }

    pub fn run<H: Handler>(self, h: H) {
        S::run(self, h)
    }

    pub fn counter(&self) -> &CloneCounter {
        &self.counter
    }

    pub async fn run_async(self, handler: impl Handler) {
        S::run_async(self, handler).await
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub fn with_host(mut self, host: &str) -> Self {
        self.host = Some(host.into());
        self
    }

    pub fn nodelay(&self) -> bool {
        self.nodelay
    }

    pub fn set_nodelay(mut self) -> Self {
        self.nodelay = true;
        self
    }

    pub fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }

    pub fn with_acceptor<A1: Acceptor<T>>(self, acceptor: A1) -> Config<S, A1, T> {
        Config {
            host: self.host,
            port: self.port,
            nodelay: self.nodelay,
            transport: PhantomData,
            server: PhantomData,
            stopper: self.stopper,
            acceptor,
            counter: self.counter,
        }
    }

    pub async fn graceful_shutdown(self) {
        let current = self.counter.current();
        if current > 0 {
            log::info!("waiting for {} in-flight requests to complete", current);
            self.counter.await;
            log::info!("all done!")
        }
    }

    pub async fn handle_stream(self, stream: T, handler: impl Handler) {
        let stream = match self.acceptor.accept(stream).await {
            Ok(stream) => stream,
            Err(e) => {
                log::error!("acceptor error: {:?}", e);
                return;
            }
        };

        let result = HttpConn::map(stream, self.stopper.clone(), |conn| async {
            let conn = Conn::new(conn);
            let conn = handler.run(conn).await;
            let conn = handler.before_send(conn).await;

            conn.into_inner()
        })
        .await;

        match result {
            Ok(Some(upgrade)) => {
                let upgrade = upgrade.map_transport(BoxedTransport::new);
                if handler.has_upgrade(&upgrade) {
                    log::debug!("upgrading...");
                    handler.upgrade(upgrade).await;
                } else {
                    log::error!("upgrade specified but no upgrade handler provided");
                }
            }

            Err(Error::ClosedByClient) | Err(Error::Shutdown) | Ok(None) => {
                log::debug!("closing connection");
            }

            Err(e) => {
                log::error!("http error: {:?}", e);
            }
        };
    }
}

#[async_trait]
pub trait Server: Sized {
    type Transport: Transport;
    fn run<A: Acceptor<Self::Transport>, H: Handler>(
        config: Config<Self, A, Self::Transport>,
        handler: H,
    );

    async fn run_async<A: Acceptor<Self::Transport>, H: Handler>(
        config: Config<Self, A, Self::Transport>,
        handler: H,
    );
}
