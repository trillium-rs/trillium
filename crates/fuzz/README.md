# Fuzzers

This directory contains multiple fuzzers that test different parts of
Trillium's API. They use AFL++, via `afl-rs` and `cargo-afl`.

## Setup

`cargo-afl` is required to use these fuzzers. Run `cargo install cargo-afl` to
install it.

Some fuzzers require the use of nightly Rust toolchains, so this directory has
a `rust-toolchain.toml` file directing rustup to use that channel. If no
nightly toolchain is installed yet, you may need to run `rustup install
nightly`.

## Usage

Build the fuzzers using `cargo afl build`. Run a particular fuzzer with, for
example, `cargo afl fuzz -i in/received_body_hang/ -o out/received_body_hang/
target/debug/received_body_hang`. Crashing inputs will be stored in
`out/received_body_hang/default/crashes/`. Crashes can be reproduced by running
the fuzz target binary and piping the files into standard input, with `cargo
afl run --bin received_body_hang < FILENAME`. There are other `cargo afl`
subcommands to invoke different AFL tools, such as corpus minimization, test
case minimization, etc. See the [AFL section of the Rust Fuzz
Book](https://rust-fuzz.github.io/book/afl.html) and the [AFL++
documentation](https://github.com/AFLplusplus/AFLplusplus/blob/stable/README.md)
for more information.

## List of Fuzzers

### `received_body_hang`

This passes fuzzer input data in multiple chunks via an `AsyncRead`
implementation, and uses `trillium_http::ReceivedBody` to decode the data. It
is specifically targeted to identify inputs that cause `<ReceivedBody as
AsyncRead>::poll_read()` to get stuck, and never return `Ready`. A nightly
compiler is required, because it uses unstable rustc features to suppress
crashes due to allocation failures.

### `received_body`

This passes fuzzer input data in multiple chunks via an `AsyncRead`
implementation, and uses `trillium_http::ReceivedBody` to decode the data. It
is a more generic version of the `received_body_hang` fuzzer.
