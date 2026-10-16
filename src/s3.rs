//! Thin async wrappers around the AWS S3 SDK, tailored to what the GUI needs.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::http::HttpResponse;
use aws_sdk_s3::config::{
    BehaviorVersion, Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::{ByteStream, DateTime, Length};
use aws_sdk_s3::types::{
    BucketLocationConstraint, CompletedMultipartUpload, CompletedPart, CreateBucketConfiguration,
    Delete, ObjectIdentifier,
};
use tokio::task::JoinSet;

use crate::config::Profile;

/// Largest object we download in full to render an image preview.
const MAX_IMAGE_PREVIEW: i64 = 20 * 1024 * 1024;
/// How many bytes of a text object we fetch for the preview.
const TEXT_PREVIEW_BYTES: i64 = 64 * 1024;
/// Files at least this large are uploaded in parts.
pub const MULTIPART_THRESHOLD: u64 = 16 * 1024 * 1024;
/// Smallest part size we use (S3's minimum is 5 MiB for all but the last part).
const MIN_PART_SIZE: u64 = 8 * 1024 * 1024;
/// S3 allows at most 10,000 parts per upload.
const MAX_PARTS: u64 = 10_000;
/// Parts of one file uploaded concurrently.
const PART_CONCURRENCY: usize = 4;

pub fn make_client(p: &Profile) -> Client {
    let creds = Credentials::new(&p.access_key, &p.secret_key, None, None, "larder");
    let mut builder = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(p.region.clone()))
        .credentials_provider(creds)
        .force_path_style(p.path_style)
        // Many S3-compatible servers don't support the newer default checksums.
        .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
        .response_checksum_validation(ResponseChecksumValidation::WhenRequired);
    if !p.endpoint.trim().is_empty() {
        builder = builder.endpoint_url(p.endpoint.trim());
    }
    Client::from_conf(builder.build())
}

/// Plain-language explanation for common S3 error codes.
pub fn explain_code(code: &str) -> Option<&'static str> {
    Some(match code {
        "AccessDenied" | "AllAccessDisabled" => {
            "Access denied: these credentials (or a bucket policy) don't allow this"
        }
        "InvalidAccessKeyId" => {
            "The server doesn't know this access key. Check the profile's access key"
        }
        "SignatureDoesNotMatch" => "Signature mismatch. The secret key is probably wrong",
        "RequestTimeTooSkewed" => "Your computer's clock is too far off from the server's",
        "AuthorizationHeaderMalformed" | "PermanentRedirect" => {
            "Wrong region: the bucket lives in a different region than the profile is set to"
        }
        "NoSuchBucket" => "The bucket doesn't exist (it may have been deleted)",
        "NoSuchKey" | "NotFound" => {
            "The object doesn't exist (it may have been deleted or renamed)"
        }
        "NoSuchUpload" => "The multipart upload no longer exists (it may have been aborted)",
        "BucketNotEmpty" => {
            "The bucket still contains objects. Tick \"Also delete all objects\" to empty it first"
        }
        "BucketAlreadyExists" => "That bucket name is already taken",
        "BucketAlreadyOwnedByYou" => "You already have a bucket with this name",
        "InvalidBucketName" => "The bucket name isn't valid",
        "EntityTooLarge" => "The object is larger than the server allows",
        "EntityTooSmall" => "An upload part was smaller than the server's minimum part size",
        "SlowDown" | "ServiceUnavailable" | "Throttling" => {
            "The server is overloaded. Try again in a moment"
        }
        "InternalError" => "The server ran into an internal error",
        "MethodNotAllowed" => "The server doesn't allow this operation here",
        "NotImplemented" => "This server doesn't support this operation",
        _ => return None,
    })
}

/// `"<explanation> (Code: server message)"`, or just `"Code: message"` for codes we don't know.
fn describe(code: &str, message: Option<&str>) -> String {
    let detail = match message.filter(|m| !m.is_empty()) {
        Some(m) => format!("{code}: {m}"),
        None => code.to_string(),
    };
    match explain_code(code) {
        Some(hint) => format!("{hint} ({detail})"),
        None => detail,
    }
}

