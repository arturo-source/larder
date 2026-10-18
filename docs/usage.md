# Using Larder

## Installing

Download the package for your system from the [latest release](https://github.com/arturo-source/larder/releases/latest):

| System | File | How |
| --- | --- | --- |
| Debian 13+, Ubuntu 24.04+ | `larder_*.deb` | `sudo apt install ./larder_*.deb` |
| Any Linux with GTK ≥ 4.14 and libadwaita ≥ 1.5 | `larder-*-linux.tar.gz` | `tar xzf larder-*-linux.tar.gz -C ~/.local --strip-components=1`, then run `larder` (it also appears in your app grid) |
| Windows 10/11 | `larder-*-windows.zip` | Unzip anywhere and run `bin\larder.exe` |
| macOS (Apple Silicon) | `larder-*-macos.zip` | Unzip and move `Larder.app` to Applications. It isn't notarized, so the first time right-click it and choose **Open** |

### Verifying downloads

Every release has a `SHA256SUMS` file. Put it next to your download and run:

```sh
sha256sum --check --ignore-missing SHA256SUMS
```

Each file also has a [build attestation](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations)
proving it was built by this repository's release workflow. With the GitHub CLI:

```sh
gh attestation verify larder_0.1.0-1_amd64.deb --repo arturo-source/larder
```

## Connecting

Fill in the endpoint, region and keys, then **Connect**, or **Save Profile** to keep it for later. Saved
profiles connect with a single click. Leave the endpoint empty for Amazon S3; enable **Path-Style Addressing**
for most self-hosted servers (RustFS, MinIO, …).

Profiles are stored in `~/.config/larder/config.json`. **Secret keys are stored there in plain text.**
Leave the access and secret keys empty to use the standard AWS credentials instead: environment variables,
`~/.aws/credentials` and `~/.aws/config` (choose a profile with `AWS_PROFILE`), SSO, credential processes or an
instance role. Nothing secret is then stored in the profile.
`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION` and `LARDER_ENDPOINT` prefill the form, and
`larder --connect <profile>` connects to a saved profile on startup.

## Features

- Buckets in the sidebar: create with **+**, right-click to delete (optionally emptying the bucket first)
- Folders and objects with sorting, search (just start typing) and multi-select (Ctrl/Shift-click, Ctrl+A)
- **Show Contents of Subfolders** in the main menu lists everything below the current folder
- Upload files or folders with **Upload**, or drag them in from your file manager. Files of 16 MiB or more
  are uploaded in parts, four at a time; the progress bar shows speed and time left, and cancelling aborts
  the upload on the server
- Download files or whole folders, create folders, rename or move objects and folders, delete recursively
- The details pane shows metadata and previews text and images
- **Share Link** creates a download link valid for 1 hour, 1 day or 7 days and copies it to the clipboard
- Errors are explained in plain language; when only some objects can be deleted, each failure is listed

## Keyboard shortcuts

| Keys | Action |
| --- | --- |
| `Alt+Up` | Parent folder |
| `F5` / `Ctrl+R` | Refresh |
| `Ctrl+F` (or just type) | Search the current folder |
| `Ctrl+U` | Upload files |
| `Ctrl+Shift+N` | New folder |
| `Ctrl+S` | Download |
| `Ctrl+L` | Share link |
| `F2` | Rename |
| `Delete` | Delete |
| `Ctrl+W` / `Ctrl+Q` | Close window / quit |
