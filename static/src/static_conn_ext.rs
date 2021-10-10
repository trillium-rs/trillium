use crate::{fs_shims::File, options::StaticOptions};
use etag::EntityTag;
use std::path::Path;
use trillium::{
    Body, Conn,
    KnownHeaderName::{self, ContentType},
};

/// conn extension trait to facilitate sending individual files and
/// paths
#[trillium::async_trait]
pub trait StaticConnExt {
    /// Send the file at the provided path. Will send a 404 if the
    /// file cannot be resolved or if it is a directory.
    async fn send_path<A: AsRef<Path> + Send>(self, path: A) -> Self;

    /// Send the file at the provided path. Will send a 404 if the
    /// file cannot be resolved or if it is a directory.
    async fn send_file(self, file: File) -> Self;

    /// Send the file at the provided path. Will send a 404 if the
    /// file cannot be resolved or if it is a directory.
    async fn send_file_with_options(self, file: File, options: &StaticOptions) -> Self;

    /// Send the file at the provided path. Will send a 404 if the
    /// file cannot be resolved or if it is a directory.
    async fn send_path_with_options<A: AsRef<Path> + Send>(
        self,
        path: A,
        options: &StaticOptions,
    ) -> Self;

    /// Guess the mime type for this fs path using
    /// [`mime_guess`](https://docs.rs/mime_guess/) and set the
    /// content-type header
    fn with_mime_from_path(self, path: impl AsRef<Path>) -> Self;
}

#[trillium::async_trait]
impl StaticConnExt for Conn {
    async fn send_path<A: AsRef<Path> + Send>(mut self, path: A) -> Self {
        self.send_path_with_options(path, &StaticOptions::default())
            .await
    }

    async fn send_file(mut self, file: File) -> Self {
        self.send_file_with_options(file, &StaticOptions::default())
            .await
    }

    async fn send_path_with_options<A: AsRef<Path> + Send>(
        mut self,
        path: A,
        options: &StaticOptions,
    ) -> Self {
        let path = path.as_ref().to_path_buf();
        let file = trillium::conn_try!(File::open(&path).await, self.with_status(404));
        self.send_file_with_options(file, options)
            .await
            .with_mime_from_path(path)
    }

    async fn send_file_with_options(mut self, file: File, options: &StaticOptions) -> Self {
        let metadata = trillium::conn_try!(file.metadata().await, self.with_status(404));

        if options.modified {
            if let Ok(last_modified) = metadata.modified() {
                self.headers_mut().try_insert(
                    KnownHeaderName::LastModified,
                    httpdate::fmt_http_date(last_modified),
                );
            }
        }

        if options.etag {
            let etag = EntityTag::from_file_meta(&metadata);
            self.headers_mut()
                .try_insert(KnownHeaderName::Etag, etag.to_string());
        }

        #[cfg(all(feature = "tokio", not(feature = "smol"), not(feature = "async-std")))]
        let file = async_compat::Compat::new(file);

        self.ok(Body::new_streaming(file, Some(metadata.len())))
    }

    fn with_mime_from_path(self, path: impl AsRef<Path>) -> Self {
        if let Some(mime) = mime_guess::from_path(path).first() {
            use mime_guess::mime::{APPLICATION, HTML, JAVASCRIPT, TEXT};
            let is_text = matches!(
                (mime.type_(), mime.subtype()),
                (APPLICATION, JAVASCRIPT) | (TEXT, _) | (_, HTML)
            );

            self.with_header(
                ContentType,
                if is_text {
                    format!("{}; charset=utf-8", mime)
                } else {
                    mime.to_string()
                },
            )
        } else {
            self
        }
    }
}