/// The innermost error in a source chain, e.g. "Connection refused (os error 111)".
fn root_cause(e: &(dyn std::error::Error + 'static)) -> String {
    let mut cur = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

/// Turns SDK errors into readable messages instead of "service error".
fn sdk_err<E>(e: SdkError<E, HttpResponse>) -> anyhow::Error
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
{
    if let Some(se) = e.as_service_error()
        && let Some(code) = se.code()
    {
        // Explanation first, the server's own wording on the next line (shown as details in the UI).
        return match (explain_code(code), se.message().filter(|m| !m.is_empty())) {
            (Some(hint), Some(msg)) => anyhow!("{hint}\n{code}: {msg}"),
            (Some(hint), None) => anyhow!("{hint}\n{code}"),
            (None, _) => anyhow!(describe(code, se.message())),
        };
    }
    match &e {
        SdkError::DispatchFailure(_) | SdkError::TimeoutError(_) => {
            anyhow!("Could not reach the server: {}", root_cause(&e))
        }
        // HEAD requests have no error body, so only the status code tells us what happened.
        SdkError::ServiceError(ctx) => match ctx.raw().status().as_u16() {
            403 => anyhow!(describe("AccessDenied", None)),
            404 => anyhow!(describe("NotFound", None)),
            status => anyhow!("The server answered with HTTP {status}"),
        },
        _ => anyhow!("{}", DisplayErrorContext(&e)),
    }
}

pub fn fmt_time(t: &DateTime) -> String {
    chrono::DateTime::from_timestamp(t.secs(), 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
pub struct BucketInfo {
    pub name: String,
    pub created: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// Full key. Folders end with `/`.
    pub key: String,
    /// Display name relative to the listed prefix.
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    pub modified: Option<String>,
    pub modified_ts: i64,
}

#[derive(Clone, Debug)]
pub struct ObjectDetails {
    pub key: String,
    pub size: i64,
    pub content_type: Option<String>,
    pub etag: Option<String>,
    pub modified: Option<String>,
    pub storage_class: Option<String>,
    pub version_id: Option<String>,
    pub cache_control: Option<String>,
    pub content_encoding: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone)]
pub enum Preview {
    Text { text: String, truncated: bool },
    Image(Vec<u8>),
    None(String),
}

pub async fn list_buckets(c: &Client) -> Result<Vec<BucketInfo>> {
    let out = c.list_buckets().send().await.map_err(sdk_err)?;
    let mut buckets: Vec<_> = out
        .buckets()
        .iter()
        .map(|b| BucketInfo {
            name: b.name().unwrap_or_default().to_string(),
            created: b.creation_date().map(fmt_time),
        })
        .collect();
    buckets.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(buckets)
}

pub async fn create_bucket(c: &Client, name: &str, region: &str) -> Result<()> {
    let mut req = c.create_bucket().bucket(name);
    if !region.is_empty() && region != "us-east-1" {
        req = req.create_bucket_configuration(
            CreateBucketConfiguration::builder()
                .location_constraint(BucketLocationConstraint::from(region))
                .build(),
        );
    }
    req.send().await.map_err(sdk_err)?;
    Ok(())
}

/// Deletes a bucket; with `force`, empties it first.
pub async fn delete_bucket(c: &Client, name: &str, force: bool) -> Result<usize> {
    let removed = if force {
        delete_prefix(c, name, "").await?
    } else {
        0
    };
    c.delete_bucket()
        .bucket(name)
        .send()
        .await
        .map_err(sdk_err)?;
    Ok(removed)
}

/// Lists one "directory" level (or everything below `prefix` when `recursive`).
pub async fn list(c: &Client, bucket: &str, prefix: &str, recursive: bool) -> Result<Vec<Entry>> {
    list_inner(c, bucket, prefix, recursive, true).await
}

async fn list_inner(
    c: &Client,
    bucket: &str,
    prefix: &str,
    recursive: bool,
    hide_marker: bool,
) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = c
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .set_continuation_token(token.take());
        if !recursive {
            req = req.delimiter("/");
        }
        let out = req.send().await.map_err(sdk_err)?;
        for cp in out.common_prefixes() {
            let Some(p) = cp.prefix() else { continue };
            entries.push(Entry {
                key: p.to_string(),
                name: p[prefix.len()..].to_string(),
                is_dir: true,
                size: 0,
                modified: None,
                modified_ts: 0,
            });
        }
        for o in out.contents() {
            let Some(key) = o.key() else { continue };
            // Skip the zero-byte marker object representing the current folder.
            if hide_marker && key == prefix {
                continue;
            }
            entries.push(Entry {
                key: key.to_string(),
                name: key.strip_prefix(prefix).unwrap_or(key).to_string(),
                // Folder marker objects only show up here in recursive listings.
                is_dir: key.ends_with('/'),
                size: o.size().unwrap_or(0),
                modified: o.last_modified().map(fmt_time),
                modified_ts: o.last_modified().map(|t| t.secs()).unwrap_or(0),
            });
        }
        match out.next_continuation_token() {
            Some(t) if out.is_truncated() == Some(true) => token = Some(t.to_string()),
            _ => break,
        }
    }
    Ok(entries)
}

/// All object keys (with sizes) below a prefix, recursively.
pub async fn list_all_keys(c: &Client, bucket: &str, prefix: &str) -> Result<Vec<(String, i64)>> {
    // Unlike `list`, this includes the folder's own marker object (if any) so it gets deleted/copied too.
    Ok(list_inner(c, bucket, prefix, true, false)
        .await?
        .into_iter()
        .map(|e| (e.key, e.size))
        .collect())
}

/// Deletes keys in batches. Keeps going when some objects can't be deleted, then reports
/// exactly which ones failed and why.
pub async fn delete_keys(c: &Client, bucket: &str, keys: &[String]) -> Result<usize> {
    let mut deleted = 0;
    let mut failures = Vec::new();
    for chunk in keys.chunks(1000) {
        let objects = chunk
            .iter()
            .map(|k| ObjectIdentifier::builder().key(k).build())
            .collect::<Result<Vec<_>, _>>()?;
        let out = c
            .delete_objects()
            .bucket(bucket)
            .delete(
                Delete::builder()
                    .set_objects(Some(objects))
                    .quiet(true)
                    .build()?,
            )
            .send()
            .await
            .map_err(sdk_err);
        let out = match out {
            Ok(out) => out,
            // The whole request failed: nothing in this batch was deleted.
            Err(e) if deleted == 0 && failures.is_empty() => return Err(e),
            Err(e) => {
                failures.extend(chunk.iter().map(|k| format!("{k}: {e}")));
                continue;
            }
        };
        for err in out.errors() {
            failures.push(format!(
                "{}: {}",
                err.key().unwrap_or("?"),
                describe(err.code().unwrap_or("UnknownError"), err.message())
            ));
        }
        deleted += chunk.len() - out.errors().len();
    }
    if failures.is_empty() {
        return Ok(deleted);
    }
    const SHOWN: usize = 10;
    let mut msg = format!(
        "{} of {} objects could not be deleted ({deleted} deleted):",
        failures.len(),
        keys.len()
    );
    for f in failures.iter().take(SHOWN) {
        msg.push_str("\n  • ");
        msg.push_str(f);
    }
    if failures.len() > SHOWN {
        msg.push_str(&format!("\n  … and {} more", failures.len() - SHOWN));
    }
    bail!(msg)
}

/// Deletes files and whole folders (`(key, is_dir)` pairs) in one go, so failures are reported together.
pub async fn delete_targets(c: &Client, bucket: &str, targets: &[(String, bool)]) -> Result<usize> {
    let mut keys = Vec::new();
    for (key, is_dir) in targets {
        if *is_dir {
            keys.extend(
                list_all_keys(c, bucket, key)
                    .await?
                    .into_iter()
                    .map(|(k, _)| k),
            );
        } else {
            keys.push(key.clone());
        }
    }
    delete_keys(c, bucket, &keys).await
}

pub async fn delete_prefix(c: &Client, bucket: &str, prefix: &str) -> Result<usize> {
    let keys: Vec<String> = list_all_keys(c, bucket, prefix)
        .await?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    delete_keys(c, bucket, &keys).await
}

pub async fn create_folder(c: &Client, bucket: &str, key: &str) -> Result<()> {
    c.put_object()
        .bucket(bucket)
        .key(key)
        .body(ByteStream::from_static(b""))
        .send()
        .await
        .map_err(sdk_err)?;
    Ok(())
}

/// Progress of one upload batch, shared between the worker task and the UI.
pub struct Progress {
    pub label: String,
    pub started: Instant,
    pub total_bytes: AtomicU64,
    pub done_bytes: AtomicU64,
    pub total_files: AtomicUsize,
    pub done_files: AtomicUsize,
    pub current: Mutex<String>,
    pub cancelled: AtomicBool,
    pub finished: AtomicBool,
}

impl Progress {
    pub fn new(label: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            label: label.into(),
            started: Instant::now(),
            total_bytes: AtomicU64::new(0),
            done_bytes: AtomicU64::new(0),
            total_files: AtomicUsize::new(0),
            done_files: AtomicUsize::new(0),
            current: Mutex::new(String::new()),
            cancelled: AtomicBool::new(false),
            finished: AtomicBool::new(false),
        })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn check_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(anyhow!("upload cancelled"))
        } else {
            Ok(())
        }
    }

    fn add_bytes(&self, n: u64) {
        self.done_bytes.fetch_add(n, Ordering::Relaxed);
    }
}

