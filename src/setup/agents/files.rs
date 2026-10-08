use crate::setup::{
    DiagnosticDirectory, diagnostic_canonicalize, diagnostic_metadata, diagnostic_read_file,
};
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, Metadata},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    time::SystemTime,
};

const LIMIT: u64 = 1024 * 1024;

// Private to the setup implementation. Never serialize the text or paths.
pub(super) struct Input {
    pub(super) path: PathBuf,
    pub(super) text: Option<String>,
    pub(super) witness: String,
    target: PathBuf,
    bindings: BTreeMap<PathBuf, Binding>,
    source: Option<Stamp>,
}
#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    identity: String,
    mode: u32,
    len: u64,
    modified: SystemTime,
}
#[derive(PartialEq, Eq)]
struct Binding {
    link: bool,
    key: String,
}

fn hash(fields: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for bytes in fields {
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    format!("{:x}", h.finalize())
}
pub(super) fn mode(m: &Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        m.mode()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes()
    }
}
fn stamp(file: &File) -> Result<Stamp> {
    let m = file.metadata()?;
    ensure!(m.is_file(), "input is not a regular file");
    Ok(Stamp {
        identity: crate::db::store_file_from(file)?,
        mode: mode(&m),
        len: m.len(),
        modified: m.modified()?,
    })
}
fn own_metadata(path: &Path) -> std::io::Result<Metadata> {
    #[cfg(unix)]
    {
        std::fs::symlink_metadata(path)
    }
    #[cfg(not(unix))]
    {
        diagnostic_metadata(path)
    }
}

// Existing aliases are supported, including an alias whose selected target is not yet present.
// For an ordinary missing tail, bind the first existing physical ancestor and
// retain every missing component; do not collapse '..' across a missing entry.
fn target(path: &Path) -> Result<PathBuf> {
    let mut current = path.to_owned();
    let mut tail = Vec::new();
    #[cfg(unix)]
    let mut aliases = 0;
    loop {
        match diagnostic_canonicalize(&current) {
            Ok(mut resolved) => {
                if !tail.is_empty() {
                    ensure!(
                        diagnostic_metadata(&resolved)?.is_dir(),
                        "input parent is not a directory"
                    );
                }
                for leaf in tail.into_iter().rev() {
                    resolved.push(leaf);
                }
                return Ok(resolved);
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                #[cfg(unix)]
                if std::fs::symlink_metadata(&current)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    aliases += 1;
                    ensure!(aliases <= 40, "too many input aliases");
                    let to = std::fs::read_link(&current)?;
                    current = if to.is_absolute() {
                        to
                    } else {
                        current
                            .parent()
                            .ok_or_else(|| anyhow::anyhow!("alias has no parent"))?
                            .join(to)
                    };
                    continue;
                }
                let leaf = current
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!("unresolved input path"))?
                    .to_owned();
                tail.push(leaf);
                ensure!(current.pop(), "no input ancestor");
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn bindings(logical: &Path, physical: &Path) -> Result<BTreeMap<PathBuf, Binding>> {
    let mut result: BTreeMap<PathBuf, Binding> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut pending = vec![logical.to_owned(), physical.to_owned()];
    #[cfg(unix)]
    let mut links = 0;
    while let Some(path) = pending.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        for prefix in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
            if result.contains_key(prefix) {
                continue;
            }
            let m = match own_metadata(prefix) {
                Ok(m) => m,
                Err(e) if e.kind() == ErrorKind::NotFound => break,
                Err(e) => return Err(e.into()),
            };
            #[cfg(unix)]
            if m.file_type().is_symlink() {
                use std::os::unix::fs::MetadataExt;
                links += 1;
                ensure!(links <= 40, "too many input aliases");
                let to = std::fs::read_link(prefix)?;
                let next = if to.is_absolute() {
                    to.clone()
                } else {
                    prefix
                        .parent()
                        .ok_or_else(|| anyhow::anyhow!("alias has no parent"))?
                        .join(&to)
                };
                let identity = format!("{}:{}", m.dev(), m.ino());
                let key = hash(&[
                    b"symlink",
                    identity.as_bytes(),
                    &mode(&m).to_le_bytes(),
                    to.as_os_str().as_encoded_bytes(),
                ]);
                result.insert(prefix.to_owned(), Binding { link: true, key });
                pending.push(next);
                continue;
            }
            if m.is_dir() {
                let dir = DiagnosticDirectory::open(prefix)?;
                let id = crate::db::store_file_from(&dir.file)?;
                let key = hash(&[
                    b"directory",
                    id.as_bytes(),
                    &mode(&dir.file.metadata()?).to_le_bytes(),
                    dir.resolved.as_os_str().as_encoded_bytes(),
                ]);
                result.insert(prefix.to_owned(), Binding { link: false, key });
            } else {
                ensure!(
                    prefix == path && m.is_file(),
                    "input route is not a regular file or directory"
                );
            }
        }
    }
    // On Windows all prefix probes above use the native no-reparse guard.
    #[cfg(unix)]
    let _ = links;
    Ok(result)
}

