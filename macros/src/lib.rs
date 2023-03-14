#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    nonstandard_style,
    unused_qualifications
)]
#![warn(missing_docs, clippy::pedantic, clippy::nursery, clippy::cargo)]
#![allow(clippy::must_use_candidate, clippy::module_name_repetitions)]

/*!
# Welcome to the `trillium-macros` crate!

This crate currently offers a derive macro for Handler that can be
used to delegate Handler behavior to a contained Handler
type. Currently it only works for structs, but will eventually support
enums as well. Note that it will only delegate to a single inner Handler type.

In the case of a newtype struct or named struct with only a single
field, `#[derive(Handler)]` is all that's required. If there is more
than one field in the struct, annotate exactly one of them with
#[handler].


```rust

// for these examples, we are using a `&'static str` as the handler type.

use trillium_macros::Handler;
# fn assert_handler(_h: impl trillium::Handler) {}

#[derive(Handler)]
struct NewType(&'static str);
assert_handler(NewType("yep"));

#[derive(Handler)]
struct TwoTypes(usize, #[handler] &'static str);
assert_handler(TwoTypes(2, "yep"));

#[derive(Handler)]
struct NamedSingleField {
    this_is_the_handler: &'static str,
}
assert_handler(NamedSingleField { this_is_the_handler: "yep" });


#[derive(Handler)]
struct NamedMultiField {
    not_handler: usize,
    #[handler]
    inner_handler: &'static str,
    also_not_handler: usize,
}

assert_handler(NamedMultiField {
    not_handler: 1,
    inner_handler: "yep",
    also_not_handler: 3,
});

#[derive(Handler)]
struct Generic<G: trillium::Handler>(G);
assert_handler(Generic("hi"));
assert_handler(Generic(trillium::Status::Ok));


```
*/
use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Index};

/// see crate docs
#[proc_macro_derive(Handler, attributes(handler))]
pub fn derive_handler(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let name = input.ident;

    let handler = match input.data {
        Data::Struct(ds) => {
            if ds.fields.len() == 1 {
                ds.fields
                    .into_iter()
                    .next()
                    .expect("len == 1 should have only one element")
                    .ident
                    .map(|field| quote!(self.#field))
                    .unwrap_or_else(|| quote!(self.0))
            } else {
                ds
                    .fields
                    .into_iter()
                    .enumerate()
                    .find_map(|(n, f)| {
                        if f.attrs.iter().any(|attr| attr.path.is_ident("handler")) {
                            Some(f.ident.map(|ident| quote!(self.#ident)).unwrap_or_else(|| {
                                let n = Index::from(n);
                                quote!(self.#n)
                            }))
                        } else {
                            None
                        }
                    })
                    .expect("for structs with more than one field, please annotate one of them with #[handler]")
            }
        }
        _ => panic!("Only structs are currently supported by derive(Handler). Enums coming soon!"),
    };

    let name_string = name.to_string();

    quote! {
        #[trillium::async_trait]
        impl #impl_generics trillium::Handler for #name #ty_generics #where_clause {
            async fn run(&self, conn: trillium::Conn) -> trillium::Conn {
                trillium::Handler::run(&#handler, conn).await
            }

            async fn init(&mut self, info: &mut trillium::Info) {
                trillium::Handler::init(&mut #handler, info).await;
            }

            async fn before_send(&self, conn: trillium::Conn) -> trillium::Conn {
                trillium::Handler::before_send(&#handler, conn).await
            }

            fn name(&self) -> std::borrow::Cow<'static, str> {
                format!("{} ({})", #name_string, trillium::Handler::name(&#handler)).into()
            }

            fn has_upgrade(&self, upgrade: &trillium::Upgrade) -> bool {
                trillium::Handler::has_upgrade(&#handler, upgrade)
            }

            async fn upgrade(&self, upgrade: trillium::Upgrade) {
                trillium::Handler::upgrade(&#handler, upgrade).await;
            }
        }
    }
    .into()
}
