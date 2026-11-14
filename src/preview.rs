//! What a previewed operation and its run share: the key that binds a confirmation to what was
//! previewed, a file's version, a private copy of a store to read from, a store opened read only,
//! the home's settings as bytes, and the code a failure is told by.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureCode {
    Stale,
    InvalidConfig,
    Busy,
    Refused,
    Failed,
}

impl std::fmt::Display for FailureCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for FailureCode {}

pub(crate) fn failure_code(cause: &anyhow::Error, fallback: FailureCode) -> FailureCode {
    cause
        .downcast_ref::<FailureCode>()
        .copied()
        .unwrap_or(fallback)
}

/// Bind consent to this code/ruleset, destination file identity, scope and effects.
pub(crate) fn preview_key(home: &Path, scope: &impl serde::Serialize) -> Result<String> {
    let identity = absolute_identity(home)?;
    let bytes = serde_json::to_vec(&(
        "settings-w5a-v1",
        env!("CARGO_PKG_VERSION"),
        crate::redact::Rules::default().version(),
        identity,
        crate::db::store_file(&crate::raw::path(home)),
        scope,
    ))?;
    Ok(crate::forget::hash(&bytes))
}

pub(crate) fn absolute_identity(path: &Path) -> Result<PathBuf> {
    if path.try_exists()? {
        return Ok(path.canonicalize()?);
    }
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent = path.parent().context("path has no parent")?;
    Ok(absolute_identity(parent)?.join(path.file_name().context("path has no name")?))
}

pub(crate) fn file_version(path: &Path) -> Result<String> {
    file_version_in(std::fs::File::open(path)?)
}

pub(crate) fn file_version_in(mut reader: impl std::io::Read) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buf = [0; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) type SourceStamp = (PathBuf, String, u64, std::time::SystemTime, String);

#[derive(Debug)]
pub(crate) enum PreviewCopyError {
    Changed,
    Cleanup,
}

impl std::fmt::Display for PreviewCopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Changed => "a store changed during preview",
            Self::Cleanup => "remove the private preview copy",
        })
    }
}

impl std::error::Error for PreviewCopyError {}

pub(crate) struct OpenedSource {
    pub(crate) file: std::fs::File,
    pub(crate) stamp: SourceStamp,
}

impl OpenedSource {
    pub(crate) fn new(path: PathBuf, file: std::fs::File) -> Result<Self> {
        Self::bounded(path, file, u64::MAX)
    }

    pub(crate) fn bounded(path: PathBuf, mut file: std::fs::File, limit: u64) -> Result<Self> {
        use std::io::Read;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > limit {
            return Err(std::io::Error::from(std::io::ErrorKind::Unsupported).into());
        }
        let id = crate::db::store_file_from(&file)?;
        let hash = file_version_in((&mut file).take(metadata.len().saturating_add(1)))?;
        let after = file.metadata()?;
        if metadata.len() != after.len() || metadata.modified()? != after.modified()? {
            return Err(PreviewCopyError::Changed.into());
        }
        Ok(Self {
            file,
            stamp: (path, id, metadata.len(), metadata.modified()?, hash),
        })
    }
}

/// Copies admitted descriptors only. The caller owns source selection and outside-home admission.
pub(crate) fn with_preview_files<T>(
    db: OpenedSource,
    wal: Option<OpenedSource>,
    inspect: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    use std::io::{Read, Seek};
    let scratch = crate::provider::scratch_dir()
        .map_err(|_| anyhow::anyhow!("cannot create a private preview directory"))?;
    let result = (|| {
        for (index, mut source) in std::iter::once(db).chain(wal).enumerate() {
            let path = scratch.0.join(if index == 0 {
                "oboete.db"
            } else {
                "oboete.db-wal"
            });
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut copy = options.open(&path)?;
            source.file.rewind()?;
            let bytes = std::io::copy(
                &mut (&mut source.file).take(source.stamp.2.saturating_add(1)),
                &mut copy,
            )
            .context("copy a store for preview")?;
            drop(copy);
            let metadata = source.file.metadata()?;
            if bytes != source.stamp.2
                || metadata.len() != source.stamp.2
                || metadata.modified()? != source.stamp.3
                || crate::db::store_file_from(&source.file)? != source.stamp.1
                || file_version(&path)? != source.stamp.4
            {
                return Err(PreviewCopyError::Changed.into());
            }
        }
        let conn = open_v1(&scratch.0.join("oboete.db"))?;
        conn.execute_batch("BEGIN")?;
        inspect(&conn)
    })();
    std::fs::remove_dir_all(&scratch.0).context(PreviewCopyError::Cleanup)?;
    result
}

/// SQLite may create sidecars even on a read-only open. Only open a stable private DB/WAL copy.
/// Deliberately schema-neutral: a restore's preview reads raw.db and knowledge.db through it.
pub(crate) fn with_preview_v1<T>(
    from: &Path,
    inspect: impl FnOnce(&Connection, &str) -> Result<T>,
) -> Result<T> {
    let parent = from
        .parent()
        .context("store has no parent")?
        .join(".")
        .canonicalize()?;
    anyhow::ensure!(
        !std::env::temp_dir().canonicalize()?.starts_with(parent),
        "preview needs a temporary directory outside the oboete home"
    );
    let source = from.canonicalize()?;
    let mut wal = source.as_os_str().to_owned();
    wal.push("-wal");
    let wal = PathBuf::from(wal);
    let versions = || -> Result<Vec<SourceStamp>> {
        let mut paths = vec![source.clone()];
        if wal.try_exists()? {
            paths.push(wal.clone());
        }
        paths
            .into_iter()
            .map(|p| {
                let m = std::fs::metadata(&p)?;
                Ok((
                    p.clone(),
                    crate::db::store_file(&p),
                    m.len(),
                    m.modified()?,
                    file_version(&p)?,
                ))
            })
            .collect()
    };
    let before = versions()?;
    let opened = before
        .iter()
        .map(|stamp| OpenedSource::new(stamp.0.clone(), std::fs::File::open(&stamp.0)?))
        .collect::<Result<Vec<_>>>()?;
    anyhow::ensure!(
        opened.iter().map(|s| &s.stamp).eq(before.iter()),
        "a store changed during preview"
    );
    let version = crate::forget::hash(&serde_json::to_vec(&before)?);
    let mut opened = opened.into_iter();
    let db = opened.next().context("missing preview database")?;
    with_preview_files(db, opened.next(), |conn| {
        anyhow::ensure!(
            before == versions()? && from.canonicalize()? == source,
            "a store changed during preview"
        );
        inspect(conn, &version)
    })
}

pub(crate) fn config_bytes(home: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(home.join("config.toml")) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// A store, read only, with the stores' busy wait: a preview's private copy of one. Never
/// `immutable`, which skips the WAL, where the newest events are.
pub fn open_v1(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_millis(2_000))?;
    Ok(conn)
}

// Its one test reads /proc: the module is Linux's, so that no import is unused elsewhere.
#[cfg(all(test, target_os = "linux"))]
mod tests;
