    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn saved_tree_has_no_owned_descendant_allocation() {
        // A structural bound, independent of input size or an arbitrary RSS/node threshold.
        // The previous Vec of descendant metadata fails this invariant even when empty.
        assert!(!std::hint::black_box(std::mem::needs_drop::<Tree>()));
    }

    fn entry_digest(path: &OsStr, metadata: &EntryMetadata) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash_entry(&mut hash, path, metadata);
        hash.finalize().into()
    }

    #[test]
    fn entry_digest_binds_framed_name_identity_kind_bytes_and_signed_mtime() {
        let base = EntryMetadata {
            kind: EntryKind::File,
            identity: "owned:one".into(),
            bytes: 4,
            modified: UNIX_EPOCH + Duration::from_nanos(100),
        };
        let before = entry_digest(OsStr::new("cache/item"), &base);
        let changed = [
            EntryMetadata {
                identity: "owned:two".into(),
                ..base.clone()
            },
            EntryMetadata {
                kind: EntryKind::Symlink,
                ..base.clone()
            },
            EntryMetadata {
                bytes: 5,
                ..base.clone()
            },
            EntryMetadata {
                modified: UNIX_EPOCH - Duration::from_nanos(100),
                ..base.clone()
            },
            EntryMetadata {
                modified: UNIX_EPOCH + Duration::from_nanos(200),
                ..base.clone()
            },
        ];
        for metadata in changed {
            assert_ne!(before, entry_digest(OsStr::new("cache/item"), &metadata));
        }
        assert_ne!(before, entry_digest(OsStr::new("cache/other"), &base));
        // Without field framing these path/kind/identity bytes could be concatenated alike.
        let left = EntryMetadata {
            identity: "b\u{2}c".into(),
            ..base.clone()
        };
        let right = EntryMetadata {
            identity: "c".into(),
            ..base
        };
        assert_ne!(
            entry_digest(OsStr::new("a"), &left),
            entry_digest(OsStr::new("a\u{2}b"), &right)
        );
    }

    #[cfg(unix)]
    #[test]
    fn entry_digest_keeps_non_utf8_names_distinct_from_lossy_replacements() {
        use std::os::unix::ffi::OsStringExt;
        let metadata = EntryMetadata {
            kind: EntryKind::File,
            identity: "owned:one".into(),
            bytes: 4,
            modified: UNIX_EPOCH,
        };
        let raw = OsString::from_vec(vec![b'a', 0xff]);
        let replacement = OsStr::new("a\u{fffd}");
        assert_eq!(raw.to_string_lossy(), replacement.to_string_lossy());
        assert_ne!(
            entry_digest(&raw, &metadata),
            entry_digest(replacement, &metadata)
        );
    }

    #[test]
    fn same_byte_descendant_name_or_identity_change_keeps_top_target_facts() {
        for replace_identity in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let cache = root.path().join("cache");
            std::fs::create_dir(&cache).unwrap();
            let item = cache.join("item");
            std::fs::write(&item, b"same").unwrap();
            let initial = DiagnosticDirectory::open(&cache)
                .unwrap()
                .entry_metadata(OsStr::new("item"), false)
                .unwrap();
            let before = Observation::begin(root.path());
            let mut checks = before.checks();
            let index = slot(OldFileCategory::Cache);
            assert_eq!(checks.categories[index].bytes.value, Some(4));
            let name = if replace_identity {
                let replacement = root.path().join("replacement");
                std::fs::write(&replacement, b"same").unwrap();
                std::fs::rename(&item, root.path().join("retired")).unwrap();
                std::fs::rename(replacement, &item).unwrap();
                "item"
            } else {
                std::fs::rename(&item, cache.join("other")).unwrap();
                "other"
            };
            let after = DiagnosticDirectory::open(&cache)
                .unwrap()
                .entry_metadata(OsStr::new(name), false)
                .unwrap();
            assert_eq!(initial.bytes, after.bytes);
            if replace_identity {
                assert_ne!(initial.identity, after.identity);
            } else {
                assert_eq!(initial.identity, after.identity);
            }
            assert!(matches!(
                before.finish(&mut checks),
                Some(CheckState::Changed)
            ));
            let row = &checks.categories[index];
            assert!(matches!(row.present.state, CheckState::Known));
            assert_eq!(row.present.value, Some(true));
            assert!(matches!(row.targets.state, CheckState::Known));
            assert_eq!(row.targets.value, Some(1));
            assert!(matches!(row.bytes.state, CheckState::Changed));
            assert_eq!(row.bytes.value, None);
            assert_eq!(
                checks.categories[slot(OldFileCategory::Logs)].bytes.value,
                Some(0)
            );
        }
    }

    #[test]
    fn same_byte_top_level_identity_change_still_invalidates_targets() {
        let root = tempfile::tempdir().unwrap();
        let item = root.path().join("memory.db");
        std::fs::write(&item, b"same").unwrap();
        let before = Observation::begin(root.path());
        let mut checks = before.checks();
        let replacement = root.path().join("replacement");
        std::fs::write(&replacement, b"same").unwrap();
        std::fs::rename(&item, root.path().join("retired")).unwrap();
        std::fs::rename(replacement, &item).unwrap();
        assert!(matches!(
            before.finish(&mut checks),
            Some(CheckState::Changed)
        ));
        let row = &checks.categories[slot(OldFileCategory::MemoryDatabase)];
        assert!(matches!(row.present.state, CheckState::Changed));
        assert!(matches!(row.targets.state, CheckState::Changed));
        assert!(matches!(row.bytes.state, CheckState::Changed));
        assert_eq!(
            (row.present.value, row.targets.value, row.bytes.value),
            (None, None, None)
        );
    }
