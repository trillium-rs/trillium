//! Integration tests for `StaticFileHandler`.
//!
//! These tests require a runtime feature (smol, tokio, or async-std) and are
//! skipped otherwise.  In CI the workspace test command enables
//! `trillium-static/tokio` so they always run there.

#![cfg(any(feature = "smol", feature = "tokio", feature = "async-std"))]

use std::{fs, path::PathBuf};
use tempfile::TempDir;
use trillium::Status;
use trillium_static::StaticFileHandler;
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
