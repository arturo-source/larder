# Development

## Dependencies

Rust (stable) plus GTK ≥ 4.14 and libadwaita ≥ 1.5 with their development files:

```sh
sudo pacman -S gtk4 libadwaita                     # Arch
sudo dnf install gtk4-devel libadwaita-devel       # Fedora
sudo apt install libgtk-4-dev libadwaita-1-dev     # Debian 13+ / Ubuntu 24.04+
```

GTK also builds on Windows (MSYS2 or gvsbuild) and macOS (Homebrew), though Larder is mainly developed for Linux.

## Local S3 server

[RustFS](https://github.com/rustfs/rustfs) in Docker works well for development:

```sh
docker run -d --name rustfs -p 9000:9000 -p 9001:9001 \
  -e RUSTFS_ACCESS_KEY=rustfsadmin -e RUSTFS_SECRET_KEY=rustfsadmin rustfs/rustfs:latest
```

Connect to `http://localhost:9000` with `rustfsadmin` / `rustfsadmin` and path-style addressing.

## Tests

```sh
cargo test
```

The end-to-end test in `src/s3.rs` (buckets, multipart uploads, rename, recursive download, presigning,
error messages) only runs when a server is configured:

```sh
LARDER_TEST_ENDPOINT=http://localhost:9000 \
AWS_ACCESS_KEY_ID=rustfsadmin AWS_SECRET_ACCESS_KEY=rustfsadmin \
cargo test
```

## Continuous integration

- `.github/workflows/ci.yml` runs on pull requests and pushes to `main`, on Ubuntu 24.04 (the oldest
  supported GTK and libadwaita): `cargo fmt --check`, `cargo clippy -D warnings`, validation of the desktop
  file and AppStream metadata, the tests (including the end-to-end test against a RustFS service container)
  and `cargo doc` with warnings denied. `main` is protected: these checks must pass before merging.
- `.github/workflows/build.yml` builds the packages: Linux x86_64 and aarch64 tarballs and `.deb`
  (built on Ubuntu 24.04), a Windows zip (MSYS2, with GTK bundled) and a macOS `.app` (Homebrew, with GTK
  bundled), plus `larder-<version>-vendored.tar.xz`: the source with every Linux dependency vendored, which
  builds offline (used by the Flathub build). Every package is smoke-tested and screenshotted on its platform. Pushing a `v*` tag publishes
  them with a `SHA256SUMS` file and build attestations as a GitHub release; manual runs and pull requests
  touching packaging keep them as workflow artifacts.

## Releasing

1. Bump `version` in `Cargo.toml` and add a `<release>` entry to `data/io.github.arturo_source.Larder.metainfo.xml`.
2. Merge, then tag: `git tag v0.2.0 && git push origin v0.2.0`. The tag must match the `Cargo.toml` version.

## Code layout

| Path | What it does |
| --- | --- |
| `src/s3.rs` | All S3 operations: listing, multipart uploads with progress, downloads, rename, delete, presigning, readable errors. UI-independent and covered by the tests |
| `src/ui/mod.rs` | Main window, actions, shortcuts and wiring of operations |
| `src/ui/objects.rs` | Object list (column view, context menu, drop target) |
| `src/ui/details.rs` | Details and preview pane |
| `src/ui/transfers.rs` | Upload progress bar |
| `src/ui/dialogs.rs` | Confirmation, input and error dialogs |
| `src/ui/connect.rs` | Connection page and saved profiles |
| `src/runtime.rs` | The tokio runtime the S3 SDK runs on, awaited from the GTK main loop |
| `src/config.rs` | Saved connection profiles |
