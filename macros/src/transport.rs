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
    SetLinger,
    SetNodelay,
    SetIpTtl,
    PeerAddr,
}

impl TryFrom<&Path> for Override {
    type Error = Error;
    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        if path.is_ident("set_linger") {
            Ok(Self::SetLinger)
        } else if path.is_ident("set_nodelay") {
            Ok(Self::SetNodelay)
        } else if path.is_ident("set_ip_ttl") {
            Ok(Self::SetIpTtl)
        } else if path.is_ident("peer_addr") {
            Ok(Self::PeerAddr)
        } else {
            Err(Error::new(
                path.span(),
                "unrecognized Transport method name",
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
        _ => Err(Error::new(expr.span(), "unrecognized override. valid options are set_linger, set_nodelay, set_ip_ttl, and peer_addr")),
    })
    .collect()
}

fn parse_attribute(attr: &Attribute) -> syn::Result<Option<Vec<Override>>> {
    if attr.path().is_ident("transport") {
        match &attr.meta {
            Meta::Path(_) => Ok(Some(vec![])),
            Meta::List(metalist) => {
                let tokens = metalist.tokens.clone();
                let ExprAssign { left, right, .. } = syn::parse(tokens.into())?;
                match (*left, *right) {
                    (Expr::Path(ExprPath { path: left, .. }), right @ Expr::Path(_))
                        if left.is_ident("except") =>
                    {
                        Ok(Some(overrides(once(&right))?))
                    }

                    (
                        Expr::Path(ExprPath { path: left, .. }),
                        Expr::Array(ExprArray { elems: right, .. }),
                    ) if left.is_ident("except") => Ok(Some(overrides(right.iter())?)),

                    (_x, _y) => Err(Error::new(
                        metalist.span(),
                        "unrecognized #[transport] attributes",
                    )),
                }
            }
            Meta::NameValue(nv) => Err(Error::new(
                nv.span(),
                "unrecognized #[transport] attributes",
            )),
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
        let Data::Struct(ds) = &input.data else {
            return Err(Error::new(input.span(), "second error"));
        };

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
                "Structs with more than one field need a #[transport] annotation",
            ))
        }
    }
}

pub fn derive_transport(input: TokenStream) -> TokenStream {
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
            .push_value(parse_quote! { #generic: trillium_server_common::Transport });
        where_clause.predicates.push_punct(Comma::default());
    }

    where_clause
        .predicates
        .push_value(parse_quote! { Self: Send + Sync + 'static });

    let transport = field
        .ident
        .map_or_else(|| Member::Unnamed(Index::from(field_index)), Member::Named);

    let transport = quote!(self.#transport);

    let set_linger = if overrides.contains(&Override::SetLinger) {
        quote!(Self::set_linger(self, linger))
    } else {
        quote!(trillium_server_common::Transport::set_linger(&mut #transport, linger))
    };

    let set_nodelay = if overrides.contains(&Override::SetNodelay) {
        quote!(Self::set_nodelay(self, nodelay))
    } else {
        quote!(trillium_server_common::Transport::set_nodelay(&mut #transport, nodelay))
    };

    let set_ip_ttl = if overrides.contains(&Override::SetIpTtl) {
        quote!(Self::set_ip_ttl(self, ttl))
    } else {
        quote!(trillium_server_common::Transport::set_ip_ttl(&mut #transport, ttl))
    };

    let peer_addr = if overrides.contains(&Override::PeerAddr) {
        quote!(Self::peer_addr(self))
    } else {
        quote!(trillium_server_common::Transport::peer_addr(&#transport))
    };

    quote! {
        impl #impl_generics trillium_server_common::Transport for #struct_name #ty_generics #where_clause {
            fn set_linger(&mut self, linger: Option<core::time::Duration>) -> std::io::Result<()> { #set_linger }
            fn set_nodelay(&mut self, nodelay: bool) -> std::io::Result<()> { #set_nodelay }
            fn set_ip_ttl(&mut self, ttl: u32) -> std::io::Result<()> { #set_ip_ttl }
            fn peer_addr(&self) -> std::io::Result<Option<std::net::SocketAddr>> { #peer_addr }
        }
    }
    .into()
}
