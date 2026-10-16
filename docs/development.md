# Development

## Dependencies

Rust (stable) plus GTK ≥ 4.14 and libadwaita ≥ 1.6 with their development files:

```sh
sudo pacman -S gtk4 libadwaita                     # Arch
sudo dnf install gtk4-devel libadwaita-devel       # Fedora
sudo apt install libgtk-4-dev libadwaita-1-dev     # Debian 13+ / Ubuntu 25.04+
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

- `.github/workflows/ci.yml` runs on pull requests and pushes to `main`: `cargo fmt --check`,
  `cargo clippy -D warnings`, the tests (including the end-to-end test against a RustFS service container)
  and `cargo doc` with warnings denied.
- `.github/workflows/build.yml` builds a release binary on pushes to `main` (downloadable from the run's
  artifacts) and attaches it to a GitHub release when a `v*` tag is pushed. The binary is built on Debian 13
  and needs a system with GTK 4 and libadwaita installed.

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
