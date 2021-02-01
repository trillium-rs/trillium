use lamedh_runtime::Context;
use std::ops::Deref;
pub struct LambdaContext(Context);
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

pub trait LambdaConnExt {
    fn lambda_context(&self) -> &Context;
}

impl LambdaConnExt for myco::Conn {
    fn lambda_context(&self) -> &Context {
        &*self
            .state::<LambdaContext>()
            .expect("lambda context should always be set inside of a lambda server")
    }
}
