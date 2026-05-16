# AGENTS.md

- This repository provides a wrapper for `sandbox-exec`.
- Run `cargo fmt`, `cargo test`, `cargo build --release`, and `cargo lint` to
  verify changes.
- In tests, prefer direct `assert!`/`assert_eq!` expectations over manual
  `panic!`, and avoid returning `Result` unless `?` materially improves setup
  clarity.
- Use `thiserror` for typed library/domain errors, `eyre` with `color-eyre` at
  application boundaries, and add context when propagating errors.
- Verify changes to shell scripts by running `shellcheck`.
- Verify changes to the SBPL profile by running the syntax smoke test:
  `./test-syntax.sh <profile>`.
