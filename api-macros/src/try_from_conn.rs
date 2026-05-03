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

    let (error_ty, body) = match attrs.source {
        Source::State => {
            let take = if attrs.clone {
                quote! { conn.state::<Self>().cloned() }
            } else {
                quote! { conn.take_state::<Self>() }
            };
            match &attrs.err {
                Some(err) => (
                    quote!(#err),
                    quote! {
                        #take.ok_or_else(<#err as ::core::default::Default>::default)
                    },
                ),
                None => (quote!(()), quote! { #take.ok_or(()) }),
            }
        }

        Source::Json => {
            if attrs.clone {
                return Err(Error::new(
                    attrs.source_span,
                    "`clone` is only meaningful with `state`",
                ));
            }
            let call = quote! {
                ::trillium_api::ApiConnExt::deserialize_json::<Self>(conn).await
            };
            match &attrs.err {
                Some(err) => (
                    quote!(#err),
                    quote! {
                        #call.map_err(|_| <#err as ::core::default::Default>::default())
                    },
                ),
                None => (quote!(::trillium_api::Error), call),
            }
        }

        Source::Body => {
            if attrs.clone {
                return Err(Error::new(
                    attrs.source_span,
                    "`clone` is only meaningful with `state`",
                ));
            }
            let call = quote! {
                ::trillium_api::ApiConnExt::deserialize::<Self>(conn).await
            };
            match &attrs.err {
                Some(err) => (
                    quote!(#err),
                    quote! {
                        #call.map_err(|_| <#err as ::core::default::Default>::default())
                    },
                ),
                None => (quote!(::trillium_api::Error), call),
            }
        }
    };

    Ok(quote! {
        impl #impl_generics ::trillium_api::TryFromConn for #name #ty_generics #where_clause {
            type Error = #error_ty;

            async fn try_from_conn(
                conn: &mut ::trillium::Conn,
            ) -> ::core::result::Result<Self, Self::Error> {
                #body
            }
        }
    })
}
