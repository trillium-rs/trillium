//! Integration tests for `StaticFileHandler`.
//!
//! These tests require a runtime feature (smol, tokio, or async-std) and are
//! skipped otherwise.  In CI the workspace test command enables
//! `trillium-static/tokio` so they always run there.

#![cfg(any(feature = "smol", feature = "tokio", feature = "async-std"))]

use std::{fs, path::PathBuf};
use tempfile::TempDir;
use trillium::Status;
use trillium_static::{StaticConnExt, StaticFileHandler};
use trillium_testing::{TestServer, block_on};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Creates a two-level layout:
///
/// ```text
/// outer/
///   secret.txt          ← outside the web root
///   www/                ← web root handed to StaticFileHandler
///     public.txt
///     subdir/
///       nested.txt
/// ```
///
/// Returns `(outer_tmpdir, www_path)`.  Keep `outer_tmpdir` alive for the
/// duration of the test.
fn setup() -> (TempDir, PathBuf) {
    let outer = TempDir::new().unwrap();
    fs::write(outer.path().join("secret.txt"), "secret content").unwrap();
    let www = outer.path().join("www");
    fs::create_dir(&www).unwrap();
    fs::write(www.join("public.txt"), "public content").unwrap();
    fs::create_dir(www.join("subdir")).unwrap();
    fs::write(www.join("subdir/nested.txt"), "nested content").unwrap();
    (outer, www)
}

// ---------------------------------------------------------------------------
// Basic serving
// ---------------------------------------------------------------------------

#[test]
fn serves_existing_file() {
    let (_outer, www) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/public.txt")
            .await
            .assert_ok()
            .assert_body("public content");
    });
}

#[test]
fn returns_404_for_missing_file() {
    let (_outer, www) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/nonexistent.txt")
            .await
            .assert_status(Status::NotFound);
    });
}

#[test]
fn serves_file_in_subdir() {
    let (_outer, www) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/subdir/nested.txt")
            .await
            .assert_ok()
            .assert_body("nested content");
    });
}

#[test]
fn dir_without_index_file() {
    let (_outer, www) = setup();
    block_on(async {
        let app = TestServer::new((
            StaticFileHandler::new(&www),
            |conn: trillium::Conn| async move {
                if let Some(dir) = conn.resolved_directory() {
                    let body = format!("resolved directory: {}", dir.path().display());
                    conn.ok(body)
                } else {
                    conn
                }
            },
        ))
        .await;

        app.get("/subdir").await.assert_ok().assert_body(&format!(
            "resolved directory: {}",
            www.canonicalize().unwrap().join("subdir").display()
        ));
    });
}

#[test]
fn dir_with_index_file() {
    let (_outer, www) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www).with_index_file("nested.txt")).await;

        app.get("/subdir/")
            .await
            .assert_ok()
            .assert_body("nested content");
    });
}

// ---------------------------------------------------------------------------
// Path normalisation — legitimate uses of `.` and `..`
// ---------------------------------------------------------------------------

#[test]
fn dot_segment_is_resolved() {
    let (_outer, www) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/./public.txt").await.assert_ok();
        app.get("/subdir/./nested.txt").await.assert_ok();
    });
}

#[test]
fn dotdot_within_root_is_resolved() {
    let (_outer, www) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        // /subdir/../public.txt resolves to /public.txt — still inside root
        app.get("/subdir/../public.txt")
            .await
            .assert_ok()
            .assert_body("public content");
    });
}

// ---------------------------------------------------------------------------
// Path traversal security — `..` must not escape the root
// ---------------------------------------------------------------------------

#[test]
fn dotdot_from_root_is_blocked() {
    let (_outer, www) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/../secret.txt")
            .await
            .assert_status(Status::NotFound);
    });
}

#[test]
fn multiple_dotdots_are_blocked() {
    let (_outer, www) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/../../secret.txt")
            .await
            .assert_status(Status::NotFound);
    });
}

#[test]
fn dotdot_after_subdir_that_escapes_root_is_blocked() {
    let (_outer, www) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        // /subdir/../../ pops back to outer/, then tries secret.txt
        app.get("/subdir/../../secret.txt")
            .await
            .assert_status(Status::NotFound);
    });
}

// `starts_with` on `Path` is component-based, so a root of `/var/www` does
// not accidentally permit `/var/wwwother`.  This test guards that assumption.
#[test]
fn path_prefix_not_confused_with_path_starts_with() {
    let outer = TempDir::new().unwrap();
    // Two sibling dirs with a common prefix in their names.
    let www = outer.path().join("www");
    let wwwother = outer.path().join("wwwother");
    fs::create_dir(&www).unwrap();
    fs::create_dir(&wwwother).unwrap();
    fs::write(www.join("public.txt"), "public").unwrap();
    fs::write(wwwother.join("secret.txt"), "sibling secret").unwrap();

    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        // No traversal possible into the sibling directory via URL alone.
        app.get("/../wwwother/secret.txt")
            .await
            .assert_status(Status::NotFound);
    });
}

