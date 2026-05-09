//! Internal-use fork of
//! [`include_dir_macros`](https://docs.rs/include_dir_macros/) for
//! [`trillium.rs`](https://trillium.rs). It is not intended for general
//! use. Credit for the bulk of the code goes to the authors of the
//! upstream crate.
//!
//! Differences from upstream:
//!
//! include_entry was added, which returns a DirEntry instead of a Dir,
//! making direct inclusion of files possible
//! Metadata is always enabled
//! relative paths are resolved from a root of CARGO_MANIFEST_DIR
//! hygiene is maintained by using a macro_rules macro to import
//! relevant structs
//! Optional compile-time precompression of file contents into Brotli /
//! Zstd / Gzip variants, gated behind cargo features.

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

use proc_macro::TokenStream;
use proc_macro2::{Literal, Span, TokenStream as TokenStream2};
use quote::quote;
use std::{
    collections::HashMap,
    error::Error,
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
    time::SystemTime,
};
use syn::{
    Ident, LitBool, LitStr, Token, bracketed,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
};

/// Embed the contents of a directory. "Returns" a Dir
#[proc_macro]
pub fn include_dir(input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(input as Args);
    expand(args, true).into()
}

/// Embed a directory or file. "Returns" a DirEntry
#[proc_macro]
pub fn include_entry(input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(input as Args);
    expand(args, false).into()
}

struct Args {
    path: LitStr,
    compress: Option<(Span, CompressSpec)>,
    etag: bool,
}

enum CompressSpec {
    Default,
    Specified(Vec<Encoding>),
}

impl Parse for Args {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let path: LitStr = input.parse()?;
        let mut compress = None;
        let mut etag = true;
        let mut etag_seen = false;

        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let kw: Ident = input.parse()?;
            let kw_str = kw.to_string();
            match kw_str.as_str() {
                "compress" => {
                    if compress.is_some() {
                        return Err(syn::Error::new(
                            kw.span(),
                            "`compress` specified more than once",
                        ));
                    }
                    let span = kw.span();
                    let spec = if input.peek(Token![=]) {
                        input.parse::<Token![=]>()?;
                        let content;
                        bracketed!(content in input);
                        let list: Punctuated<Ident, Token![,]> =
                            Punctuated::parse_terminated(&content)?;
                        let mut encodings = Vec::new();
                        for id in &list {
                            encodings.push(Encoding::from_ident(id)?);
                        }
                        CompressSpec::Specified(encodings)
                    } else {
                        CompressSpec::Default
                    };
                    compress = Some((span, spec));
                }
                "etag" => {
                    if etag_seen {
                        return Err(syn::Error::new(
                            kw.span(),
                            "`etag` specified more than once",
                        ));
                    }
                    etag_seen = true;
                    input.parse::<Token![=]>()?;
                    let val: LitBool = input.parse()?;
                    etag = val.value;
                }
                _ => {
                    return Err(syn::Error::new(
                        kw.span(),
                        format!("unknown argument `{kw_str}`; expected `compress` or `etag`"),
                    ));
                }
            }
        }

        Ok(Args {
            path,
            compress,
            etag,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Encoding {
    Brotli,
    Zstd,
    Gzip,
}

impl Encoding {
    fn from_ident(ident: &Ident) -> syn::Result<Self> {
        let s = ident.to_string();
        let enc = match s.as_str() {
            "Brotli" => Self::Brotli,
            "Zstd" => Self::Zstd,
            "Gzip" => Self::Gzip,
            _ => {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("unknown encoding `{s}`; expected `Brotli`, `Zstd`, or `Gzip`"),
                ));
            }
        };
        if let Some(missing_feature) = enc.required_feature_if_disabled() {
            return Err(syn::Error::new(
                ident.span(),
                format!(
                    "encoding `{s}` requires the `{missing_feature}` feature on \
                     `trillium-static-compiled`"
                ),
            ));
        }
        Ok(enc)
    }

    fn required_feature_if_disabled(self) -> Option<&'static str> {
        match self {
            Self::Brotli => {
                #[cfg(feature = "brotli")]
                {
                    None
                }
                #[cfg(not(feature = "brotli"))]
                {
                    Some("brotli")
                }
            }
            Self::Zstd => {
                #[cfg(feature = "zstd")]
                {
                    None
                }
                #[cfg(not(feature = "zstd"))]
                {
                    Some("zstd")
                }
            }
            Self::Gzip => {
                #[cfg(feature = "gzip")]
                {
                    None
                }
                #[cfg(not(feature = "gzip"))]
                {
                    Some("gzip")
                }
            }
        }
    }

    fn variant_path(self) -> TokenStream2 {
        match self {
            Self::Brotli => quote!(Encoding::Brotli),
            Self::Zstd => quote!(Encoding::Zstd),
            Self::Gzip => quote!(Encoding::Gzip),
        }
    }
}

