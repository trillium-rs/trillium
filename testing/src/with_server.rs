cfg_if::cfg_if! {
    if #[cfg(feature = "smol")] {
        mod smol;
        pub use smol::with_server;
    } else if #[cfg(feature = "async-std")] {
        mod async_std;
        pub use async_std::with_server;
    } else if #[cfg(feature = "tokio")] {
        mod tokio;
        pub use tokio::with_server;
   } else {
        pub fn with_server<H, Fun, Fut>(_handler: H, _tests: Fun)
        where
            H: trillium::Handler,
            Fun: Fn(crate::Url) -> Fut,
            Fut: std::future::Future<Output = Result<(), Box<dyn std::error::Error>>>,
        {
            panic!()
        }
    }
}