/// One local file and the key it is uploaded to.
pub struct UploadItem {
    pub path: PathBuf,
    pub key: String,
    pub size: u64,
}

/// Expands files and directories into upload items below `prefix` (directories keep their own name).
/// Blocking: walks the file system.
pub fn plan_upload(paths: &[PathBuf], prefix: &str) -> Result<Vec<UploadItem>> {
    let mut items = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if path.is_dir() {
            for (file, rel) in collect_files(path)? {
                let size = std::fs::metadata(&file)?.len();
                items.push(UploadItem {
                    path: file,
                    key: format!("{prefix}{name}/{rel}"),
                    size,
                });
            }
        } else {
            let size = std::fs::metadata(path)
                .with_context(|| format!("reading {}", path.display()))?
                .len();
            items.push(UploadItem {
                path: path.clone(),
                key: format!("{prefix}{name}"),
                size,
            });
        }
    }
    Ok(items)
}

/// Uploads all items one after another, reporting into `progress`. Returns (files, bytes).
pub async fn upload_all(
    c: &Client,
    bucket: &str,
    items: &[UploadItem],
    progress: &Arc<Progress>,
) -> Result<(usize, u64)> {
    progress
        .total_bytes
        .store(items.iter().map(|i| i.size).sum(), Ordering::Relaxed);
    progress.total_files.store(items.len(), Ordering::Relaxed);
    let mut bytes = 0;
    for item in items {
        progress.check_cancelled()?;
        *progress.current.lock().unwrap() = item.key.clone();
        bytes += upload_file(c, bucket, &item.key, &item.path, progress).await?;
        progress.done_files.fetch_add(1, Ordering::Relaxed);
    }
    Ok((items.len(), bytes))
}

