# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-08-24

### Added
- `BasicAuth::validate_fn` and `BasicAuth::validate_async_fn` accept credentials by predicate, so
  authentication can consult a user table or an api key store instead of a single configured pair.
- `Credentials::from_conn` exposes the header-parsing half of this crate for applications that need
  the password itself, such as the convention of sending an api key as the password.
- `BasicAuthConnExt::basic_auth_username` reads the authenticated username downstream.
- `BasicAuth::realm`.

### Fixed
- Credentials are compared in constant time. The previous string comparison of the whole
  `Authorization` header short-circuited on the first differing byte, a timing oracle for recovering
  the credential a byte at a time.
- The `Basic` auth-scheme is matched case-insensitively, per RFC 9110 §11.
- The `Debug` implementations of `Credentials` and `BasicAuth` no longer print the password.

### Changed
- **Breaking**: the conn state set by this handler is the authenticated username rather than a
  `Credentials`, read through `BasicAuthConnExt::basic_auth_username`. The password is no longer
  retained anywhere after validation.
- **Breaking**: `BasicAuth::new` takes `impl Into<String>` arguments.

## [0.2.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- *(deps)* update base64 requirement from 0.21.5 to 0.22.0