// cfg-gated pushes; `vec![]` doesn't compose with #[cfg].
#[allow(unused_mut, clippy::vec_init_then_push)]
fn default_encodings() -> Vec<Encoding> {
    let mut v = Vec::new();
    #[cfg(feature = "brotli")]
    v.push(Encoding::Brotli);
    #[cfg(feature = "zstd")]
    v.push(Encoding::Zstd);
    #[cfg(feature = "gzip")]
    v.push(Encoding::Gzip);
    v
}

fn expand(args: Args, dir_only: bool) -> TokenStream2 {
    let raw = args.path.value();
    let path = match resolve_path(&raw, get_env).and_then(|p| Ok(p.canonicalize()?)) {
        Ok(p) => p,
        Err(e) => {
            return syn::Error::new(args.path.span(), format!("{e}")).to_compile_error();
        }
    };

    if dir_only && !path.is_dir() {
        return syn::Error::new(
            args.path.span(),
            format!("\"{}\" is not a directory", path.display()),
        )
        .to_compile_error();
    }

    let encodings = match args.compress {
        None => Vec::new(),
        Some((span, CompressSpec::Default)) => {
            let v = default_encodings();
            if v.is_empty() {
                return syn::Error::new(
                    span,
                    "no compression features are enabled on `trillium-static-compiled`; enable \
                     one or more of `brotli`, `zstd`, `gzip`, or the `compression` meta-feature",
                )
                .to_compile_error();
            }
            v
        }
        Some((_, CompressSpec::Specified(list))) => list,
    };

    let mut file_paths = Vec::new();
    if path.is_file() {
        file_paths.push(path.clone());
    } else {
        collect_files(&path, &mut file_paths);
    }

    let processed = process_files(file_paths, &encodings, args.etag);

    if dir_only {
        expand_dir(&path, &path, &processed)
    } else {
        expand_entry(&path, &path, &processed)
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if !dir.is_dir() {
        return;
    }
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(it) => it.filter_map(|e| e.ok().map(|e| e.path())).collect(),
        Err(e) => panic!("Unable to read \"{}\": {}", dir.display(), e),
    };
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect_files(&p, out);
        } else if p.is_file() {
            out.push(p);
        }
    }
}

struct FileBytes {
    source: Vec<u8>,
    variants: Vec<(Encoding, Vec<u8>)>,
    etag: Option<String>,
}

fn compute_etag(source: &[u8]) -> String {
    etag::EntityTag::from_data(source).to_string()
}

#[cfg(any(feature = "brotli", feature = "zstd", feature = "gzip"))]
fn process_files(
    paths: Vec<PathBuf>,
    encodings: &[Encoding],
    with_etag: bool,
) -> HashMap<PathBuf, FileBytes> {
    use rayon::prelude::*;

    let pairs: Vec<(PathBuf, Vec<u8>)> = paths
        .into_par_iter()
        .map(|p| {
            let bytes = std::fs::read(&p)
                .unwrap_or_else(|e| panic!("Unable to read \"{}\": {}", p.display(), e));
            (p, bytes)
        })
        .collect();

    pairs
        .into_par_iter()
        .map(|(p, source)| {
            let mut variants: Vec<(Encoding, Vec<u8>)> = encodings
                .iter()
                .filter_map(|&enc| compress(enc, &source).map(|c| (enc, c)))
                .collect();
            variants.sort_by_key(|(_, bytes)| bytes.len());
            let etag = with_etag.then(|| compute_etag(&source));
            (
                p,
                FileBytes {
                    source,
                    variants,
                    etag,
                },
            )
        })
        .collect()
}

#[cfg(not(any(feature = "brotli", feature = "zstd", feature = "gzip")))]
fn process_files(
    paths: Vec<PathBuf>,
    _encodings: &[Encoding],
    with_etag: bool,
) -> HashMap<PathBuf, FileBytes> {
    paths
        .into_iter()
        .map(|p| {
            let bytes = std::fs::read(&p)
                .unwrap_or_else(|e| panic!("Unable to read \"{}\": {}", p.display(), e));
            let etag = with_etag.then(|| compute_etag(&bytes));
            (
                p,
                FileBytes {
                    source: bytes,
                    variants: Vec::new(),
                    etag,
                },
            )
        })
        .collect()
}

#[cfg(any(feature = "brotli", feature = "zstd", feature = "gzip"))]
fn compress(encoding: Encoding, source: &[u8]) -> Option<Vec<u8>> {
    /// Skip compression entirely for files smaller than this.
    const MIN_SIZE: usize = 256;

    if source.len() < MIN_SIZE {
        return None;
    }

    let encoded = match encoding {
        #[cfg(feature = "brotli")]
        Encoding::Brotli => brotli_encode(source),
        #[cfg(feature = "zstd")]
        Encoding::Zstd => zstd_encode(source),
        #[cfg(feature = "gzip")]
        Encoding::Gzip => gzip_encode(source),
        #[allow(unreachable_patterns)]
        _ => return None,
    };

    // Require a 5% size reduction over the source to bother baking the variant.
    if encoded.len() * 20 <= source.len() * 19 {
        Some(encoded)
    } else {
        None
    }
}