/// Uploads one file, switching to a multipart upload for large files.
pub async fn upload_file(
    c: &Client,
    bucket: &str,
    key: &str,
    path: &Path,
    progress: &Arc<Progress>,
) -> Result<u64> {
    let len = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    if len >= MULTIPART_THRESHOLD {
        return upload_multipart(c, bucket, key, path, len, mime.essence_str(), progress)
            .await
            .with_context(|| format!("uploading {}", path.display()));
    }
    let body = ByteStream::from_path(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    c.put_object()
        .bucket(bucket)
        .key(key)
        .content_type(mime.essence_str())
        .body(body)
        .send()
        .await
        .map_err(sdk_err)
        .with_context(|| format!("uploading {}", path.display()))?;
    progress.add_bytes(len);
    Ok(len)
}

/// Part size for a file: at least `MIN_PART_SIZE`, and large enough to stay within `MAX_PARTS`.
pub fn part_size(len: u64) -> u64 {
    const MIB: u64 = 1024 * 1024;
    let needed = len.div_ceil(MAX_PARTS).div_ceil(MIB) * MIB;
    needed.max(MIN_PART_SIZE)
}

async fn upload_multipart(
    c: &Client,
    bucket: &str,
    key: &str,
    path: &Path,
    len: u64,
    content_type: &str,
    progress: &Arc<Progress>,
) -> Result<u64> {
    let created = c
        .create_multipart_upload()
        .bucket(bucket)
        .key(key)
        .content_type(content_type)
        .send()
        .await
        .map_err(sdk_err)?;
    let upload_id = created
        .upload_id()
        .ok_or_else(|| anyhow!("server returned no upload id"))?
        .to_string();

    let result = upload_parts(c, bucket, key, path, len, &upload_id, progress).await;
    let parts = match result {
        Ok(parts) => parts,
        Err(e) => {
            // Best effort: don't leave orphaned parts behind on the server.
            let _ = c
                .abort_multipart_upload()
                .bucket(bucket)
                .key(key)
                .upload_id(&upload_id)
                .send()
                .await;
            return Err(e);
        }
    };
    let completed = c
        .complete_multipart_upload()
        .bucket(bucket)
        .key(key)
        .upload_id(&upload_id)
        .multipart_upload(
            CompletedMultipartUpload::builder()
                .set_parts(Some(parts))
                .build(),
        )
        .send()
        .await
        .map_err(sdk_err);
    if let Err(e) = completed {
        let _ = c
            .abort_multipart_upload()
            .bucket(bucket)
            .key(key)
            .upload_id(&upload_id)
            .send()
            .await;
        return Err(e);
    }
    Ok(len)
}

/// Uploads all parts with bounded concurrency; returns them sorted by part number.
async fn upload_parts(
    c: &Client,
    bucket: &str,
    key: &str,
    path: &Path,
    len: u64,
    upload_id: &str,
    progress: &Arc<Progress>,
) -> Result<Vec<CompletedPart>> {
    let size = part_size(len);
    let count = len.div_ceil(size);
    let mut parts = Vec::with_capacity(count as usize);
    // Dropping the JoinSet (on error or cancel) aborts the in-flight parts.
    let mut tasks = JoinSet::new();
    let mut next = 0u64;
    while next < count || !tasks.is_empty() {
        while next < count && tasks.len() < PART_CONCURRENCY {
            progress.check_cancelled()?;
            let offset = next * size;
            let part_len = size.min(len - offset);
            let part_number = (next + 1) as i32;
            let (c, bucket, key, path, upload_id) = (
                c.clone(),
                bucket.to_string(),
                key.to_string(),
                path.to_path_buf(),
                upload_id.to_string(),
            );
            let progress = progress.clone();
            tasks.spawn(async move {
                let body = ByteStream::read_from()
                    .path(&path)
                    .offset(offset)
                    .length(Length::Exact(part_len))
                    .build()
                    .await
                    .with_context(|| format!("reading {}", path.display()))?;
                let out = c
                    .upload_part()
                    .bucket(bucket)
                    .key(key)
                    .upload_id(upload_id)
                    .part_number(part_number)
                    .content_length(part_len as i64)
                    .body(body)
                    .send()
                    .await
                    .map_err(sdk_err)
                    .with_context(|| format!("uploading part {part_number}"))?;
                progress.add_bytes(part_len);
                anyhow::Ok(
                    CompletedPart::builder()
                        .set_e_tag(out.e_tag().map(str::to_string))
                        .part_number(part_number)
                        .build(),
                )
            });
            next += 1;
        }
        if let Some(done) = tasks.join_next().await {
            parts.push(done??);
        }
        progress.check_cancelled()?;
    }
    parts.sort_by_key(|p| p.part_number());
    Ok(parts)
}

/// Recursively collects regular files below `dir`, paired with their `/`-separated relative path.
fn collect_files(dir: &Path) -> Result<Vec<(PathBuf, String)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).with_context(|| format!("reading {}", d.display()))? {
            let entry = entry?;
            let ft = entry.file_type()?;
            let path = entry.path();
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                let rel = path.strip_prefix(dir)?;
                let rel = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((path, rel));
            }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}