fn read(path: &Path) -> Result<(Option<String>, Option<Stamp>)> {
    let m = match diagnostic_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok((None, None)),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        m.is_file() && m.len() <= LIMIT,
        "input is not a bounded regular file"
    );
    let mut file = diagnostic_read_file(path)?;
    let before = stamp(&file)?;
    ensure!(before.len <= LIMIT, "input is too large");
    let mut bytes = Vec::new();
    (&mut file).take(LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= LIMIT && bytes.len() as u64 == before.len && before == stamp(&file)?,
        "input changed while reading"
    );
    Ok((Some(String::from_utf8(bytes)?), Some(before)))
}
fn current_stamp(path: &Path) -> Result<Option<Stamp>> {
    match diagnostic_metadata(path) {
        Ok(m) => {
            ensure!(m.is_file(), "input is no longer a regular file");
            Ok(Some(stamp(&diagnostic_read_file(path)?)?))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub(super) fn input(path: &Path) -> Result<Input> {
    ensure!(path.is_absolute(), "input path must be absolute");
    // Run the guarded route probe before canonicalizing. It also distinguishes
    // dangling symlinks from ordinary absence without reading their data.
    let _ = bindings(path, path)?;
    let physical = target(path)?;
    let before = bindings(path, &physical)?;
    let (text, source) = read(&physical)?;
    ensure!(
        before == bindings(path, &physical)?
            && source == current_stamp(&physical)?
            && physical == target(path)?,
        "input binding changed"
    );
    let mut fields = vec![
        b"oboete-w6-input-v1".to_vec(),
        path.as_os_str().as_encoded_bytes().to_vec(),
        physical.as_os_str().as_encoded_bytes().to_vec(),
    ];
    match (&text, &source) {
        (Some(text), Some(s)) => {
            fields.extend([
                b"present".to_vec(),
                s.identity.as_bytes().to_vec(),
                s.mode.to_le_bytes().to_vec(),
                s.len.to_le_bytes().to_vec(),
                format!("{:?}", s.modified).into_bytes(),
                text.as_bytes().to_vec(),
            ]);
        }
        (None, None) => fields.push(b"missing".to_vec()),
        _ => unreachable!(),
    }
    for (path, binding) in &before {
        fields.push(path.as_os_str().as_encoded_bytes().to_vec());
        fields.push(binding.key.as_bytes().to_vec());
    }
    let witness = hash(&fields.iter().map(Vec::as_slice).collect::<Vec<_>>());
    Ok(Input {
        path: path.to_owned(),
        text,
        witness,
        target: physical,
        bindings: before,
        source,
    })
}

impl Input {
    pub(super) fn present_entry(&self) -> bool {
        self.source.is_some()
            || self
                .bindings
                .get(&self.path)
                .is_some_and(|binding| binding.link)
    }
    pub(super) fn target_path(&self) -> &Path {
        &self.target
    }

    pub(super) fn unchanged(&self) -> bool {
        input(&self.path).is_ok_and(|now| {
            self.same_route(&now) && self.text == now.text && self.source == now.source
        })
    }

    pub(super) fn same_route(&self, now: &Self) -> bool {
        self.path == now.path && self.target == now.target
            && self.bindings.iter().all(|(path, binding)| now.bindings.get(path) == Some(binding))
            // Staging/native writes may create ordinary parents along the agreed target.
            && now.bindings.iter().all(|(path, binding)| self.bindings.contains_key(path)
                || (!binding.link && target(path).is_ok_and(|physical| self.target.starts_with(&physical) && physical != self.target)))
    }

    pub(super) fn after_unlink(&self, now: &Self) -> bool {
        self.path == now.path
            && now.text.is_none()
            && !now.present_entry()
            && self
                .bindings
                .iter()
                .filter(|(path, _)| **path != self.path)
                .all(|(path, binding)| {
                    bindings(path, path).is_ok_and(|current| current.get(path) == Some(binding))
                })
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use std::fs;

    #[test]
    fn w6a_input_missing_and_empty_are_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config");
        let missing = input(&path).unwrap();
        assert!(missing.text.is_none());
        assert!(!missing.present_entry());
        assert!(missing.unchanged());
        fs::write(&path, "").unwrap();
        let empty = input(&path).unwrap();
        assert_eq!(empty.text.as_deref(), Some(""));
        assert!(empty.present_entry());
        assert_ne!(missing.witness, empty.witness);
        assert!(!missing.unchanged());
    }

    #[test]
    fn w6a_input_detects_content_and_same_bytes_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config");
        fs::write(&path, "first").unwrap();
        let first = input(&path).unwrap();
        fs::write(&path, "other").unwrap();
        assert!(!first.unchanged());
        let old = input(&path).unwrap();
        let metadata = fs::metadata(&path).unwrap();
        // Retain the old inode, and restore content, permissions and modified time.
        fs::rename(&path, temp.path().join("original-retained")).unwrap();
        fs::write(&path, "other").unwrap();
        fs::set_permissions(&path, metadata.permissions()).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
            .unwrap();
        let replacement = input(&path).unwrap();
        assert_eq!(old.text, replacement.text);
        assert_ne!(old.witness, replacement.witness);
        assert!(!old.unchanged());
    }

    #[cfg(unix)]
    #[test]
    fn w6a_input_detects_mode_drift() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config");
        fs::write(&path, "same").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let before = input(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(!before.unchanged());
        assert_ne!(before.witness, input(&path).unwrap().witness);
    }

    #[test]
    fn w6a_input_allows_native_stage_to_create_ordinary_parents() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("new/nested/config");
        let before = input(&path).unwrap();
        assert!(!path.parent().unwrap().exists());
        let staged = crate::setup::stage(before.target_path(), "created").unwrap();
        assert!(path.parent().unwrap().is_dir());
        assert!(!path.exists());
        assert!(before.unchanged());
        staged.commit().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "created");
        assert!(!before.unchanged());
    }

    #[cfg(unix)]
    #[test]
    fn w6a_input_follows_existing_alias_and_detects_retarget() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let alias = temp.path().join("alias");
        fs::write(temp.path().join("a"), "same").unwrap();
        fs::write(temp.path().join("b"), "same").unwrap();
        symlink("a", &alias).unwrap();
        let before = input(&alias).unwrap();
        assert_eq!(before.text.as_deref(), Some("same"));
        assert!(before.present_entry());
        assert!(before.unchanged());
        assert_eq!(
            before.target_path(),
            temp.path().join("a").canonicalize().unwrap()
        );
        fs::remove_file(&alias).unwrap();
        symlink("b", &alias).unwrap();
        assert!(!before.unchanged());
        assert_ne!(before.witness, input(&alias).unwrap().witness);
    }

    #[cfg(unix)]
    #[test]
    fn w6a_input_stages_through_a_dangling_relative_file_alias() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let alias = temp.path().join("alias");
        symlink("future/config", &alias).unwrap();
        let before = input(&alias).unwrap();
        assert!(before.text.is_none());
        assert!(before.present_entry());
        assert_eq!(
            before.target_path(),
            temp.path().canonicalize().unwrap().join("future/config")
        );
        let staged = crate::setup::stage(before.target_path(), "created").unwrap();
        assert!(before.unchanged());
        staged.commit().unwrap();
        assert!(
            fs::symlink_metadata(&alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&alias).unwrap(), "created");
        assert!(!before.unchanged());
    }

    #[cfg(unix)]
    #[test]
    fn w6a_input_allows_new_parents_through_directory_alias_but_not_redirect() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        let other = temp.path().join("other");
        fs::create_dir(&real).unwrap();
        fs::create_dir(&other).unwrap();
        symlink("real", temp.path().join("alias")).unwrap();
        let before = input(&temp.path().join("alias/nested/config")).unwrap();
        let staged = crate::setup::stage(before.target_path(), "staged").unwrap();
        assert!(before.unchanged());
        drop(staged);
        fs::remove_dir(real.join("nested")).unwrap();
        symlink(&other, real.join("nested")).unwrap();
        assert!(!before.unchanged());
        assert!(!other.join("config").exists());
    }

    #[test]
    fn w6a_input_refuses_invalid_utf8_and_excess_length() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config");
        fs::write(&path, [0xff]).unwrap();
        assert!(input(&path).is_err());
        fs::write(&path, vec![b'x'; LIMIT as usize]).unwrap();
        assert!(input(&path).is_ok());
        fs::write(&path, vec![b'x'; LIMIT as usize + 1]).unwrap();
        assert!(input(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn w6a_input_refuses_known_fifo_without_waiting_for_a_writer() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt, sync::mpsc, time::Duration};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.fifo");
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: the owned, NUL-terminated temporary pathname remains live for mkfifo.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(input(&path).is_err()).unwrap();
        });
        assert!(
            rx.recv_timeout(Duration::from_secs(5))
                .expect("FIFO input waited for a writer")
        );
        worker.join().unwrap();
    }
}
