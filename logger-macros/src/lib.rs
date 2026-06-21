#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    nonstandard_style,
    unused_qualifications
)]
#![warn(missing_docs, clippy::nursery, clippy::cargo)]
#![allow(clippy::must_use_candidate, clippy::module_name_repetitions)]
#![doc = include_str!("../README.md")]

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use std::collections::{HashMap, HashSet};
use syn::{
    Expr, Ident, LitStr, Token,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
};

/// Build a server-side log formatter from a `format_args!`-style string.
///
/// See the [`trillium_logger`](https://docs.rs/trillium-logger) crate documentation for usage.
#[proc_macro]
pub fn log_format(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as LogFormatInput);
    expand(parsed, &quote!(::trillium_logger::formatters))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Build a client-side log formatter from a `format_args!`-style string.
///
/// See the [`trillium_logger`](https://docs.rs/trillium-logger) crate documentation for usage.
#[proc_macro]
pub fn client_log_format(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as LogFormatInput);
    expand(parsed, &quote!(::trillium_logger::client::formatters))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

struct LogFormatInput {
    fmt: LitStr,
    named: Vec<(Ident, Expr)>,
    positional: Vec<Expr>,
}

enum Arg {
    Named(Ident, Expr),
    Positional(Expr),
}

impl Parse for Arg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(Ident) && input.peek2(Token![=]) {
            let name = input.parse()?;
            let _: Token![=] = input.parse()?;
            Ok(Self::Named(name, input.parse()?))
        } else {
            Ok(Self::Positional(input.parse()?))
        }
    }
}

impl Parse for LogFormatInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let fmt = input.parse()?;
        let mut named = Vec::new();
        let mut positional = Vec::new();
        if input.peek(Token![,]) {
            let _: Token![,] = input.parse()?;
            for arg in Punctuated::<Arg, Token![,]>::parse_terminated(input)? {
                match arg {
                    Arg::Named(ident, expr) => named.push((ident, expr)),
                    Arg::Positional(expr) => positional.push(expr),
                }
            }
        }
        Ok(Self {
            fmt,
            named,
            positional,
        })
    }
}

enum Piece {
    Lit(String),
    Named(String),
    Positional,
}

fn parse_format(s: &str, span: Span) -> syn::Result<Vec<Piece>> {
    let mut pieces = Vec::new();
    let mut lit = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                lit.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                lit.push('}');
            }
            '}' => {
                return Err(syn::Error::new(
                    span,
                    "unmatched `}` in format string; write `}}` for a literal brace",
                ));
            }
            '{' => {
                let mut name = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    name.push(c);
                }
                if !closed {
                    return Err(syn::Error::new(
                        span,
                        "unmatched `{` in format string; write `{{` for a literal brace",
                    ));
                }
                if !lit.is_empty() {
                    pieces.push(Piece::Lit(std::mem::take(&mut lit)));
                }
                let name = name.trim();
                if name.contains(':') {
                    return Err(syn::Error::new(
                        span,
                        "format specifiers are not supported in log formatters",
                    ));
                }
                pieces.push(if name.is_empty() {
                    Piece::Positional
                } else {
                    Piece::Named(name.to_string())
                });
            }
            _ => lit.push(c),
        }
    }
    if !lit.is_empty() {
        pieces.push(Piece::Lit(lit));
    }
    Ok(pieces)
}

fn expand(input: LogFormatInput, module: &TokenStream2) -> syn::Result<TokenStream2> {
    let span = input.fmt.span();
    let pieces = parse_format(&input.fmt.value(), span)?;
    let named: HashMap<String, &Expr> = input
        .named
        .iter()
        .map(|(ident, expr)| (ident.to_string(), expr))
        .collect();

    let mut used_named = HashSet::new();
    let mut positional = input.positional.iter();
    let mut elements = Vec::new();

    for piece in pieces {
        elements.push(match piece {
            Piece::Lit(s) => {
                let lit = LitStr::new(&s, span);
                quote!(#lit)
            }
            Piece::Positional => {
                let expr = positional.next().ok_or_else(|| {
                    syn::Error::new(
                        span,
                        "not enough positional arguments for `{}` placeholders",
                    )
                })?;
                quote!(#expr)
            }
            Piece::Named(name) => {
                if let Some(expr) = named.get(&name) {
                    used_named.insert(name);
                    quote!(#expr)
                } else {
                    let ident: Ident = syn::parse_str(&name).map_err(|_| {
                        syn::Error::new(
                            span,
                            format!(
                                "`{name}` is not a valid formatter name; supply it as a named \
                                 argument (`{name} = ...`)"
                            ),
                        )
                    })?;
                    quote!(#module::#ident)
                }
            }
        });
    }

    if positional.next().is_some() {
        return Err(syn::Error::new(
            span,
            "more positional arguments than `{}` placeholders",
        ));
    }

    if let Some((ident, _)) = input
        .named
        .iter()
        .find(|(i, _)| !used_named.contains(&i.to_string()))
    {
        return Err(syn::Error::new(
            ident.span(),
            format!("named argument `{ident}` is not referenced in the format string"),
        ));
    }

    Ok(nest(elements))
}

const MAX_TUPLE: usize = 26;

fn nest(mut elements: Vec<TokenStream2>) -> TokenStream2 {
    match elements.len() {
        0 => quote!(""),
        1 => elements.pop().unwrap(),
        n if n <= MAX_TUPLE => quote!((#(#elements),*)),
        _ => {
            let groups = elements
                .chunks(MAX_TUPLE)
                .map(|chunk| nest(chunk.to_vec()))
                .collect();
            nest(groups)
        }
    }
}
