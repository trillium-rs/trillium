cfg_if::cfg_if! {
    if #[cfg(feature = "smol")] {
        mod smol;
        pub use smol::with_server;
        use smol::tcp_connect;
    } else if #[cfg(feature = "async-std")] {
        mod async_std;
        use async_std::tcp_connect;
        pub use async_std::with_server;
    } else if #[cfg(feature = "tokio")] {
        mod tokio;
        use tokio::tcp_connect;
        pub use tokio::with_server;
   } else {
        pub fn with_server<H, Fun, Fut>(_handler: H, _tests: Fun)
        where
            H: trillium::Handler,
            Fun: FnOnce(crate::Url) -> Fut,
            Fut: std::future::Future<Output = Result<(), Box<dyn std::error::Error>>>,
        {
            panic!()
        }

        pub(crate) async fn tcp_connect(
            url: &crate::Url,
        ) -> std::io::Result<trillium_http::transport::BoxedTransport> {
            unimplemented!()
        }
    }
}

/// start a trillium server on a random port for the provided handler,
/// establish a tcp connection, run the provided test function with
/// that tcp stream, and shut down the server.
pub fn with_socket<H, Fun, Fut>(handler: H, tests: Fun)
where
    H: trillium::Handler,
    Fun: FnOnce(trillium_http::transport::BoxedTransport) -> Fut,
    Fut: std::future::Future<Output = Result<(), Box<dyn std::error::Error>>>,
{
    with_server(handler, move |url| async move {
        let tcp = tcp_connect(&url).await?;
        tests(tcp).await
    })
}
