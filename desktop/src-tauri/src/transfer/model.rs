use crate::{AppError, config, platform};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024 * 1024;

pub fn error(code: &str, detail: impl Into<String>) -> AppError {
    AppError::new(code, detail)
}
pub fn io_error(e: impl std::fmt::Display) -> AppError {
    error("transfer_io", e.to_string())
}
pub fn id() -> Result<String, AppError> {
    Ok(config::generate_token()?[..24].to_string())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub url: String,
    pub token: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub port: u16,
    pub lan_enabled: bool,
    pub shared_dir: String,
    pub token: String,
    pub peers: Vec<Peer>,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPatch {
    pub port: Option<u16>,
    pub lan_enabled: Option<bool>,
    pub shared_dir: Option<String>,
    pub token: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerInput {
    pub name: String,
    pub url: String,
    pub token: String,
}

pub fn valid_token(token: &str) -> bool {
    (32..=256).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_graphic())
}
pub fn peer_url(input: &str) -> Result<String, AppError> {
    let url =
        reqwest::Url::parse(input).map_err(|_| error("transfer_peer", "Use http://LAN-IP:port"))?;
    let ip = url
        .host_str()
        .unwrap_or("")
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .map_err(|_| {
            error(
                "transfer_peer",
                "An explicit local or LAN IP address is required",
            )
        })?;
    let local = match ip {
        std::net::IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local(),
        std::net::IpAddr::V6(v) => {
            v.is_loopback() || v.is_unique_local() || v.is_unicast_link_local()
        }
    };
    if !local
        || url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
        || url.port() == Some(0)
    {
        return Err(error(
            "transfer_peer",
            "Use http://LAN-IP:port without credentials or a path",
        ));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}
impl Settings {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.port == 0 {
            return Err(error("invalid_port", ""));
        }
        if !valid_token(&self.token) {
            return Err(error("invalid_token", ""));
        }
        if !Path::new(&self.shared_dir).is_absolute() {
            return Err(error(
                "transfer_directory",
                "Choose an absolute receive directory",
            ));
        }
        if self.peers.len() > 50 {
            return Err(error("transfer_peer", "At most 50 saved peers"));
        }
        for peer in &self.peers {
            peer_url(&peer.url)?;
            if !valid_token(&peer.token) {
                return Err(error("invalid_token", ""));
            }
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u8,
    protected: Vec<u8>,
}
pub fn load(directory: &Path, default_root: &Path) -> Result<Settings, AppError> {
    let path = directory.join("transfer.json");
    if !path.exists() {
        let settings = Settings {
            port: 8002,
            lan_enabled: true,
            shared_dir: default_root.to_string_lossy().into(),
            token: config::generate_token()?,
            peers: vec![],
        };
        save(directory, &settings)?;
        return Ok(settings);
    }
    let stored: Stored =
        serde_json::from_slice(&fs::read(path).map_err(io_error)?).map_err(io_error)?;
    if stored.version != 1 {
        return Err(error("config_version", ""));
    }
    let settings: Settings =
        serde_json::from_slice(&platform::unprotect(&stored.protected)?).map_err(io_error)?;
    settings.validate()?;
    Ok(settings)
}
pub fn save(directory: &Path, settings: &Settings) -> Result<(), AppError> {
    settings.validate()?;
    fs::create_dir_all(directory).map_err(io_error)?;
    let protected = platform::protect(&serde_json::to_vec(settings).map_err(io_error)?)?;
    platform::atomic_write(
        &directory.join("transfer.json"),
        &serde_json::to_vec_pretty(&Stored {
            version: 1,
            protected,
        })
        .map_err(io_error)?,
    )
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub directory: bool,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub directory: bool,
    pub entries: Vec<Entry>,
}
pub fn component(value: &str) -> Result<(), AppError> {
    let stem = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    if value.is_empty()
        || value.len() > 240
        || value == "."
        || value == ".."
        || value.ends_with(['.', ' '])
        || value.to_ascii_lowercase().starts_with(".wmcp-")
        || value
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(error(
            "transfer_path",
            "Unsafe or unsupported Windows file name",
        ));
    }
    Ok(())
}
pub fn relative(value: &str) -> Result<(), AppError> {
    if value.len() > 2048 {
        return Err(error("transfer_path", "Path is too long"));
    }
    for part in value.split('/') {
        component(part)?;
    }
    Ok(())
}
pub fn ordinary(path: &Path) -> Result<fs::Metadata, AppError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink()
        || metadata.file_attributes() & 0x400 != 0
        || !(metadata.is_file() || metadata.is_dir())
    {
        return Err(error(
            "transfer_path",
            "Links, junctions and special files are not transferred",
        ));
    }
    Ok(metadata)
}
pub fn check_cancel(flag: &AtomicBool) -> Result<(), AppError> {
    if flag.load(Ordering::Relaxed) {
        Err(error("transfer_cancelled", ""))
    } else {
        Ok(())
    }
}
pub fn file_hash(path: &Path, flag: &AtomicBool) -> Result<String, AppError> {
    let mut file = fs::File::open(path).map_err(io_error)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 256 * 1024];
    loop {
        check_cancel(flag)?;
        let count = file.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
impl Manifest {
    pub fn validate(&self) -> Result<u64, AppError> {
        component(&self.name)?;
        if self.entries.len() > MAX_ENTRIES {
            return Err(error(
                "transfer_limit",
                "At most 10000 entries per transfer",
            ));
        }
        if !self.directory
            && (self.entries.len() != 1
                || self.entries[0].directory
                || self.entries[0].path != self.name)
        {
            return Err(error("transfer_path", "Invalid single-file manifest"));
        }
        let mut seen = HashSet::new();
        let mut total = 0u64;
        for entry in &self.entries {
            relative(&entry.path)?;
            if !seen.insert(entry.path.to_lowercase())
                || (entry.directory && (entry.size != 0 || !entry.sha256.is_empty()))
                || (!entry.directory
                    && (entry.sha256.len() != 64
                        || !entry
                            .sha256
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())))
            {
                return Err(error(
                    "transfer_path",
                    "Invalid or colliding manifest entries",
                ));
            }
            total = total
                .checked_add(entry.size)
                .ok_or_else(|| error("transfer_limit", "Size overflow"))?;
            if total > MAX_TOTAL_BYTES {
                return Err(error("transfer_limit", "Transfer exceeds 1 TiB"));
            }
        }
        let files: HashSet<_> = self
            .entries
            .iter()
            .filter(|e| !e.directory)
            .map(|e| e.path.to_lowercase())
            .collect();
        for entry in &self.entries {
            let mut parent = entry.path.as_str();
            while let Some((prefix, _)) = parent.rsplit_once('/') {
                if files.contains(&prefix.to_lowercase()) {
                    return Err(error("transfer_path", "File/directory collision"));
                }
                parent = prefix;
            }
        }
        Ok(total)
    }
}
pub struct Plan {
    pub manifest: Manifest,
    pub files: Vec<(usize, PathBuf)>,
}
pub fn plan(paths: &[String], cancel: &Arc<AtomicBool>) -> Result<Plan, AppError> {
    if paths.is_empty() || paths.len() > MAX_ENTRIES {
        return Err(error("transfer_path", "Select files or folders"));
    }
    let sources: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    for path in &sources {
        if !path.is_absolute() {
            return Err(error("transfer_path", "Use absolute source paths"));
        }
        ordinary(path)?;
    }
    let single = sources.len() == 1;
    let directory = !single || ordinary(&sources[0])?.is_dir();
    let name = if single {
        sources[0]
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| error("transfer_path", "Invalid name"))?
            .to_string()
    } else {
        format!("Transfer-{}", &id()?[..8])
    };
    component(&name)?;
    let mut result = Plan {
        manifest: Manifest {
            name,
            directory,
            entries: vec![],
        },
        files: vec![],
    };
    fn visit(
        source: &Path,
        rel: String,
        result: &mut Plan,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(), AppError> {
        check_cancel(cancel)?;
        if result.manifest.entries.len() >= MAX_ENTRIES {
            return Err(error(
                "transfer_limit",
                "At most 10000 entries per transfer",
            ));
        }
        let metadata = ordinary(source)?;
        if !rel.is_empty() {
            relative(&rel)?;
            let index = result.manifest.entries.len();
            let sha256 = if metadata.is_file() {
                file_hash(source, cancel)?
            } else {
                String::new()
            };
            result.manifest.entries.push(Entry {
                path: rel.clone(),
                directory: metadata.is_dir(),
                size: if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                },
                sha256,
            });
            if metadata.is_file() {
                result.files.push((index, source.to_path_buf()));
            }
        }
        if metadata.is_dir() {
            let mut children = fs::read_dir(source)
                .map_err(io_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(io_error)?;
            children.sort_by_key(|entry| entry.file_name());
            for child in children {
                let name = child
                    .file_name()
                    .into_string()
                    .map_err(|_| error("transfer_path", "Invalid Unicode file name"))?;
                let next = if rel.is_empty() {
                    name
                } else {
                    format!("{rel}/{name}")
                };
                visit(&child.path(), next, result, cancel)?;
            }
        }
        Ok(())
    }
    for source in &sources {
        let rel = if single && directory {
            String::new()
        } else {
            source
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| error("transfer_path", "Invalid source"))?
                .to_string()
        };
        visit(source, rel, &mut result, cancel)?;
    }
    result.manifest.validate()?;
    Ok(result)
}
