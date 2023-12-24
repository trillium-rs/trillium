#[derive(Debug, Clone, Copy)]
pub struct StaticOptions {
    pub(crate) etag: bool,
    pub(crate) modified: bool,
}

impl StaticOptions {
    pub fn without_etag(mut self) -> Self {
        self.etag = false;
        self
    }

    pub fn without_modified(mut self) -> Self {
        self.modified = false;
        self
    }
}

impl Default for StaticOptions {
    fn default() -> Self {
        Self {
            etag: true,
            modified: true,
        }
    }
}
