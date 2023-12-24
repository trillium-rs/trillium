use lamedh_runtime::Context;
use std::ops::Deref;

pub(crate) struct LambdaContext(Context);
impl LambdaContext {
    pub fn new(context: Context) -> Self {
        Self(context)
    }
}

impl Deref for LambdaContext {
    type Target = Context;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/**
Provides access to the aws lambda context for [`trillium::Conn`].

See [`lamedh_runtime::Context`] for more details on the data available
on this struct.
*/
pub trait LambdaConnExt {
    /// returns the [`lamedh_runtime::Context`] for this conn
    fn lambda_context(&self) -> &Context;
}

impl LambdaConnExt for trillium::Conn {
    fn lambda_context(&self) -> &Context {
        self.state::<LambdaContext>()
            .expect("lambda context should always be set inside of a lambda server")
    }
}
