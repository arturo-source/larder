# s3gui

A desktop browser for S3-compatible object storage (Amazon S3, RustFS, MinIO, Ceph, …), written in Rust
with GTK 4 and libadwaita.

## Features

- Connection profiles (endpoint, region, keys, path-style); click a saved profile to connect
- Bucket sidebar: create, delete (optionally emptying the bucket first)
- Object list with file-type icons, sorting, search (just start typing), multi-select and a right-click menu
- "Show contents of subfolders" for a recursive, flat view
- Upload files or folders with the Upload button or by dragging them in from your file manager
- Files of 16 MiB or more use multipart uploads (8 MiB+ parts, 4 in parallel), with progress, speed,
  time left and Cancel; cancelling aborts the upload on the server so no orphaned parts are left
- Download files or whole folders; create folders; rename or move objects and folders; delete recursively
- Details pane with metadata plus text and image previews
- Share links (presigned URLs valid for 1 hour, 1 day or 7 days), copied to the clipboard
- Clear errors: S3 error codes are explained in plain language, and a partially failed delete lists
  exactly which objects could not be deleted and why
- Follows the system light/dark style and adapts to narrow windows

### Keyboard shortcuts

| Keys | Action |
| --- | --- |
| `Alt+Up` | Parent folder |
| `F5` / `Ctrl+R` | Refresh |
| `Ctrl+F` (or just type) | Search the current folder |
| `Ctrl+U` / `Ctrl+Shift+N` | Upload files / new folder |
| `Ctrl+S` / `Ctrl+L` | Download / share link |
| `F2` / `Delete` | Rename / delete |

## Building

Needs GTK ≥ 4.14 and libadwaita ≥ 1.6 development files:

```sh
# Arch
sudo pacman -S gtk4 libadwaita
# Fedora
sudo dnf install gtk4-devel libadwaita-devel
# Debian / Ubuntu
sudo apt install libgtk-4-dev libadwaita-1-dev
```

```sh
cargo run --release
# or connect straight to a saved profile:
cargo run --release -- --connect local
```

`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION` and `S3GUI_ENDPOINT` prefill the connection form.
Profiles are stored in `~/.config/s3gui/config.json`; secret keys are stored there in plain text.

GTK also runs on Windows (via MSYS2 or gvsbuild) and macOS (via Homebrew), but it looks most at home on Linux.

## Local test server (RustFS)

```sh
docker run -d --name rustfs-s3gui -p 9000:9000 -p 9001:9001 \
  -e RUSTFS_ACCESS_KEY=rustfsadmin -e RUSTFS_SECRET_KEY=rustfsadmin rustfs/rustfs:latest
```

Connect to `http://localhost:9000` with `rustfsadmin` / `rustfsadmin` and path-style enabled.

## Tests

```sh
cargo test                                   # unit tests
S3GUI_TEST_ENDPOINT=http://localhost:9000 \
AWS_ACCESS_KEY_ID=rustfsadmin AWS_SECRET_ACCESS_KEY=rustfsadmin \
cargo test                                   # + end-to-end test against a live server
```

## Code layout

- `src/s3.rs`: all S3 operations (listing, multipart uploads with progress, downloads, rename, delete,
  presigning, readable errors). UI-independent, covered by the tests.
- `src/ui/`: the GTK interface (`mod.rs` window and actions, `objects.rs` list, `details.rs`,
  `transfers.rs`, `dialogs.rs`, `connect.rs`).
- `src/runtime.rs`: the tokio runtime the S3 SDK runs on, awaited from the GTK main loop.
