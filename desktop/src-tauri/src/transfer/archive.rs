use super::*;
use async_deflate_zip::{EntryOptions, ZipWriter};
use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use std::os::windows::fs::MetadataExt;
use tokio_util::io::ReaderStream;

// Bound both metadata and buffered bytes; file contents are streamed by the ZIP library.
const BUFFER_BYTES: usize = 256 * 1024;

fn entries(root: &Path, cancel: &AtomicBool) -> Result<Vec<(String, bool)>, AppError> {
    let mut result = Vec::new();
    let mut pending = vec![String::new()];
    let mut total = 0u64;
    while let Some(parent) = pending.pop() {
        check_cancel(cancel)?;
        let directory = if parent.is_empty() {
            root.to_path_buf()
        } else {
            web::destination(root, &parent)?
        };
        ordinary(&directory)?;
        for child in fs::read_dir(directory).map_err(io_error)? {
            check_cancel(cancel)?;
            let child = child.map_err(io_error)?;
            let name = child
                .file_name()
                .into_string()
                .map_err(|_| error("transfer_path", "Invalid Unicode file name"))?;
            if name.to_ascii_lowercase().starts_with(".wmcp-") {
                continue;
            }
            let metadata = fs::symlink_metadata(child.path()).map_err(io_error)?;
            if metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0 {
                continue;
            }
            ordinary(&child.path())?;
            let entry_path = if parent.is_empty() {
                name
            } else {
                format!("{parent}/{name}")
            };
            relative(&entry_path)?;
            if result.len() >= MAX_ENTRIES {
                return Err(error("transfer_limit", "At most 10000 ZIP entries"));
            }
            if metadata.is_file() {
                total = total.saturating_add(metadata.len());
                if total > MAX_TOTAL_BYTES {
                    return Err(error("transfer_limit", "ZIP source exceeds 1 TiB"));
                }
            } else {
                pending.push(entry_path.clone());
            }
            result.push((entry_path, metadata.is_dir()));
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

async fn write(
    writer: tokio::io::DuplexStream,
    root: PathBuf,
    empty_name: String,
    cancel: Arc<AtomicBool>,
) -> Result<(), AppError> {
    let scan_root = root.clone();
    let scan_cancel = cancel.clone();
    let entries = tokio::task::spawn_blocking(move || entries(&scan_root, &scan_cancel))
        .await
        .map_err(io_error)??;
    let mut zip = ZipWriter::new(writer);
    if entries.is_empty() {
        zip.add_directory(&format!("{empty_name}/"), &EntryOptions::directory())
            .await
            .map_err(io_error)?;
    }
    for (name, directory) in entries {
        check_cancel(&cancel)?;
        ordinary(&root)?;
        let source = web::destination(&root, &name)?;
        if directory {
            let options = EntryOptions::from_path(&source).await.map_err(io_error)?;
            zip.add_directory(&format!("{name}/"), &options)
                .await
                .map_err(io_error)?;
        } else {
            zip.add_file(&name, &source).await.map_err(io_error)?;
        }
    }
    check_cancel(&cancel)?;
    zip.finish().await.map_err(io_error)?;
    Ok(())
}

pub(super) fn stream(
    root: PathBuf,
    empty_name: String,
    cancel: Arc<AtomicBool>,
) -> impl Stream<Item = Result<Bytes, AppError>> + Send {
    let (writer, reader) = tokio::io::duplex(BUFFER_BYTES);
    let producer = tauri::async_runtime::spawn(write(writer, root, empty_name, cancel));
    // EOF alone is insufficient: propagate producer failures rather than completing a bad ZIP.
    ReaderStream::with_capacity(reader, BUFFER_BYTES)
        .map(|result| result.map_err(io_error))
        .chain(futures_util::stream::once(async move {
            producer.await.map_err(io_error)??;
            Ok(Bytes::new())
        }))
}