// ---------------------------------------------------------------------------
// Symlinks — followed by design (same as nginx default), operator's
// responsibility.  A future `.without_follow_symlinks()` option could
// restrict this for shared-hosting scenarios.
// ---------------------------------------------------------------------------

#[test]
#[cfg(unix)]
fn symlink_within_root_is_followed() {
    use std::os::unix::fs::symlink;

    let (_outer, www) = setup();
    // A symlink that stays inside the root — common deployment pattern.
    symlink(www.join("public.txt"), www.join("link.txt")).unwrap();

    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/link.txt")
            .await
            .assert_ok()
            .assert_body("public content");
    });
}

#[test]
#[cfg(unix)]
fn symlink_escaping_root_is_followed() {
    // This documents the current intended behaviour: symlinks pointing outside
    // the root are followed.  The threat model is that only the server admin
    // can create symlinks on the filesystem, so this is their responsibility —
    // analogous to nginx's default `disable_symlinks off`.
    use std::os::unix::fs::symlink;

    let (_outer, www) = setup();
    symlink(
        _outer.path().join("secret.txt"),
        www.join("via_symlink.txt"),
    )
    .unwrap();

    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/via_symlink.txt")
            .await
            .assert_ok()
            .assert_body("secret content");
    });
}

// ---------------------------------------------------------------------------
// Precompressed sidecars
// ---------------------------------------------------------------------------

mod precompressed {
    use super::*;
    use trillium::{Conn, KnownHeaderName::Vary};

    /// Layout produced by [`setup_with_sidecars`]:
    ///
    /// ```text
    /// www/
    ///   page.html              ← all three sidecars present
    ///   page.html.br
    ///   page.html.zst
    ///   page.html.gz
    ///   only-gz.html           ← only one sidecar present
    ///   only-gz.html.gz
    ///   plain.html             ← no sidecars
    ///   home/
    ///     index.html           ← exercises the index/Dir code path
    ///     index.html.br
    /// ```
    fn setup_with_sidecars() -> (TempDir, PathBuf) {
        let outer = TempDir::new().unwrap();
        let www = outer.path().join("www");
        fs::create_dir(&www).unwrap();

        fs::write(www.join("page.html"), "original page content").unwrap();
        fs::write(www.join("page.html.br"), "brotli-encoded payload").unwrap();
        fs::write(www.join("page.html.zst"), "zstd-encoded payload").unwrap();
        fs::write(www.join("page.html.gz"), "gzip-encoded payload").unwrap();

        fs::write(www.join("only-gz.html"), "only gz original").unwrap();
        fs::write(www.join("only-gz.html.gz"), "only gz precompressed").unwrap();

        fs::write(www.join("plain.html"), "plain original").unwrap();

        fs::create_dir(www.join("home")).unwrap();
        fs::write(www.join("home/index.html"), "home index original").unwrap();
        fs::write(www.join("home/index.html.br"), "home index brotli").unwrap();

        (outer, www)
    }