#[cfg(feature = "brotli")]
fn brotli_encode(source: &[u8]) -> Vec<u8> {
    let params = brotli::enc::BrotliEncoderParams {
        quality: 11,
        ..Default::default()
    };
    let mut input = source;
    let mut output = Vec::new();
    brotli::BrotliCompress(&mut input, &mut output, &params).expect("brotli compression failed");
    output
}

#[cfg(feature = "zstd")]
fn zstd_encode(source: &[u8]) -> Vec<u8> {
    zstd::encode_all(source, 22).expect("zstd compression failed")
}

#[cfg(feature = "gzip")]
fn gzip_encode(source: &[u8]) -> Vec<u8> {
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(source).expect("gzip compression failed");
    encoder.finish().expect("gzip compression failed")
}

fn expand_entry(root: &Path, child: &Path, files: &HashMap<PathBuf, FileBytes>) -> TokenStream2 {
    if child.is_dir() {
        let tokens = expand_dir(root, child, files);
        quote!(DirEntry::Dir(#tokens))
    } else if child.is_file() {
        let tokens = expand_file(root, child, files);
        quote!(DirEntry::File(#tokens))
    } else {
        panic!("\"{}\" is neither a file nor a directory", child.display());
    }
}

fn expand_dir(root: &Path, path: &Path, files: &HashMap<PathBuf, FileBytes>) -> TokenStream2 {
    let mut children: Vec<PathBuf> = std::fs::read_dir(path)
        .unwrap_or_else(|e| panic!("Unable to read \"{}\": {}", path.display(), e))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    children.sort();

    let child_tokens: Vec<TokenStream2> = children
        .iter()
        .map(|c| expand_entry(root, c, files))
        .collect();

    let path_str = normalize_path(root, path);

    quote!(Dir::new(#path_str, &[ #(#child_tokens),* ]))
}

fn expand_file(root: &Path, path: &Path, files: &HashMap<PathBuf, FileBytes>) -> TokenStream2 {
    let fb = files
        .get(path)
        .expect("file should be present in compression results");
    let source_lit = Literal::byte_string(&fb.source);

    // When the file IS the root (include_entry! called directly on a file),
    // normalize_path would strip the entire path leaving "". Use the filename
    // directly so that mime_guess can detect the correct content type.
    let normalized_path = if root == path {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        normalize_path(root, path)
    };

    let base = quote!(File::new(#normalized_path, #source_lit));

    let with_meta = match metadata(path) {
        Some(m) => quote!(#base.with_metadata(#m)),
        None => base,
    };

    let with_etag = match &fb.etag {
        Some(etag) => quote!(#with_meta.with_etag(#etag)),
        None => with_meta,
    };

    if fb.variants.is_empty() {
        with_etag
    } else {
        let variant_tokens = fb.variants.iter().map(|(enc, bytes)| {
            let enc_path = enc.variant_path();
            let bytes_lit = Literal::byte_string(bytes);
            quote!((#enc_path, #bytes_lit))
        });
        quote!(#with_etag.with_encodings(&[#(#variant_tokens),*]))
    }
}

fn metadata(path: &Path) -> Option<TokenStream2> {
    fn to_unix(t: SystemTime) -> u64 {
        t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
    }

    let meta = path.metadata().ok()?;
    let accessed = meta.accessed().map(to_unix).ok()?;
    let created = meta.created().map(to_unix).ok()?;
    let modified = meta.modified().map(to_unix).ok()?;

    Some(quote!(Metadata::from_secs(#accessed, #created, #modified)))
}

/// Make sure that paths use the same separator regardless of whether the host
/// machine is Windows or Linux.
fn normalize_path(root: &Path, path: &Path) -> String {
    let stripped = path
        .strip_prefix(root)
        .expect("Should only ever be called using paths inside the root path");
    let as_string = stripped.to_string_lossy();

    as_string.replace('\\', "/")
}

fn resolve_path(
    raw: &str,
    get_env: impl Fn(&str) -> Option<String>,
) -> Result<PathBuf, Box<dyn Error>> {
    let mut unprocessed = raw;
    let mut resolved = String::new();

    while let Some(dollar_sign) = unprocessed.find('$') {
        let (head, tail) = unprocessed.split_at(dollar_sign);
        resolved.push_str(head);

        match parse_identifier(&tail[1..]) {
            Some((variable, rest)) => {
                let value = get_env(variable).ok_or_else(|| MissingVariable {
                    variable: variable.to_string(),
                })?;
                resolved.push_str(&value);
                unprocessed = rest;
            }
            None => {
                return Err(UnableToParseVariable { rest: tail.into() }.into());
            }
        }
    }
    resolved.push_str(unprocessed);

    let path = PathBuf::from(resolved);
    if path.is_relative() {
        Ok(PathBuf::from(
            get_env("CARGO_MANIFEST_DIR").ok_or_else(|| MissingVariable {
                variable: "CARGO_MANIFEST_DIR".to_string(),
            })?,
        )
        .join(path))
    } else {
        Ok(path)
    }
}

#[derive(Debug, PartialEq)]
struct MissingVariable {
    variable: String,
}

impl Error for MissingVariable {}

impl Display for MissingVariable {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Unable to resolve ${}", self.variable)
    }
}

#[derive(Debug, PartialEq)]
struct UnableToParseVariable {
    rest: String,
}

impl Error for UnableToParseVariable {}

impl Display for UnableToParseVariable {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Unable to parse a variable from \"{}\"", self.rest)
    }
}

fn parse_identifier(text: &str) -> Option<(&str, &str)> {
    let mut calls = 0;

    let (head, tail) = take_while(text, |c| {
        calls += 1;

        match c {
            '_' => true,
            letter if letter.is_ascii_alphabetic() => true,
            digit if digit.is_ascii_digit() && calls > 1 => true,
            _ => false,
        }
    });

    if head.is_empty() {
        None
    } else {
        Some((head, tail))
    }
}

fn take_while(s: &str, mut predicate: impl FnMut(char) -> bool) -> (&str, &str) {
    let mut index = 0;

    for c in s.chars() {
        if predicate(c) {
            index += c.len_utf8();
        } else {
            break;
        }
    }

    s.split_at(index)
}

fn get_env(variable: &str) -> Option<String> {
    std::env::var(variable).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_path_with_no_environment_variables() {
        let path = "./file.txt";

        let resolved = resolve_path(path, |name| {
            assert_eq!(name, "CARGO_MANIFEST_DIR");
            Some("/files/cargo_manifest_dir".to_string())
        })
        .unwrap();

        assert_eq!(
            resolved.to_str().unwrap(),
            PathBuf::from("/files/cargo_manifest_dir")
                .join("./file.txt")
                .to_str()
                .unwrap()
        );
    }

    #[test]
    fn simple_environment_variable() {
        let path = "../$VAR";

        let resolved = resolve_path(path, |name| match name {
            "VAR" => Some("file.txt".to_string()),
            "CARGO_MANIFEST_DIR" => Some("/files/cargo_manifest_dir".to_string()),
            _ => unreachable!(),
        })
        .unwrap();

        assert_eq!(
            resolved.to_str().unwrap(),
            PathBuf::from("/files/cargo_manifest_dir")
                .join("../file.txt")
                .to_str()
                .unwrap()
        );
    }

    #[test]
    fn dont_resolve_recursively() {
        let path = "./$TOP_LEVEL.txt";

        let resolved = resolve_path(path, |name| match name {
            "TOP_LEVEL" => Some("$NESTED".to_string()),
            "CARGO_MANIFEST_DIR" => Some("/files/cargo_manifest_dir".to_string()),
            "$NESTED" => unreachable!("Shouln't resolve recursively"),
            _ => unreachable!(),
        })
        .unwrap();

        assert_eq!(
            resolved.to_str().unwrap(),
            PathBuf::from("/files/cargo_manifest_dir")
                .join("./$NESTED.txt")
                .to_str()
                .unwrap()
        );
    }

    #[test]
    fn parse_valid_identifiers() {
        let inputs = vec![
            ("a", "a"),
            ("a_", "a_"),
            ("_asf", "_asf"),
            ("a1", "a1"),
            ("a1_#sd", "a1_"),
        ];

        for (src, expected) in inputs {
            let (got, rest) = parse_identifier(src).unwrap();
            assert_eq!(got.len() + rest.len(), src.len());
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn unknown_environment_variable() {
        let path = "$UNKNOWN";

        let err = resolve_path(path, |_| None).unwrap_err();

        let missing_variable = err.downcast::<MissingVariable>().unwrap();
        assert_eq!(
            *missing_variable,
            MissingVariable {
                variable: String::from("UNKNOWN"),
            }
        );
    }

    #[test]
    fn invalid_variables() {
        let inputs = &["$1", "$"];

        for input in inputs {
            let err = resolve_path(input, |_| unreachable!()).unwrap_err();

            let err = err.downcast::<UnableToParseVariable>().unwrap();
            assert_eq!(
                *err,
                UnableToParseVariable {
                    rest: input.to_string(),
                }
            );
        }
    }
}
