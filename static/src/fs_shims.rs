cfg_if::cfg_if! {
   if #[cfg(feature = "smol")] {
       pub(crate) use async_fs::{self as fs, File};
    } else if #[cfg(feature = "tokio")] {
       pub(crate) use tokio_crate::fs::{self, File};
    } else if #[cfg(feature = "async-std")] {
        pub(crate) use async_std_crate::fs::{self, File};
    } else {
        compile_error!("trillium-static:
You must enable one of the three runtime feature flags
to use this crate:

* tokio
* async-std
* smol

This is a temporary constraint, and hopefully soon this
will not require the use of cargo feature flags."
);
    }
}
