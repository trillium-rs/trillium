cfg_if::cfg_if! {
    if #[cfg(feature = "tokio")] {
       pub(crate) use tokio_crate::fs::{self, File};
    } else if #[cfg(feature = "async-std")] {
        pub(crate) use async_std_crate::fs::{self, File};
    } else if #[cfg(feature = "smol")] {
        pub(crate) use async_fs::{self as fs, File};
    } else {
        #[derive(Debug, Clone, Copy)]
        pub struct File;
        impl File {
            pub(crate) async fn open(_path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
                unimplemented!("please enable the tokio, async-std, or smol runtime feature")
            }

            pub(crate) async fn metadata(&self) -> std::io::Result<std::fs::Metadata> {
                unimplemented!("please enable the tokio, async-std, or smol runtime feature")
            }
        }

        impl futures_lite::AsyncRead for File {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
                _buf: &mut [u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                unimplemented!("please enable the tokio, async-std, or smol runtime feature")

            }
        }



        pub(crate) mod fs {
            pub(crate) async fn canonicalize(_path: impl AsRef<std::path::Path>) -> std::io::Result<std::path::PathBuf> {
                unimplemented!("please enable the tokio, async-std, or smol runtime feature")
            }

            pub(crate) async fn metadata(_path: impl AsRef<std::path::Path>) -> std::io::Result<std::fs::Metadata> {
                unimplemented!("please enable the tokio, async-std, or smol runtime feature")
            }
        }
    }
}
