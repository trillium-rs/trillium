#[test]
#[cfg_attr(
    not(macrotest),
    ignore = "to run macrotests in an environment with `cargo expand` enable \
              `RUSTFLAGS=\"--cfg=macrotest\"`"
)]
fn macrotest() {
    println!("`MACROTEST=overwrite cargo test` to accept");
    macrotest::expand("tests/expand/expand_*.rs");
}

#[rustversion::stable]
#[test]
fn ui_tests() {
    ui_tests_impl()
}

#[rustversion::not(stable)]
#[test]
#[ignore = "to run ui tests, use the stable toolchain"]
fn ui_tests() {
    ui_tests_impl()
}

fn ui_tests_impl() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
