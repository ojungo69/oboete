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
