    use super::*;

    #[test]
    fn a_key_is_letters_digits_and_a_few_signs() {
        let long = |n: usize| "k".repeat(n);
        assert!(valid(&long(8)) && valid(&long(512)));
        assert!(valid(&format!("gsk_{}", "Ab9._~+/=:-")));
        for bad in [
            String::new(),
            long(7),
            long(513),
            format!("{}\nbase_url", long(8)),
            format!("{} x", long(8)),
            format!("{}\tx", long(8)),
            format!("{}\"x", long(8)),
            format!("{}\\x", long(8)),
            format!("{}é", long(8)),
            format!("\u{feff}{}", long(8)),
        ] {
            assert!(!valid(&bad), "{bad:?}");
        }
    }

    #[test]
    fn line_2_is_replaced_and_every_other_byte_kept() {
        let with = |old: &str| String::from_utf8(with_key(Some(old.as_bytes()), "KEY")).unwrap();
        assert_eq!(with("Groq\nold\nnotes\n\n"), "Groq\nKEY\nnotes\n\n");
        assert_eq!(with("Groq\r\nold\r\nnotes"), "Groq\r\nKEY\r\nnotes");
        assert_eq!(with("Groq\nold"), "Groq\nKEY\n");
        assert_eq!(with("Groq\n"), "Groq\nKEY\n");
        assert_eq!(with("Groq\r\n"), "Groq\r\nKEY\r\n");
        assert_eq!(with("Groq"), "Groq\nKEY\n");
        assert_eq!(with("\n"), "\nKEY\n");
        assert_eq!(with(""), "API key (oboete)\nKEY\n");
        assert_eq!(
            String::from_utf8(with_key(None, "KEY")).unwrap(),
            "API key (oboete)\nKEY\n"
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn managed_registration_waits_for_owner_only_storage_off_linux() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            managed(
                "key-abcdefgh",
                &root.path().join("corpus"),
                &root.path().join("owner"),
                None
            )
            .unwrap_err(),
            Refused::Unsupported
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn saving_a_key_waits_for_owner_only_files_off_linux() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("GROQ_KEY.md");
        let home = dir.path().join("home");
        assert_eq!(
            write(&path, "key-abcdefgh", &home),
            Err(Refused::Unsupported)
        );
        assert!(!path.exists());
    }

    #[cfg(target_os = "linux")]
    mod on_linux {
        use super::super::*;
        use std::os::unix::fs::PermissionsExt;

        const KEY: &str = "canary-9f3e1c7a";

        /// A key folder and an oboete home beside it, on the test's temporary filesystem.
        fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
            let root = tempfile::tempdir().unwrap();
            let keys = root.path().join("keys");
            let home = root.path().join("home");
            std::fs::create_dir_all(&keys).unwrap();
            std::fs::create_dir_all(&home).unwrap();
            for dir in [root.path(), &keys, &home] {
                private(dir);
            }
            (root, keys, home)
        }

        /// Owner-only whatever the umask: under 0002 a new folder is group-writable, which the
        /// check refuses on the key file's path.
        fn private(dir: &Path) {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }

        fn mode(p: &Path) -> u32 {
            std::fs::metadata(p).unwrap().permissions().mode() & 0o777
        }

        /// Every regular file under `dirs` that holds `text` (a FIFO is not opened).
        fn holding(dirs: &[&Path], text: &str) -> Vec<PathBuf> {
            let mut out = Vec::new();
            for d in dirs {
                for e in std::fs::read_dir(d).unwrap().flatten() {
                    let p = e.path();
                    if e.file_type().is_ok_and(|t| t.is_file())
                        && std::fs::read(&p)
                            .is_ok_and(|b| String::from_utf8_lossy(&b).contains(text))
                    {
                        out.push(p);
                    }
                }
            }
            out
        }

        /// A trusted kernel mount table and the mount IDs reported for opened fixture folders.
        /// Actual files, permissions, owner checks and writes still use the temporary filesystem.
        fn mounts(root: &Path, alias: &Path, source: &Path) {
            use std::os::unix::fs::MetadataExt;
            let dev = std::fs::metadata(root).unwrap().dev();
            let dev = format!("{}:{}", libc::major(dev), libc::minor(dev));
            let escape = |path: &Path| {
                path.to_str()
                    .unwrap()
                    .replace('\\', "\\134")
                    .replace(' ', "\\040")
                    .replace('\t', "\\011")
                    .replace('\n', "\\012")
            };
            let table = format!(
                "1 0 {dev} / / rw - ext4 none rw\n2 1 {dev} {} {} rw shared:10 - ext4 none rw\n7 1 0:999 net:[12345] /unrelated-namespace rw - nsfs nsfs rw\n",
                escape(source),
                escape(alias)
            );
            MOUNTS.with(|f| {
                *f.borrow_mut() = Some(MountFixture {
                    table: table.into_bytes(),
                    ids: vec![(root.to_path_buf(), 1), (alias.to_path_buf(), 2)],
                })
            });
        }

        #[test]
        fn managed_registration_rejects_bound_corpus_descendants_before_mkdir() {
            let (root, _keys, corpus) = setup();
            let source = corpus.join("deep/descendant\t\n\\");
            std::fs::create_dir_all(&source).unwrap();
            let alias = root.path().join("ali as\t\n\\");
            std::fs::create_dir(&alias).unwrap();
            private(&alias);
            mounts(root.path(), &alias, &source);
            let result = managed(KEY, &corpus, root.path(), Some(&alias));
            MOUNTS.with(|f| *f.borrow_mut() = None);
            let registration = result.unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                root.path().join(".oboete-keys")
            );
            assert!(!alias.join("oboete").exists());
            assert!(!source.join("oboete").exists());
        }

        #[test]
        fn managed_registration_refuses_unproven_corpus_submounts_before_mkdir() {
            for kind in [
                "fuse.passthrough",
                "ecryptfs",
                "9p",
                "nfs",
                "unprovenfs",
                "overlay",
            ] {
                let (root, _keys, corpus) = setup();
                let data = root.path().join("data");
                std::fs::create_dir(&data).unwrap();
                private(&data);
                let layer = corpus.join("layer");
                std::fs::create_dir(&layer).unwrap();
                private(&layer);
                mounts(root.path(), &data, &root.path().join("separate"));
                MOUNTS.with(|f| {
                    f.borrow_mut().as_mut().unwrap().table.extend_from_slice(
                        format!(
                            "8 1 0:997 / {} rw - {kind} opaque-source rw\n",
                            layer.display()
                        )
                        .as_bytes(),
                    )
                });
                let result = managed(KEY, &corpus, root.path(), Some(&data));
                MOUNTS.with(|f| *f.borrow_mut() = None);
                assert_eq!(result.unwrap_err(), Refused::Protected, "{kind}");
                assert!(!data.join("oboete").exists(), "{kind}");
                assert!(!root.path().join(".oboete-keys").exists(), "{kind}");
            }
        }

        #[test]
        fn managed_registration_keeps_unrelated_opaque_and_disjoint_native_mounts_usable() {
            for (kind, protected) in [
                ("fuse.passthrough", false),
                ("unprovenfs", false),
                ("tmpfs", true),
            ] {
                let (root, _keys, corpus) = setup();
                let data = root.path().join("data");
                std::fs::create_dir(&data).unwrap();
                private(&data);
                let at = if protected {
                    corpus.join("layer")
                } else {
                    root.path().join("unrelated")
                };
                std::fs::create_dir(&at).unwrap();
                private(&at);
                mounts(root.path(), &data, &root.path().join("separate"));
                MOUNTS.with(|f| {
                    f.borrow_mut().as_mut().unwrap().table.extend_from_slice(
                        format!(
                            "8 1 0:997 / {} rw - {kind} opaque-source rw\n",
                            at.display()
                        )
                        .as_bytes(),
                    )
                });
                let result = managed(KEY, &corpus, root.path(), Some(&data));
                MOUNTS.with(|f| *f.borrow_mut() = None);
                let registration = result.unwrap();
                assert_eq!(
                    registration.path().parent().unwrap(),
                    data.join("oboete/keys"),
                    "{kind}"
                );
                assert_eq!(mode(registration.path()), 0o600);
            }
        }

        #[test]
        fn managed_registration_uses_mount_identity_when_subvolume_stat_devices_differ() {
            use std::os::unix::fs::MetadataExt;
            let (root, _keys, corpus) = setup();
            let alias = root.path().join("alias");
            std::fs::create_dir(&alias).unwrap();
            private(&alias);
            let stat_dev = std::fs::metadata(root.path()).unwrap().dev();
            let observed = format!(" {}:{} ", libc::major(stat_dev), libc::minor(stat_dev));
            // Btrfs reports the subvolume's anon_dev through stat and the superblock's
            // different s_dev in mountinfo. Only the mount observations are injected.
            let mounted = format!(
                " {}:{} ",
                libc::major(stat_dev),
                u64::from(libc::minor(stat_dev)) + 1
            );
            let describe_subvolume = || {
                MOUNTS.with(|f| {
                    let mut f = f.borrow_mut();
                    let fixture = f.as_mut().unwrap();
                    fixture.table = String::from_utf8(fixture.table.clone())
                        .unwrap()
                        .replace(&observed, &mounted)
                        .replace("- ext4 ", "- btrfs ")
                        .into_bytes();
                })
            };
            mounts(root.path(), &alias, &root.path().join("separate"));
            describe_subvolume();
            let result = managed(KEY, &corpus, root.path(), Some(&alias));
            MOUNTS.with(|f| *f.borrow_mut() = None);
            let registration = result.unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                alias.join("oboete/keys")
            );
            assert_eq!(mode(registration.path()), 0o600);
            drop(registration);

            // Different stat devices must not hide a bind into the same protected filesystem.
            mounts(root.path(), &alias, &corpus.join("descendant"));
            describe_subvolume();
            let result = managed(KEY, &corpus, root.path(), Some(&alias));
            MOUNTS.with(|f| *f.borrow_mut() = None);
            let registration = result.unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                root.path().join(".oboete-keys")
            );
            assert_eq!(
                std::fs::read_dir(alias.join("oboete/keys"))
                    .unwrap()
                    .count(),
                0
            );
        }

        #[test]
        fn managed_registration_protects_a_missing_corpus_without_creating_it() {
            let (root, _keys, _corpus) = setup();
            let corpus = root.path().join("not-yet-created/corpus");
            let registration = managed(KEY, &corpus, root.path(), None).unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                root.path().join(".local/share/oboete/keys")
            );
            assert!(!root.path().join("not-yet-created").exists());
        }

        #[test]
        fn managed_registration_requires_complete_bounded_mount_observations() {
            let (root, _keys, corpus) = setup();
            let alias = root.path().join("alias");
            std::fs::create_dir(&alias).unwrap();
            private(&alias);
            mounts(root.path(), &alias, &root.path().join("separate"));
            let good = MOUNTS.with(|f| f.borrow().as_ref().unwrap().table.clone());
            let empty_type = String::from_utf8(good.clone())
                .unwrap()
                .replace("- ext4 ", "-  ")
                .into_bytes();
            let bad_escape = String::from_utf8(good.clone())
                .unwrap()
                .replace(" / / ", " /bad\\999 / ")
                .into_bytes();
            let mut duplicate = good.clone();
            duplicate.extend_from_slice(&good);
            for bad in [
                empty_type,
                Vec::new(),
                b"not a mount table".to_vec(),
                bad_escape,
                duplicate,
                vec![b'x'; 1_048_577],
            ] {
                MOUNTS.with(|f| f.borrow_mut().as_mut().unwrap().table = bad);
                let result = managed(KEY, &corpus, root.path(), Some(&alias));
                assert_eq!(result.unwrap_err(), Refused::Protected);
                assert!(!alias.join("oboete").exists());
                assert!(!root.path().join(".oboete-keys").exists());
            }
            MOUNTS.with(|f| {
                let mut f = f.borrow_mut();
                let fixture = f.as_mut().unwrap();
                fixture.table = good;
                fixture.ids = vec![(root.path().to_path_buf(), 999)];
            });
            let result = managed(KEY, &corpus, root.path(), Some(&alias));
            MOUNTS.with(|f| *f.borrow_mut() = None);
            assert_eq!(result.unwrap_err(), Refused::Protected);
            assert!(!alias.join("oboete").exists());
            assert!(!root.path().join(".oboete-keys").exists());
        }

        #[test]
        fn managed_registration_creates_private_directories_and_a_fresh_key() {
            let (root, _keys, corpus) = setup();
            let owner = root.path().join("owner");
            std::fs::create_dir(&owner).unwrap();
            private(&owner);
            let registration = managed(KEY, &corpus, &owner, None).unwrap();
            let path = registration.path().to_path_buf();
            assert_eq!(
                path.parent().unwrap(),
                owner.join(".local/share/oboete/keys")
            );
            for name in [
                ".local",
                ".local/share",
                ".local/share/oboete",
                ".local/share/oboete/keys",
            ] {
                assert_eq!(mode(&owner.join(name)), 0o700);
            }
            assert_eq!(mode(&path), 0o600);
            let name = path.file_name().unwrap().to_str().unwrap();
            assert!(name.ends_with("_KEY.md"));
            assert_eq!(name.len(), 39);
            assert_eq!(crate::config::read_key(&path).unwrap(), KEY);
            assert!(registration.durable);
            registration.retain();
            assert_eq!(crate::config::read_key(&path).unwrap(), KEY);
        }

        #[test]
        fn managed_registration_falls_back_outside_the_corpus_before_creating_directories() {
            let (root, _keys, corpus) = setup();
            let owner = root.path().join("owner");
            std::fs::create_dir(&owner).unwrap();
            private(&owner);
            let ordinary = corpus.join("local-data");
            let registration = managed(KEY, &corpus, &owner, Some(&ordinary)).unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                owner.join(".oboete-keys")
            );
            assert!(!ordinary.exists());
            assert!(!owner.join(".oboete").exists());
            assert_eq!(mode(registration.path().parent().unwrap()), 0o700);
            assert_eq!(crate::config::read_key(registration.path()).unwrap(), KEY);
        }

        #[test]
        fn managed_registration_removes_only_its_own_uncommitted_file() {
            let (root, _keys, corpus) = setup();
            let registration = managed(KEY, &corpus, root.path(), None).unwrap();
            let path = registration.path().to_path_buf();
            let legacy = path.parent().unwrap().join("LEGACY_KEY.md");
            std::fs::write(&legacy, "legacy unchanged").unwrap();
            drop(registration);
            assert!(!path.exists());
            assert_eq!(
                std::fs::read_to_string(&legacy).unwrap(),
                "legacy unchanged"
            );

            let registration = managed(KEY, &corpus, root.path(), None).unwrap();
            let path = registration.path().to_path_buf();
            let moved = path.with_extension("saved");
            std::fs::rename(&path, &moved).unwrap();
            std::fs::write(&path, "concurrent replacement").unwrap();
            drop(registration);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "concurrent replacement"
            );
            assert_eq!(crate::config::read_key(&moved).unwrap(), KEY);
            assert_eq!(
                std::fs::read_to_string(&legacy).unwrap(),
                "legacy unchanged"
            );
        }

        #[test]
        fn managed_registration_write_and_sync_failures_leave_no_unused_key() {
            for at in [Step::Stage, Step::Write, Step::Sync] {
                let (root, _keys, corpus) = setup();
                FAIL.with(|f| f.set(Some(at)));
                let result = managed(KEY, &corpus, root.path(), None);
                FAIL.with(|f| f.set(None));
                assert_eq!(result.unwrap_err(), Refused::Failed, "{at:?}");
                assert_eq!(
                    std::fs::read_dir(root.path().join(".local/share/oboete/keys"))
                        .unwrap()
                        .count(),
                    0,
                    "{at:?}"
                );
            }
        }

        #[test]
        fn managed_registration_does_not_overwrite_or_delete_a_filename_collision() {
            let (root, _keys, corpus) = setup();
            let registration = managed(KEY, &corpus, root.path(), None).unwrap();
            let dir = registration.path().parent().unwrap().to_path_buf();
            drop(registration);
            let collision = dir.join("abababababababababababababababab_KEY.md");
            std::fs::write(&collision, "legacy bytes unchanged").unwrap();
            std::fs::set_permissions(&collision, std::fs::Permissions::from_mode(0o640)).unwrap();
            MANAGED_TAG.with(|f| f.set(Some([0xab; 16])));
            assert_eq!(
                managed(KEY, &corpus, root.path(), None).unwrap_err(),
                Refused::Failed
            );
            assert_eq!(
                std::fs::read_to_string(&collision).unwrap(),
                "legacy bytes unchanged"
            );
            assert_eq!(mode(&collision), 0o640);
            assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1);
        }

        #[test]
        fn managed_registration_refuses_a_bad_file_filesystem_before_writing_the_key() {
            for magic in [0x0102_1997, 0x6573_5546] {
                let (root, _keys, corpus) = setup();
                let dir = root.path().join(".local/share/oboete/keys");
                let observed = dir.clone();
                BEFORE_WRITE.with(|b| {
                    *b.borrow_mut() = Some(Box::new(move || {
                        let files: Vec<_> =
                            std::fs::read_dir(&observed).unwrap().flatten().collect();
                        assert_eq!(files.len(), 1);
                        assert_eq!(mode(&observed), 0o700);
                        assert_eq!(mode(&files[0].path()), 0o600);
                        assert!(std::fs::read(files[0].path()).unwrap().is_empty());
                        fake_fs(Some(magic));
                    }))
                });
                let result = managed(KEY, &corpus, root.path(), None);
                fake_fs(None);
                assert_eq!(result.unwrap_err(), Refused::NotPrivate);
                assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
            }
        }

        #[test]
        fn managed_registration_uses_safe_fallback_without_changing_shared_ancestors() {
            let (root, _keys, corpus) = setup();
            let shared = root.path().join("shared");
            std::fs::create_dir(&shared).unwrap();
            std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
            let alias = root.path().join("data-alias");
            std::os::unix::fs::symlink(&shared, &alias).unwrap();
            let registration = managed(KEY, &corpus, root.path(), Some(&alias)).unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                root.path().join(".oboete-keys")
            );
            assert_eq!(mode(&shared), 0o777);
            assert_eq!(std::fs::read_dir(&shared).unwrap().count(), 0);
            drop(registration);

            std::fs::remove_dir(root.path().join(".oboete-keys")).unwrap();
            let owner = shared.join("owner");
            std::fs::create_dir(&owner).unwrap();
            private(&owner);
            assert_eq!(
                managed(KEY, &corpus, &owner, Some(&alias)).unwrap_err(),
                Refused::SharedDir
            );
            assert!(!owner.join(".oboete-keys").exists());
        }

        #[test]
        fn managed_registration_keeps_existing_folder_modes_and_refuses_both_unsafe_locations() {
            let (root, _keys, corpus) = setup();
            let data = root.path().join("data");
            let ordinary = data.join("oboete/keys");
            std::fs::create_dir_all(&ordinary).unwrap();
            for dir in [&data, &data.join("oboete")] {
                private(dir);
            }
            std::fs::set_permissions(&ordinary, std::fs::Permissions::from_mode(0o755)).unwrap();
            let fallback = root.path().join(".oboete-keys");
            std::fs::create_dir(&fallback).unwrap();
            std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(
                managed(KEY, &corpus, root.path(), Some(&data)).unwrap_err(),
                Refused::NotPrivate
            );
            assert_eq!(mode(&ordinary), 0o755);
            assert_eq!(mode(&fallback), 0o755);
            assert_eq!(std::fs::read_dir(&ordinary).unwrap().count(), 0);
            assert_eq!(std::fs::read_dir(&fallback).unwrap().count(), 0);
        }

        #[test]
        fn managed_registration_refuses_unknown_ancestor_filesystems_before_creation() {
            let (root, _keys, corpus) = setup();
            let data = root.path().join("data");
            std::fs::create_dir(&data).unwrap();
            private(&data);
            FS_AT.with(|f| *f.borrow_mut() = Some((data.clone(), 0x6573_5546)));
            let result = managed(KEY, &corpus, root.path(), Some(&data));
            FS_AT.with(|f| *f.borrow_mut() = None);
            let registration = result.unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                root.path().join(".oboete-keys")
            );
            assert_eq!(std::fs::read_dir(&data).unwrap().count(), 0);
            drop(registration);

            FS_AT.with(|f| *f.borrow_mut() = Some((data.clone(), 0x6573_5546)));
            let result = managed(KEY, &corpus, &data, Some(&data));
            FS_AT.with(|f| *f.borrow_mut() = None);
            assert_eq!(result.unwrap_err(), Refused::NotPrivate);
            assert_eq!(std::fs::read_dir(&data).unwrap().count(), 0);
        }

        #[test]
        fn managed_registration_refuses_corpus_aliases_and_never_creates_an_owner_corpus() {
            let (root, _keys, corpus) = setup();
            let alias = root.path().join("corpus-alias");
            std::os::unix::fs::symlink(&corpus, &alias).unwrap();
            let registration = managed(KEY, &corpus, root.path(), Some(&alias)).unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                root.path().join(".oboete-keys")
            );
            assert_eq!(std::fs::read_dir(&corpus).unwrap().count(), 0);
            assert_eq!(
                managed(KEY, &corpus, &corpus, None).unwrap_err(),
                Refused::Protected
            );
            assert_eq!(std::fs::read_dir(&corpus).unwrap().count(), 0);
            assert!(!root.path().join(".oboete").exists());
        }

        #[test]
        fn managed_registration_reports_durability_without_losing_a_retained_key() {
            let (root, _keys, corpus) = setup();
            FAIL.with(|f| f.set(Some(Step::DirSync)));
            let result = managed(KEY, &corpus, root.path(), None);
            FAIL.with(|f| f.set(None));
            let registration = result.unwrap();
            let path = registration.path().to_path_buf();
            assert!(!registration.durable);
            registration.retain();
            assert_eq!(crate::config::read_key(&path).unwrap(), KEY);
        }

        #[test]
        fn managed_registration_bad_key_errors_never_include_the_input() {
            let (root, _keys, corpus) = setup();
            let input = "secret-canary\nrefused";
            let refused = managed(input, &corpus, root.path(), None).unwrap_err();
            assert_eq!(refused, Refused::BadKey);
            assert!(!format!("{refused:?} {}", refused.code()).contains(input));
            assert!(!root.path().join(".local").exists());
            assert!(!root.path().join(".oboete-keys").exists());
        }

        #[test]
        fn a_key_goes_on_line_2_of_its_file_owner_only() {
            let (_root, keys, home) = setup();
            let path = keys.join("GROQ_KEY.md");
            std::fs::write(&path, "Groq\r\nold-key-123\r\nrotate yearly\r\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(write(&path, KEY, &home), Ok(Written { durable: true }));
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                format!("Groq\r\n{KEY}\r\nrotate yearly\r\n")
            );
            assert_eq!(mode(&path), 0o600);
            assert_eq!(crate::config::read_key(&path).unwrap(), KEY);
            assert_eq!(holding(&[&keys, &home], KEY), [path]);
        }

        #[test]
        fn a_new_key_file_gets_a_title() {
            let (_root, keys, home) = setup();
            let path = keys.join("NEW_KEY.md");
            assert_eq!(write(&path, KEY, &home), Ok(Written { durable: true }));
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                format!("API key (oboete)\n{KEY}\n")
            );
            assert_eq!(mode(&path), 0o600);
            let missing = keys.join("no-such-dir").join("X_KEY.md");
            assert_eq!(write(&missing, KEY, &home), Err(Refused::NoDir));
            assert!(!keys.join("no-such-dir").exists());
        }

        #[test]
        fn an_ancestor_swap_is_refused_before_any_key_is_written() {
            let (root, _keys, home) = setup();
            let shared = root.path().join("shared");
            let keys = shared.join("keys");
            let moved = root.path().join("moved");
            std::fs::create_dir_all(&keys).unwrap();
            private(&keys);
            std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
            let path = keys.join("GROQ_KEY.md");
            let before = "Groq\nold-key-123\n";
            std::fs::write(&path, before).unwrap();
            let (swap, into, target) = (keys.clone(), moved.clone(), home.clone());
            BEFORE_WRITE.with(|b| {
                *b.borrow_mut() = Some(Box::new(move || {
                    std::fs::rename(&swap, &into).unwrap();
                    std::os::unix::fs::symlink(&target, &swap).unwrap();
                }))
            });
            // Without the ancestry check this swaps in the home after destination() has
            // approved the old key folder, and the save writes the key into that home.
            let result = write(&path, KEY, &home);
            let swap = BEFORE_WRITE.with(|b| b.borrow_mut().take());
            assert_eq!(result, Err(Refused::SharedDir));
            assert!(swap.is_some(), "must refuse before the read/stage/write");
            swap.unwrap()();
            assert_eq!(
                std::fs::read_to_string(moved.join("GROQ_KEY.md")).unwrap(),
                before
            );
            assert!(!path.exists());
            assert!(holding(&[&moved, &home], KEY).is_empty());
            assert_eq!(std::fs::read_dir(&moved).unwrap().count(), 1);
            assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
        }

        #[test]
        fn original_and_resolved_ancestors_must_both_be_protected() {
            let (root, keys, home) = setup();
            let shared = root.path().join("shared");
            let nested = shared.join("nested");
            let alias = root.path().join("alias");
            std::fs::create_dir_all(&nested).unwrap();
            private(&nested);
            std::os::unix::fs::symlink(&nested, &alias).unwrap();
            for mode in [0o775, 0o757, 0o777] {
                std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(mode)).unwrap();
                for path in [
                    nested.join("GROQ_KEY.md"),
                    shared.join("..").join("keys").join("GROQ_KEY.md"),
                    alias.join("GROQ_KEY.md"),
                ] {
                    assert_eq!(write(&path, KEY, &home), Err(Refused::SharedDir));
                }
            }
            assert!(holding(&[&keys, &nested, &home], KEY).is_empty());
        }

        /// A link whose target is reached through another link: the folder that holds the second
        /// link is on the path too, and a user who can write there can point it anywhere.
        #[test]
        fn a_folder_a_second_link_passes_through_is_checked() {
            let (root, keys, home) = setup();
            let shared = root.path().join("shared");
            std::fs::create_dir_all(&shared).unwrap();
            let hop = shared.join("hop");
            std::os::unix::fs::symlink(&keys, &hop).unwrap();
            let alias = root.path().join("alias");
            std::os::unix::fs::symlink(&hop, &alias).unwrap();
            let path = alias.join("GROQ_KEY.md");
            // Both links in folders only the owner writes: the chain is as safe as its target.
            private(&shared);
            assert_eq!(write(&path, KEY, &home), Ok(Written { durable: true }));
            // The second link's folder open to others: `hop` can be replaced under the first link.
            for mode in [0o775, 0o757, 0o777] {
                std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(mode)).unwrap();
                assert_eq!(
                    write(&path, "canary-second-hop", &home),
                    Err(Refused::SharedDir),
                    "{mode:o}"
                );
            }
            assert!(holding(&[&keys, &home], "canary-second-hop").is_empty());
            // A chain that never ends is no folder.
            let (a, b) = (root.path().join("a"), root.path().join("b"));
            std::os::unix::fs::symlink(&b, &a).unwrap();
            std::os::unix::fs::symlink(&a, &b).unwrap();
            assert_eq!(
                write(&a.join("GROQ_KEY.md"), KEY, &home),
                Err(Refused::NoDir)
            );
        }

        /// A link target that ends in `/` or `/.` is still a link: the kernel follows it when it
        /// is asked about `hop/`, so the folders behind it must be walked, not skipped (Codex's
        /// review of #355).
        #[test]
        fn a_link_target_with_a_trailing_slash_does_not_hide_its_hops() {
            for suffix in ["/", "/."] {
                let (root, keys, home) = setup();
                let shared = root.path().join("shared");
                std::fs::create_dir_all(&shared).unwrap();
                std::os::unix::fs::symlink(&keys, shared.join("bridge")).unwrap();
                std::os::unix::fs::symlink(shared.join("bridge"), root.path().join("hop")).unwrap();
                let alias = root.path().join("alias");
                std::os::unix::fs::symlink(format!("hop{suffix}"), &alias).unwrap();
                let path = alias.join("GROQ_KEY.md");
                private(&shared);
                assert_eq!(
                    write(&path, KEY, &home),
                    Ok(Written { durable: true }),
                    "{suffix}"
                );
                std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
                assert_eq!(
                    write(&path, "canary-trailing-slash", &home),
                    Err(Refused::SharedDir),
                    "{suffix}"
                );
                assert!(holding(&[&keys, &home], "canary-trailing-slash").is_empty());
            }
        }

        /// Each link is followed once, as the kernel counts them: a path through several links is
        /// not refused for its length, and the kernel's limit of 40 is the limit.
        #[test]
        fn links_are_counted_once_each_up_to_the_kernels_forty() {
            let (root, keys, home) = setup();
            // l1 -> d1, d1/l2 -> d2, ... d5/l6 -> the key folder: six links in one path, each
            // target relative to the folder its link is in.
            let mut at = root.path().to_path_buf();
            let mut path = root.path().to_path_buf();
            for n in 1..=6 {
                let target = if n == 6 {
                    "../../../../../keys".to_string()
                } else {
                    format!("d{n}")
                };
                std::os::unix::fs::symlink(target, at.join(format!("l{n}"))).unwrap();
                path.push(format!("l{n}"));
                if n < 6 {
                    at.push(format!("d{n}"));
                    std::fs::create_dir(&at).unwrap();
                    private(&at);
                }
            }
            assert_eq!(
                write(&path.join("GROQ_KEY.md"), KEY, &home),
                Ok(Written { durable: true })
            );
            // A chain: c40 -> c39 -> ... -> c1 -> the key folder is 40 links; one more is refused.
            let mut target = keys.clone();
            for n in 1..=41 {
                let link = root.path().join(format!("c{n}"));
                std::os::unix::fs::symlink(&target, &link).unwrap();
                target = link;
            }
            assert_eq!(
                write(&root.path().join("c40").join("GROQ_KEY.md"), KEY, &home),
                Ok(Written { durable: true })
            );
            assert_eq!(
                write(&root.path().join("c41").join("GROQ_KEY.md"), KEY, &home),
                Err(Refused::NoDir)
            );
            // Asked of the walk itself: in `write` the kernel refuses a 41st link after it, which
            // would hide a walk that no longer counts (and then never ends on a loop of links).
            assert_eq!(
                crate::keyfile::linux::check_dirs(&root.path().join("c41")),
                Err(Refused::NoDir)
            );
        }

        #[test]
        fn trusted_sticky_ancestors_and_private_aliases_keep_working() {
            let (root, keys, home) = setup();
            let alias = root.path().join("alias");
            std::os::unix::fs::symlink(&keys, &alias).unwrap();
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o1777)).unwrap();
            let path = alias.join("GROQ_KEY.md");
            assert_eq!(write(&path, KEY, &home), Ok(Written { durable: true }));
            assert_eq!(mode(&path), 0o600);
            // The key folder cannot itself be shared: a new key's name has no owner yet.
            std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o1777)).unwrap();
            let new = keys.join("NEW_KEY.md");
            assert_eq!(write(&new, KEY, &home), Err(Refused::SharedDir));
            assert!(!new.exists());
        }

        #[test]
        fn a_key_goes_only_into_a_dedicated_key_file() {
            let (_root, keys, home) = setup();
            let config = home.join("config.toml");
            std::fs::write(&config, "[summary]\nlanguage = \"ja\"\n").unwrap();
            for (path, why) in [
                (PathBuf::from("GROQ_KEY.md"), Refused::NotAbsolute),
                (keys.join("groq.txt"), Refused::NotAKeyFile),
                (config.clone(), Refused::NotAKeyFile),
                (home.join("GROQ_KEY.md"), Refused::Protected),
                (
                    keys.join("..").join("home").join("GROQ_KEY.md"),
                    Refused::Protected,
                ),
            ] {
                assert_eq!(write(&path, KEY, &home), Err(why), "{}", path.display());
            }
            assert_eq!(
                std::fs::read_to_string(&config).unwrap(),
                "[summary]\nlanguage = \"ja\"\n"
            );
            assert!(holding(&[&keys, &home], KEY).is_empty());
        }

        #[test]
        fn a_key_file_that_is_no_plain_small_utf8_file_is_left_alone() {
            let (_root, keys, home) = setup();
            let target = keys.join("target.txt");
            std::fs::write(&target, "untouched").unwrap();
            let link = keys.join("LINK_KEY.md");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert_eq!(write(&link, KEY, &home), Err(Refused::NotAFile));
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
            let folder = keys.join("DIR_KEY.md");
            std::fs::create_dir(&folder).unwrap();
            assert_eq!(write(&folder, KEY, &home), Err(Refused::NotAFile));
            // A FIFO is refused at once, not waited on.
            let fifo = keys.join("FIFO_KEY.md");
            let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            assert_eq!(write(&fifo, KEY, &home), Err(Refused::NotAFile));
            let big = keys.join("BIG_KEY.md");
            std::fs::write(&big, vec![b'a'; 64 * 1024 + 1]).unwrap();
            assert_eq!(write(&big, KEY, &home), Err(Refused::TooBig));
            let latin = keys.join("LATIN_KEY.md");
            std::fs::write(&latin, b"Groq \xe9\nold\n").unwrap();
            assert_eq!(write(&latin, KEY, &home), Err(Refused::NotUtf8));
            assert_eq!(std::fs::read(&latin).unwrap(), b"Groq \xe9\nold\n");
            assert!(holding(&[&keys, &home], KEY).is_empty());
        }

        #[test]
        fn a_failure_before_the_rename_leaves_the_file_and_no_stage() {
            let (_root, keys, home) = setup();
            let path = keys.join("GROQ_KEY.md");
            let before = "Groq\nold-key-123\n";
            for at in [Step::Stage, Step::Write, Step::Sync, Step::Rename] {
                std::fs::write(&path, before).unwrap();
                FAIL.with(|f| f.set(Some(at)));
                let result = write(&path, KEY, &home);
                FAIL.with(|f| f.set(None));
                assert_eq!(result, Err(Refused::Failed), "{at:?}");
                assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "{at:?}");
                let names: Vec<_> = std::fs::read_dir(&keys)
                    .unwrap()
                    .flatten()
                    .map(|e| e.file_name())
                    .collect();
                assert_eq!(names, ["GROQ_KEY.md"], "{at:?}");
            }
            assert!(holding(&[&keys, &home], KEY).is_empty());
        }

        /// A folder's owner and mode are believed only on a filesystem known to enforce them: a
        /// FUSE mount another user made can show its folders as this user's, 0700, and then turn
        /// one into a link (Codex's second review of #355). Such a folder on the way, or as the
        /// key folder, is refused before any read or stage.
        #[test]
        fn a_folder_on_the_way_must_be_on_a_filesystem_known_to_keep_a_mode() {
            let (root, _keys, home) = setup();
            let mount = root.path().join("m");
            let keys = mount.join("keys");
            std::fs::create_dir_all(&keys).unwrap();
            private(&mount);
            private(&keys);
            let path = keys.join("GROQ_KEY.md");
            assert_eq!(write(&path, KEY, &home), Ok(Written { durable: true }));
            for at in [&mount, &keys] {
                FS_AT.with(|f| *f.borrow_mut() = Some((at.clone(), 0x6573_5546)));
                let result = write(&path, "canary-fuse-on-the-way", &home);
                FS_AT.with(|f| *f.borrow_mut() = None);
                assert_eq!(result, Err(Refused::NotPrivate), "{}", at.display());
            }
            assert!(holding(&[&keys, &home], "canary-fuse-on-the-way").is_empty());
            let names: Vec<_> = std::fs::read_dir(&keys)
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect();
            assert_eq!(names, ["GROQ_KEY.md"]);
            // The kernel's own answer, with no seam: /proc is on no filesystem of the list.
            assert_eq!(
                write(Path::new("/proc/GROQ_KEY.md"), KEY, &home),
                Err(Refused::NotPrivate)
            );
        }

        /// The staged file's own filesystem decides, before the key is written: 9p (a Windows
        /// drive under WSL) and FUSE are refused, and the file and folder stay as they were.
        #[test]
        fn a_filesystem_not_known_to_keep_a_mode_gets_no_key() {
            let (_root, keys, home) = setup();
            let path = keys.join("GROQ_KEY.md");
            let before = "Groq\nold-key-123\n";
            std::fs::write(&path, before).unwrap();
            for magic in [0x0102_1997, 0x6573_5546] {
                FS.with(|f| f.set(Some(magic)));
                let result = write(&path, KEY, &home);
                FS.with(|f| f.set(None));
                assert_eq!(result, Err(Refused::NotPrivate), "{magic:x}");
                assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
                let names: Vec<_> = std::fs::read_dir(&keys)
                    .unwrap()
                    .flatten()
                    .map(|e| e.file_name())
                    .collect();
                assert_eq!(names, ["GROQ_KEY.md"], "{magic:x}");
            }
            assert!(holding(&[&keys, &home], KEY).is_empty());
        }

        #[test]
        fn a_failed_folder_sync_after_the_rename_keeps_the_key() {
            let (_root, keys, home) = setup();
            let path = keys.join("GROQ_KEY.md");
            std::fs::write(&path, "Groq\nold-key-123\n").unwrap();
            FAIL.with(|f| f.set(Some(Step::DirSync)));
            let result = write(&path, KEY, &home);
            FAIL.with(|f| f.set(None));
            assert_eq!(result, Ok(Written { durable: false }));
            assert_eq!(crate::config::read_key(&path).unwrap(), KEY);
        }

        #[test]
        fn a_file_changed_since_it_was_read_is_not_overwritten() {
            let (_root, keys, home) = setup();
            let path = keys.join("GROQ_KEY.md");
            std::fs::write(&path, "Groq\nold-key-123\n").unwrap();
            let edit = path.clone();
            BEFORE_CHECK.with(|b| {
                *b.borrow_mut() = Some(Box::new(move || {
                    std::fs::write(&edit, "Groq\nold-key-123\nnote added meanwhile\n").unwrap();
                }))
            });
            assert_eq!(write(&path, KEY, &home), Err(Refused::Changed));
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "Groq\nold-key-123\nnote added meanwhile\n"
            );
            // A file made meanwhile where there was none counts as a change too.
            let new = keys.join("NEW_KEY.md");
            let made = new.clone();
            BEFORE_CHECK.with(|b| {
                *b.borrow_mut() = Some(Box::new(move || std::fs::write(&made, "mine\n").unwrap()))
            });
            assert_eq!(write(&new, KEY, &home), Err(Refused::Changed));
            assert_eq!(std::fs::read_to_string(&new).unwrap(), "mine\n");
            assert!(holding(&[&keys, &home], KEY).is_empty());
        }
    }
