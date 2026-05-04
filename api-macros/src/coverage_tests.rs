use proc_macro2::TokenStream;

#[test]
fn code_coverage() {
    use std::{
        env,
        fs::{self, File},
        path::Path,
    };

    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let paths = vec![tests_dir.join("expand"), tests_dir.join("ui")];

    for path in paths {
        for file in fs::read_dir(path).unwrap() {
            let direntry = file.unwrap();
            let path = direntry.path();
            if path.extension().is_some_and(|x| x == "rs")
                && !path.to_string_lossy().contains(".expanded")
            {
                let file = File::open(&path).unwrap();
                let tfc: &dyn Fn(TokenStream) -> TokenStream =
                    &crate::try_from_conn::derive_internal;
                let h: &dyn Fn(TokenStream) -> TokenStream = &crate::handler::derive_internal;

                runtime_macros::emulate_derive_macro_expansion(
                    file,
                    &[
                        ("trillium_api_macros::TryFromConn", tfc),
                        ("trillimm_api_macros::Handler", h),
                        ("trillium_api::TryFromConn", tfc),
                        ("trillimm_api::Handler", h),
                        ("TryFromConn", tfc),
                        ("Handler", h),
                    ],
                )
                .unwrap();
            }
        }
    }
}
