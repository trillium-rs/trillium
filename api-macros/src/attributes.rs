use proc_macro2::Span;
use syn::{
    Attribute, Error, Expr, ExprAssign, ExprLit, ExprPath, Lit, LitBool, Meta, Type, parse::Parser,
    punctuated::Punctuated, spanned::Spanned, token::Comma,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    State,
    Json,
    Body,
}

impl Source {
    fn from_ident(ident: &str) -> Option<Self> {
        match ident {
            "state" => Some(Self::State),
            "json" => Some(Self::Json),
            "body" => Some(Self::Body),
            _ => None,
        }
    }
}

pub struct ApiAttributes {
    pub(crate) source: Source,
    pub(crate) source_span: Span,
    pub(crate) clone: bool,
    pub(crate) err: Option<Type>,
}

impl ApiAttributes {
    pub(crate) fn from_attrs(attrs: &[Attribute]) -> syn::Result<Self> {
        let attr = attrs
            .iter()
            .find(|a| a.path().is_ident("api"))
            .ok_or_else(|| {
                Error::new(
                    Span::call_site(),
                    "missing required `#[api(...)]` attribute (state | json | body)",
                )
            })?;

        let Meta::List(list) = &attr.meta else {
            return Err(Error::new(attr.span(), "expected `#[api(...)]`"));
        };

        let exprs = Punctuated::<Expr, Comma>::parse_terminated.parse2(list.tokens.clone())?;

        let mut source: Option<(Source, Span)> = None;
        let mut clone = false;
        let mut err: Option<Type> = None;

        for expr in &exprs {
            match expr {
                Expr::Path(ExprPath { path, .. }) => {
                    let ident = path.require_ident()?.to_string();
                    if let Some(s) = Source::from_ident(&ident) {
                        if let Some((_, prev)) = source {
                            let mut e = Error::new(path.span(), "extraction source already set");
                            e.combine(Error::new(prev, "previously set here"));
                            return Err(e);
                        }
                        source = Some((s, path.span()));
                    } else if ident == "clone" {
                        clone = true;
                    } else {
                        return Err(Error::new(
                            path.span(),
                            format!(
                                "unrecognized `#[api]` key `{ident}` — expected one of: state, \
                                 json, body, clone, err"
                            ),
                        ));
                    }
                }

                Expr::Assign(ExprAssign { left, right, .. }) => {
                    let lhs = match &**left {
                        Expr::Path(ExprPath { path, .. }) => path.require_ident()?.to_string(),
                        _ => {
                            return Err(Error::new(left.span(), "expected identifier"));
                        }
                    };
                    match lhs.as_str() {
                        "err" => {
                            err = Some(expr_to_type(right)?);
                        }
                        "clone" => match &**right {
                            Expr::Lit(ExprLit {
                                lit: Lit::Bool(LitBool { value, .. }),
                                ..
                            }) => {
                                clone = *value;
                            }
                            _ => {
                                return Err(Error::new(
                                    right.span(),
                                    "`clone` expects a bool literal",
                                ));
                            }
                        },
                        other => {
                            return Err(Error::new(
                                left.span(),
                                format!("unrecognized `#[api]` key `{other}`"),
                            ));
                        }
                    }
                }

                _ => {
                    return Err(Error::new(expr.span(), "unrecognized `#[api]` entry"));
                }
            }
        }

        let (source, source_span) = source.ok_or_else(|| {
            Error::new(
                attr.span(),
                "`#[api(...)]` requires one of: `state`, `json`, `body`",
            )
        })?;

        Ok(Self {
            source,
            source_span,
            clone,
            err,
        })
    }
}

fn expr_to_type(expr: &Expr) -> syn::Result<Type> {
    match expr {
        Expr::Path(ExprPath { path, qself, .. }) => Ok(Type::Path(syn::TypePath {
            qself: qself.clone(),
            path: path.clone(),
        })),
        _ => Err(Error::new(
            expr.span(),
            "`err = ...` expects a type path (e.g. `err = my::ErrorHandler`)",
        )),
    }
}
