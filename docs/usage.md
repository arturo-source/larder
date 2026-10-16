# Using Larder

## Connecting

Fill in the endpoint, region and keys, then **Connect**, or **Save Profile** to keep it for later. Saved
profiles connect with a single click. Leave the endpoint empty for Amazon S3; enable **Path-Style Addressing**
for most self-hosted servers (RustFS, MinIO, …).

Profiles are stored in `~/.config/larder/config.json`. **Secret keys are stored there in plain text.**
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