pub async fn download(c: &Client, bucket: &str, key: &str, dest: &Path) -> Result<u64> {
    let out = c
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .map_err(sdk_err)?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("creating {}", dest.display()))?;
    let mut reader = out.body.into_async_read();
    let n = tokio::io::copy(&mut reader, &mut file)
        .await
        .with_context(|| format!("downloading {key}"))?;
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    Ok(n)
}

/// Converts the part of a key below a prefix into a safe relative path (no `..`, no absolute paths).
fn safe_relative(rel: &str) -> Option<PathBuf> {
    let path: PathBuf = rel.split('/').filter(|s| !s.is_empty()).collect();
    let ok = path.components().all(|c| matches!(c, Component::Normal(_)));
    (ok && path.components().next().is_some()).then_some(path)
}

/// Downloads everything below `prefix` into `dest_dir`, mirroring the folder structure.
pub async fn download_prefix(
    c: &Client,
    bucket: &str,
    prefix: &str,
    dest_dir: &Path,
) -> Result<(usize, u64)> {
    let mut count = 0;
    let mut bytes = 0;
    tokio::fs::create_dir_all(dest_dir).await?;
    for (key, _) in list_all_keys(c, bucket, prefix).await? {
        let rel = &key[prefix.len()..];
        let Some(rel_path) = safe_relative(rel) else {
            continue;
        };
        let dest = dest_dir.join(rel_path);
        if key.ends_with('/') {
            tokio::fs::create_dir_all(&dest).await?;
        } else {
            bytes += download(c, bucket, &key, &dest).await?;
            count += 1;
        }
    }
    Ok((count, bytes))
}

