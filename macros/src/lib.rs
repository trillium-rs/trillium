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
use quote::quote;
use std::{collections::HashSet, iter::once};
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input, parse_quote,
    punctuated::Punctuated,
    spanned::Spanned,
    token::{Comma, Where},
    visit::{visit_type_path, Visit},
    Attribute, Data, DeriveInput, Error, Expr, ExprArray, ExprAssign, ExprPath, Field, Ident,
    Index, Member, Meta, Path, Type, TypePath, WhereClause,
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Override {
    Run,
    Init,
    BeforeSend,
    HasUpgrade,
    Upgrade,
    Name,
}

impl TryFrom<&Path> for Override {
    type Error = Error;
    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        if path.is_ident("run") {
            Ok(Self::Run)
        } else if path.is_ident("init") {
            Ok(Self::Init)
        } else if path.is_ident("before_send") {
            Ok(Self::BeforeSend)
        } else if path.is_ident("has_upgrade") {
            Ok(Self::HasUpgrade)
        } else if path.is_ident("upgrade") {
            Ok(Self::Upgrade)
        } else if path.is_ident("name") {
            Ok(Self::Name)
        } else {
            Err(Error::new(
                path.span(),
                "unrecognized trillium::Handler function name",
            ))
        }
    }
}

struct DeriveOptions {
    overrides: Vec<Override>,
    input: DeriveInput,
    field: Field,
    field_index: usize,
}

fn overrides<'a, I: Iterator<Item = &'a Expr>>(iter: I) -> syn::Result<Vec<Override>> {
    iter.map(|expr| match expr {
        Expr::Path(ExprPath { path, .. }) => path.try_into(),
        _ => Err(Error::new(expr.span(), "unrecognized override. valid options are run, init, before_send, name, has_upgrade, and upgrade")),
    })
    .collect()
}

fn parse_attribute(attr: &Attribute) -> syn::Result<Option<Vec<Override>>> {
    if attr.path().is_ident("handler") {
        match &attr.meta {
            Meta::Path(_) => Ok(Some(vec![])),
            Meta::List(metalist) => {
                let tokens = metalist.tokens.clone();
                let ExprAssign { left, right, .. } = syn::parse(tokens.into())?;
                match (*left, *right) {
                    (Expr::Path(ExprPath { path: left, .. }), right @ Expr::Path(_))
                        if left.is_ident("overrides") =>
                    {
                        Ok(Some(overrides(once(&right))?))
                    }

                    (
                        Expr::Path(ExprPath { path: left, .. }),
                        Expr::Array(ExprArray { elems: right, .. }),
                    ) if left.is_ident("overrides") => Ok(Some(overrides(right.iter())?)),

                    (_x, _y) => Err(Error::new(
                        metalist.span(),
                        "unrecognized #[handler] attributes",
                    )),
                }
            }
            Meta::NameValue(nv) => Err(Error::new(nv.span(), "unrecognized #[handler] attributes")),
        }
    } else {
        Ok(None)
    }
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
        let Data::Struct(ds) = &input.data else { return Err(Error::new(input.span(), "second erro")) };

        for (field_index, field) in ds.fields.iter().enumerate() {
            for attr in &field.attrs {
                if let Some(overrides) = parse_attribute(attr)? {
                    let field = field.clone();
                    return Ok(Self {
                        overrides,
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
                overrides: vec![],
                input,
                field,
                field_index: 0,
            })
        } else {
            Err(Error::new(
                input.span(),
                "Structs with more than one field need a #[handler] annotation",
            ))
        }
    }
}

/// see crate docs
#[proc_macro_derive(Handler, attributes(handler))]
pub fn derive_handler(input: TokenStream) -> TokenStream {
    let DeriveOptions {
        overrides,
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
            .push_value(parse_quote! { #generic: trillium::Handler });
        where_clause.predicates.push_punct(Comma::default());
    }

    where_clause
        .predicates
        .push_value(parse_quote! { Self: Send + Sync + 'static });

    let handler = field
        .ident
        .map_or_else(|| Member::Unnamed(Index::from(field_index)), Member::Named);

    let handler = quote!(self.#handler);

    let run = if overrides.contains(&Override::Run) {
        quote!(Self::run(&self, conn))
    } else {
        quote!(trillium::Handler::run(&#handler, conn))
    };

    let init = if overrides.contains(&Override::Init) {
        quote!(Self::init(&mut self, info))
    } else {
        quote!(trillium::Handler::init(&mut #handler, info))
    };

    let before_send = if overrides.contains(&Override::BeforeSend) {
        quote!(Self::before_send(&self, conn))
    } else {
        quote!(trillium::Handler::before_send(&#handler, conn))
    };

    let name = if overrides.contains(&Override::Name) {
        quote!(Self::name(&self))
    } else {
        let name_string = struct_name.to_string();
        quote!(format!("{} ({})", #name_string, trillium::Handler::name(&#handler)).into())
    };

    let has_upgrade = if overrides.contains(&Override::HasUpgrade) {
        quote!(Self::has_upgrade(&self, upgrade))
    } else {
        quote!(trillium::Handler::has_upgrade(&#handler, upgrade))
    };

    let upgrade = if overrides.contains(&Override::Upgrade) {
        quote!(Self::upgrade(&self, upgrade))
    } else {
        quote!(trillium::Handler::upgrade(&#handler, upgrade))
    };

    quote! {
        #[trillium::async_trait]
        impl #impl_generics trillium::Handler for #struct_name #ty_generics #where_clause {
            async fn run(&self, conn: trillium::Conn) -> trillium::Conn { #run.await }
            async fn init(&mut self, info: &mut trillium::Info) { #init.await; }
            async fn before_send(&self, conn: trillium::Conn) -> trillium::Conn { #before_send.await }
            fn name(&self) -> std::borrow::Cow<'static, str> { #name }
            fn has_upgrade(&self, upgrade: &trillium::Upgrade) -> bool { #has_upgrade }
            async fn upgrade(&self, upgrade: trillium::Upgrade) { #upgrade.await }
        }
    }
    .into()
}
