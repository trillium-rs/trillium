use proc_macro::TokenStream;
use quote::quote;
use std::collections::HashSet;
use syn::{
    Data, DeriveInput, Error, Field, Ident, Index, Member, Type, TypePath, WhereClause,
    parse::{Parse, ParseStream},
    parse_macro_input, parse_quote,
    punctuated::Punctuated,
    spanned::Spanned,
    token::{Comma, Where},
    visit::{Visit, visit_type_path},
};

fn is_required_generic_for_type(ty: &Type, generic: &Ident) -> bool {
    struct PathVisitor<'g> {
        generic: &'g Ident,
        generic_is_required: bool,
    }
    impl<'g, 'ast> Visit<'ast> for PathVisitor<'g> {
        fn visit_type_path(&mut self, node: &'ast TypePath) {
            if node.qself.is_none() {
                if let Some(first_segment) = node.path.segments.first() {
                    if first_segment.ident == *self.generic {
                        self.generic_is_required = true;
                    }
                }
            }
            visit_type_path(self, node);
        }
    }

    let mut path_visitor = PathVisitor {
        generic,
        generic_is_required: false,
    };

    path_visitor.visit_type(ty);

    path_visitor.generic_is_required
}

struct DeriveOptions {
    input: DeriveInput,
    field: Field,
    field_index: usize,
}

fn generics(field: &Field, input: &DeriveInput) -> Vec<Ident> {
    input
        .generics
        .type_params()
        .filter_map(|g| {
            if is_required_generic_for_type(&field.ty, &g.ident) {
                Some(g.ident.clone())
            } else {
                None
            }
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

impl Parse for DeriveOptions {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let input = DeriveInput::parse(input)?;
        let Data::Struct(ds) = &input.data else {
            return Err(Error::new(input.span(), "second error"));
        };

        for (field_index, field) in ds.fields.iter().enumerate() {
            for attr in &field.attrs {
                if attr.path().is_ident("async_write") || attr.path().is_ident("async_io") {
                    let field = field.clone();
                    return Ok(Self {
                        input,
                        field,
                        field_index,
                    });
                }
            }
        }

        if ds.fields.len() == 1 {
            let field = ds
                .fields
                .iter()
                .next()
                .expect("len == 1 should have one element")
                .clone();
            Ok(Self {
                input,
                field,
                field_index: 0,
            })
        } else {
            Err(Error::new(
                input.span(),
                "Structs with more than one field need an #[async_io] or #[async_write] annotation",
            ))
        }
    }
}

pub fn derive_async_write(input: TokenStream) -> TokenStream {
    let DeriveOptions {
        field,
        input,
        field_index,
    } = parse_macro_input!(input as DeriveOptions);

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let generics = generics(&field, &input);

    let struct_name = input.ident;

    let mut where_clause = where_clause.map_or_else(
        || WhereClause {
            where_token: Where::default(),
            predicates: Punctuated::new(),
        },
        |where_clause| where_clause.to_owned(),
    );

    for generic in generics {
        where_clause
            .predicates
            .push_value(parse_quote! { #generic: AsyncWrite + Unpin });
        where_clause.predicates.push_punct(Comma::default());
    }

    where_clause
        .predicates
        .push_value(parse_quote! { Self: Unpin });

    let handler = field
        .ident
        .map_or_else(|| Member::Unnamed(Index::from(field_index)), Member::Named);

    let handler = quote!(self.#handler);

    quote! {
        impl #impl_generics AsyncWrite for #struct_name #ty_generics #where_clause {
            fn poll_write(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                buf: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::pin::Pin::new(&mut #handler).poll_write(cx, buf)
            }

            fn poll_flush(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::pin::Pin::new(&mut #handler).poll_flush(cx)
            }

            fn poll_close(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::pin::Pin::new(&mut #handler).poll_close(cx)
            }

            fn poll_write_vectored(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                bufs: &[std::io::IoSlice<'_>]
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::pin::Pin::new(&mut #handler).poll_write_vectored(cx, bufs)
            }
        }
    }
    .into()
}