async fn copy_object(c: &Client, bucket: &str, from: &str, to: &str) -> Result<()> {
    let source = format!("{bucket}/{}", urlencoding::encode(from).replace("%2F", "/"));
    c.copy_object()
        .bucket(bucket)
        .copy_source(source)
        .key(to)
        .send()
        .await
        .map_err(sdk_err)
        .with_context(|| format!("copying {from}"))?;
    Ok(())
}

/// Renames (moves) an object, or a whole folder when `from` ends with `/`.
pub async fn rename(c: &Client, bucket: &str, from: &str, to: &str) -> Result<usize> {
    if from == to {
        return Ok(0);
    }
    if from.ends_with('/') {
        if to.starts_with(from) {
            bail!("cannot move a folder into itself");
        }
        let keys = list_all_keys(c, bucket, from).await?;
        for (key, _) in &keys {
            copy_object(c, bucket, key, &format!("{to}{}", &key[from.len()..])).await?;
        }
        let keys: Vec<String> = keys.into_iter().map(|(k, _)| k).collect();
        delete_keys(c, bucket, &keys).await
    } else {
        copy_object(c, bucket, from, to).await?;
        delete_keys(c, bucket, &[from.to_string()]).await
    }
}

pub async fn head(c: &Client, bucket: &str, key: &str) -> Result<ObjectDetails> {
    let out = c
        .head_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .map_err(sdk_err)?;
    Ok(ObjectDetails {
        key: key.to_string(),
        size: out.content_length().unwrap_or(0),
        content_type: out.content_type().map(str::to_string),
        etag: out.e_tag().map(str::to_string),
        modified: out.last_modified().map(fmt_time),
        storage_class: out.storage_class().map(|s| s.as_str().to_string()),
        version_id: out.version_id().map(str::to_string),
        cache_control: out.cache_control().map(str::to_string),
        content_encoding: out.content_encoding().map(str::to_string),
        metadata: out
            .metadata()
            .map(|m| m.clone().into_iter().collect())
            .unwrap_or_default(),
    })
}

