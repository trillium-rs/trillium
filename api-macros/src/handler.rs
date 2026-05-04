use crate::attributes::{ApiAttributes, Source};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{DeriveInput, Error};

pub fn derive(input: TokenStream) -> TokenStream {
    derive_internal(input.into()).into()
}

pub fn derive_internal(input: TokenStream2) -> TokenStream2 {
    match syn::parse2(input) {
        Ok(di) => expand(&di).unwrap_or_else(Error::into_compile_error),
        Err(e) => e.into_compile_error(),
    }
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let attrs = ApiAttributes::from_attrs(&input.attrs)?;

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let body = match attrs.source {
        Source::State => quote! {
            conn.with_state(<Self as ::core::clone::Clone>::clone(self))
        },
        Source::Json => quote! {
            ::trillium_api::ApiConnExt::with_json(conn, &*self)
        },
        Source::Body => quote! {
            match ::trillium_api::ApiConnExt::serialize(&mut conn, &*self).await {
                ::core::result::Result::Ok(()) => conn,
                ::core::result::Result::Err(e) => conn.with_state(e).halt(),
            }
        },
    };

    let run = match attrs.source {
        Source::Body => quote! {
            async fn run(&self, mut conn: ::trillium::Conn) -> ::trillium::Conn {
                #body
            }

            async fn before_send(&self, mut conn: ::trillium::Conn) -> ::trillium::Conn {
                if let ::core::option::Option::Some(error) =
                    conn.take_state::<::trillium_api::Error>()
                {
                    ::trillium::Handler::before_send(&error, conn).await
                } else {
                    conn
                }
            }
        },
        _ => quote! {
            async fn run(&self, conn: ::trillium::Conn) -> ::trillium::Conn {
                #body
            }
        },
    };

    Ok(quote! {
        impl #impl_generics ::trillium::Handler for #name #ty_generics #where_clause {
            #run
        }
    })
}