    #[test]
    fn no_accept_encoding_serves_original_with_vary() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            let app = TestServer::new(StaticFileHandler::new(&www).with_precompressed()).await;
            app.get("/page.html")
                .await
                .assert_ok()
                .assert_body("original page content")
                .assert_no_header("content-encoding")
                .assert_header("vary", "Accept-Encoding")
                .assert_header("content-type", "text/html; charset=utf-8");
        });
    }

    #[test]
    fn feature_disabled_emits_no_vary() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            let app = TestServer::new(StaticFileHandler::new(&www)).await;
            // Even when the client offers compression, an unconfigured handler
            // must not advertise Vary — there is no encoding negotiation.
            app.get("/page.html")
                .with_request_header("accept-encoding", "br")
                .await
                .assert_ok()
                .assert_body("original page content")
                .assert_no_header("content-encoding")
                .assert_no_header("vary");
        });
    }

    #[test]
    fn gzip_accept_serves_gzip_sidecar() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            let app = TestServer::new(StaticFileHandler::new(&www).with_precompressed()).await;
            app.get("/page.html")
                .with_request_header("accept-encoding", "gzip")
                .await
                .assert_ok()
                .assert_body("gzip-encoded payload")
                .assert_header("content-encoding", "gzip")
                .assert_header("vary", "Accept-Encoding")
                // MIME comes from the original, not the .gz sidecar
                .assert_header("content-type", "text/html; charset=utf-8");
        });
    }

    #[test]
    fn brotli_preferred_over_gzip_with_default_priority() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            let app = TestServer::new(StaticFileHandler::new(&www).with_precompressed()).await;
            app.get("/page.html")
                .with_request_header("accept-encoding", "br, gzip")
                .await
                .assert_ok()
                .assert_body("brotli-encoded payload")
                .assert_header("content-encoding", "br");
        });
    }

    #[test]
    fn registration_order_decides_priority() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            // Register zstd first; it must beat brotli even when both are accepted.
            let app = TestServer::new(
                StaticFileHandler::new(&www)
                    .with_precompressed_variant("zstd", "zst")
                    .with_precompressed_variant("br", "br"),
            )
            .await;
            app.get("/page.html")
                .with_request_header("accept-encoding", "br, zstd")
                .await
                .assert_ok()
                .assert_body("zstd-encoded payload")
                .assert_header("content-encoding", "zstd");
        });
    }

    #[test]
    fn falls_back_to_original_when_no_sidecar_exists() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            let app = TestServer::new(StaticFileHandler::new(&www).with_precompressed()).await;
            app.get("/plain.html")
                .with_request_header("accept-encoding", "gzip, br, zstd")
                .await
                .assert_ok()
                .assert_body("plain original")
                .assert_no_header("content-encoding")
                .assert_header("vary", "Accept-Encoding");
        });
    }

    #[test]
    fn falls_back_when_only_unaccepted_sidecar_exists() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            // only-gz has only .gz on disk; client refuses gzip.
            let app = TestServer::new(StaticFileHandler::new(&www).with_precompressed()).await;
            app.get("/only-gz.html")
                .with_request_header("accept-encoding", "br")
                .await
                .assert_ok()
                .assert_body("only gz original")
                .assert_no_header("content-encoding");
        });
    }

    #[test]
    fn q0_disables_specific_encoding() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            let app = TestServer::new(StaticFileHandler::new(&www).with_precompressed()).await;
            app.get("/page.html")
                .with_request_header("accept-encoding", "gzip;q=0")
                .await
                .assert_ok()
                .assert_body("original page content")
                .assert_no_header("content-encoding")
                .assert_header("vary", "Accept-Encoding");
        });
    }

    #[test]
    fn wildcard_matches_first_registered_sidecar() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            // page.html has all three sidecars; default order is br first.
            let app = TestServer::new(StaticFileHandler::new(&www).with_precompressed()).await;
            app.get("/page.html")
                .with_request_header("accept-encoding", "*")
                .await
                .assert_ok()
                .assert_body("brotli-encoded payload")
                .assert_header("content-encoding", "br");
        });
    }

    #[test]
    fn index_file_serves_sidecar() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            let app = TestServer::new(
                StaticFileHandler::new(&www)
                    .with_index_file("index.html")
                    .with_precompressed(),
            )
            .await;
            app.get("/home")
                .with_request_header("accept-encoding", "br")
                .await
                .assert_ok()
                .assert_body("home index brotli")
                .assert_header("content-encoding", "br")
                .assert_header("vary", "Accept-Encoding")
                .assert_header("content-type", "text/html; charset=utf-8");
        });
    }

    #[test]
    fn index_file_falls_back_to_original_when_no_sidecar_accepted() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            let app = TestServer::new(
                StaticFileHandler::new(&www)
                    .with_index_file("index.html")
                    .with_precompressed(),
            )
            .await;
            app.get("/home")
                .with_request_header("accept-encoding", "gzip")
                .await
                .assert_ok()
                .assert_body("home index original")
                .assert_no_header("content-encoding")
                .assert_header("vary", "Accept-Encoding");
        });
    }

    #[test]
    fn vary_is_appended_to_upstream_value() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            // A handler ahead of the static handler stamps Vary; the static
            // handler must concatenate, not overwrite.
            let inject_vary = |conn: Conn| async { conn.with_response_header(Vary, "User-Agent") };
            let app = TestServer::new((
                inject_vary,
                StaticFileHandler::new(&www).with_precompressed(),
            ))
            .await;
            app.get("/page.html")
                .with_request_header("accept-encoding", "br")
                .await
                .assert_ok()
                .assert_body("brotli-encoded payload")
                .assert_header("vary", "User-Agent, Accept-Encoding");
        });
    }

    // Verifies the File-arm fix: `.without_etag_header()` previously only
    // applied to index-file resolution, not direct file requests.
    #[test]
    fn without_etag_header_applies_to_direct_file() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            let app = TestServer::new(StaticFileHandler::new(&www).without_etag_header()).await;
            app.get("/plain.html")
                .await
                .assert_ok()
                .assert_no_header("etag");
        });
    }

    #[test]
    fn without_modified_header_applies_to_direct_file() {
        let (_outer, www) = setup_with_sidecars();
        block_on(async {
            let app = TestServer::new(StaticFileHandler::new(&www).without_modified_header()).await;
            app.get("/plain.html")
                .await
                .assert_ok()
                .assert_no_header("last-modified");
        });
    }
}