fn extension(key: &str) -> String {
    key.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_image(key: &str, content_type: &str) -> bool {
    content_type.starts_with("image/") && !content_type.contains("svg")
        || matches!(
            extension(key).as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
        )
}

fn is_text(key: &str, content_type: &str) -> bool {
    content_type.starts_with("text/")
        || ["json", "xml", "javascript", "yaml", "toml", "csv", "x-sh"]
            .iter()
            .any(|t| content_type.contains(t))
        || matches!(
            extension(key).as_str(),
            "txt"
                | "md"
                | "json"
                | "yaml"
                | "yml"
                | "toml"
                | "csv"
                | "tsv"
                | "xml"
                | "html"
                | "htm"
                | "css"
                | "js"
                | "ts"
                | "rs"
                | "py"
                | "go"
                | "c"
                | "h"
                | "cpp"
                | "java"
                | "sh"
                | "log"
                | "ini"
                | "conf"
                | "cfg"
                | "sql"
                | "svg"
                | "env"
        )
}

pub async fn preview(c: &Client, bucket: &str, details: &ObjectDetails) -> Result<Preview> {
    let ct = details.content_type.as_deref().unwrap_or_default();
    let key = &details.key;
    if is_image(key, ct) {
        if details.size > MAX_IMAGE_PREVIEW {
            return Ok(Preview::None("Image too large to preview".into()));
        }
        let out = c
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(sdk_err)?;
        let data = out.body.collect().await?.into_bytes();
        return Ok(Preview::Image(data.to_vec()));
    }
    let looks_text = is_text(key, ct);
    if details.size == 0 {
        return Ok(Preview::Text {
            text: String::new(),
            truncated: false,
        });
    }
    let out = c
        .get_object()
        .bucket(bucket)
        .key(key)
        .range(format!("bytes=0-{}", TEXT_PREVIEW_BYTES - 1))
        .send()
        .await
        .map_err(sdk_err)?;
    let data = out.body.collect().await?.into_bytes();
    // Sniff unknown types: treat as text if it decodes as UTF-8 without NUL bytes.
    let text = match std::str::from_utf8(&data) {
        Ok(s) => Some(s.to_string()),
        // A truncated read may split a multibyte char at the end.
        Err(e) if e.error_len().is_none() => {
            Some(String::from_utf8_lossy(&data[..e.valid_up_to()]).into_owned())
        }
        Err(_) if looks_text => Some(String::from_utf8_lossy(&data).into_owned()),
        Err(_) => None,
    };
    Ok(match text {
        Some(t) if looks_text || !t.contains('\0') => Preview::Text {
            text: t,
            truncated: details.size > data.len() as i64,
        },
        _ => Preview::None("Binary content, no preview available".into()),
    })
}

pub async fn presign(c: &Client, bucket: &str, key: &str, expires: Duration) -> Result<String> {
    let req = c
        .get_object()
        .bucket(bucket)
        .key(key)
        .presigned(PresigningConfig::expires_in(expires)?)
        .await
        .map_err(sdk_err)?;
    Ok(req.uri().to_string())
}

pub fn human_size(n: i64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_relative_rejects_traversal() {
        assert_eq!(safe_relative("a/b.txt"), Some(PathBuf::from("a/b.txt")));
        assert_eq!(safe_relative("a//b.txt"), Some(PathBuf::from("a/b.txt")));
        assert_eq!(safe_relative("../etc/passwd"), None);
        assert_eq!(safe_relative("a/../../x"), None);
        assert_eq!(safe_relative(""), None);
    }

    #[test]
    fn part_sizes() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(part_size(20 * MIB), 8 * MIB);
        // 100 GiB needs bigger parts to fit in 10,000 of them.
        let len = 100 * 1024 * MIB;
        assert!(len.div_ceil(part_size(len)) <= MAX_PARTS);
        assert_eq!(part_size(len) % MIB, 0);
    }

    #[test]
    fn error_descriptions() {
        assert_eq!(
            describe("AccessDenied", Some("Access Denied")),
            "Access denied: these credentials (or a bucket policy) don't allow this (AccessDenied: Access Denied)"
        );
        assert_eq!(describe("WeirdCode", Some("boom")), "WeirdCode: boom");
        assert_eq!(describe("WeirdCode", None), "WeirdCode");
    }

    /// Unreachable servers produce a short, specific message rather than an SDK dump.
    #[tokio::test]
    async fn unreachable_server() {
        let c = make_client(&Profile {
            endpoint: "http://127.0.0.1:1".into(),
            ..Profile::default()
        });
        let err = list_buckets(&c)
            .await
            .expect_err("nothing listens on port 1")
            .to_string();
        assert!(err.starts_with("Could not reach the server"), "{err}");
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(2048), "2.0 KiB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MiB");
    }

    /// End-to-end test against a live server. Run with e.g.
    /// `LARDER_TEST_ENDPOINT=http://localhost:9000 AWS_ACCESS_KEY_ID=rustfsadmin AWS_SECRET_ACCESS_KEY=rustfsadmin cargo test`
    #[tokio::test]
    async fn live_roundtrip() -> Result<()> {
        let Ok(endpoint) = std::env::var("LARDER_TEST_ENDPOINT") else {
            return Ok(());
        };
        let c = make_client(&Profile {
            endpoint,
            ..Profile::default()
        });
        let bucket = format!("larder-test-{}", std::process::id());
        create_bucket(&c, &bucket, "us-east-1").await?;
        assert!(list_buckets(&c).await?.iter().any(|b| b.name == bucket));

        let tmp = std::env::temp_dir().join(format!("larder-test-{}", std::process::id()));
        let src = tmp.join("src");
        std::fs::create_dir_all(src.join("sub"))?;
        std::fs::write(src.join("hello.txt"), "hello world")?;
        std::fs::write(src.join("sub/data.json"), r#"{"a":1}"#)?;

        create_folder(&c, &bucket, "docs/").await?;
        let progress = Progress::new("test");
        upload_file(
            &c,
            &bucket,
            "docs/hello world+é.txt",
            &src.join("hello.txt"),
            &progress,
        )
        .await?;
        let items = plan_upload(std::slice::from_ref(&src), "")?;
        let (n, _) = upload_all(&c, &bucket, &items, &progress).await?;
        assert_eq!(n, 2);
        assert_eq!(progress.done_bytes.load(Ordering::Relaxed), 11 + 11 + 7);

        let root = list(&c, &bucket, "", false).await?;
        let names: Vec<_> = root.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.contains(&"docs/") && names.contains(&"src/"));
        let docs = list(&c, &bucket, "docs/", false).await?;
        assert_eq!(docs.len(), 1, "folder marker must be hidden");
        assert_eq!(docs[0].size, 11);

        let d = head(&c, &bucket, "docs/hello world+é.txt").await?;
        assert_eq!(d.content_type.as_deref(), Some("text/plain"));
        match preview(&c, &bucket, &d).await? {
            Preview::Text { text, truncated } => assert!(text == "hello world" && !truncated),
            _ => panic!("expected text preview"),
        }

        // Rename a file with special characters, then a whole folder.
        rename(&c, &bucket, "docs/hello world+é.txt", "docs/renamed.txt").await?;
        assert_eq!(
            list(&c, &bucket, "docs/", false).await?[0].name,
            "renamed.txt"
        );
        rename(&c, &bucket, "src/", "moved/").await?;
        assert_eq!(list(&c, &bucket, "moved/", true).await?.len(), 2);
        assert!(list(&c, &bucket, "src/", true).await?.is_empty());

        let dl = tmp.join("dl");
        let (n, bytes) = download_prefix(&c, &bucket, "moved/", &dl).await?;
        assert_eq!((n, bytes), (2, 18));
        assert_eq!(
            std::fs::read_to_string(dl.join("sub/data.json"))?,
            r#"{"a":1}"#
        );

        let url = presign(&c, &bucket, "docs/renamed.txt", Duration::from_secs(60)).await?;
        assert!(url.contains("X-Amz-Signature"));

        // Multipart: 3 parts (8 + 8 + 4 MiB) with distinct content per region of the file.
        let big = tmp.join("big.bin");
        let data: Vec<u8> = (0..20 * 1024 * 1024u32).map(|i| (i / 4093) as u8).collect();
        std::fs::write(&big, &data)?;
        let progress = Progress::new("big");
        let items = plan_upload(std::slice::from_ref(&big), "large/")?;
        upload_all(&c, &bucket, &items, &progress).await?;
        assert_eq!(
            progress.done_bytes.load(Ordering::Relaxed),
            data.len() as u64
        );
        let info = head(&c, &bucket, "large/big.bin").await?;
        assert_eq!(info.size, data.len() as i64);
        assert!(
            info.etag.as_deref().unwrap_or_default().ends_with("-3\""),
            "not multipart: {:?}",
            info.etag
        );
        download(&c, &bucket, "large/big.bin", &tmp.join("big.out")).await?;
        assert!(
            std::fs::read(tmp.join("big.out"))? == data,
            "multipart round trip corrupted data"
        );

        // Cancelling aborts the multipart upload and leaves no object or pending upload behind.
        let progress = Progress::new("cancelled");
        progress.cancel();
        assert!(
            upload_file(&c, &bucket, "large/cancelled.bin", &big, &progress)
                .await
                .is_err()
        );
        assert!(head(&c, &bucket, "large/cancelled.bin").await.is_err());
        let pending = c
            .list_multipart_uploads()
            .bucket(&bucket)
            .send()
            .await
            .map_err(sdk_err)?;
        assert!(pending.uploads().is_empty(), "aborted upload still listed");

        let err = delete_bucket(&c, &bucket, false)
            .await
            .expect_err("non-empty bucket must not delete");
        assert!(err.to_string().contains("still contains objects"), "{err}");
        let err = head(&c, &bucket, "nope.txt")
            .await
            .expect_err("missing object");
        assert!(err.to_string().contains("doesn't exist"), "{err}");
        delete_bucket(&c, &bucket, true).await?;
        assert!(!list_buckets(&c).await?.iter().any(|b| b.name == bucket));
        std::fs::remove_dir_all(tmp)?;
        Ok(())
    }
}
