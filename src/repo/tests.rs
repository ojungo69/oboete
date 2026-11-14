    use super::*;

    #[test]
    fn one_key_for_every_way_to_clone_and_no_secrets() {
        for url in [
            "https://github.com/ojungo69/oboete.git",
            "https://github.com/ojungo69/oboete",
            "git@github.com:ojungo69/oboete.git",
            "ssh://git@github.com/ojungo69/oboete.git",
            "ssh://git@GitHub.com:22/ojungo69/oboete/",
            "https://user:ghp_secret@github.com:443/ojungo69/oboete.git?ref=x#frag",
            "git+ssh://git@github.com/ojungo69/oboete",
        ] {
            assert_eq!(
                normalize(url).as_deref(),
                Some("github.com/ojungo69/oboete"),
                "{url}"
            );
        }
        // A port other than the scheme's default names another service on the same host.
        assert_eq!(
            normalize("https://git.example.com:8443/team/app.git").as_deref(),
            Some("git.example.com:8443/team/app")
        );
        assert_eq!(
            normalize("ssh://git@git.example.com:2222/team/app.git").as_deref(),
            Some("git.example.com:2222/team/app")
        );
        // A bracketed IPv6 host, scp-style or ssh://, with and without the default port.
        for url in [
            "git@[2001:db8::1]:team/app.git",
            "ssh://git@[2001:db8::1]/team/app.git",
            "ssh://git@[2001:db8::1]:22/team/app",
        ] {
            assert_eq!(
                normalize(url).as_deref(),
                Some("[2001:db8::1]/team/app"),
                "{url}"
            );
        }
        for local in [
            "/srv/git/app.git",
            "../app",
            "C:\\repos\\app",
            "C:/repos/app",
            "file:///srv/git/app.git",
            "https://github.com/",
            "",
        ] {
            assert_eq!(normalize(local), None, "{local}");
        }
    }

    #[test]
    fn config_origin_is_read_without_git() {
        let config = "[core]\n\turl = nope\n[remote \"upstream\"]\n\turl = https://x.org/u/up.git\n\
                      [remote \"origin\"]\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
                      \tURL = \"git@github.com:o/r.git\" # comment\n";
        assert_eq!(
            origin_url(config).as_deref(),
            Some("git@github.com:o/r.git")
        );
        assert_eq!(origin_url("[remote \"upstream\"]\nurl = x\n"), None);
        for header in [
            "[REMOTE \"origin\"]",
            "[remote.origin]",
            "[remote \"origin\"] # comment",
        ] {
            assert_eq!(
                origin_url(&format!("{header}\n\turl = https://x.org/o/r\n")).as_deref(),
                Some("https://x.org/o/r"),
                "{header}"
            );
        }
        assert_eq!(origin_url("[remote \"Origin\"]\nurl = x\n"), None);
        for value in [
            "https://x.org/o/r;comment",
            "https://x.org/o/r\t# comment",
            "\"https://x.org/o/r\" ; c",
            "  https://x.org/o/r  ",
        ] {
            assert_eq!(config_value(value), "https://x.org/o/r", "{value:?}");
        }
        assert_eq!(config_value("\"a#b\" # c"), "a#b");
        // A trailing backslash continues the value on the next line; the value ends at the line.
        assert_eq!(
            origin_url(
                "[remote \"origin\"]\r\n\turl = https://github.com/owner/\\\r\nrepo.git\r\n"
            )
            .as_deref(),
            Some("https://github.com/owner/repo.git")
        );
        assert_eq!(
            origin_url("[remote \"origin\"]\n\turl = https://x.org/o/r\n\tfetch = y\n").as_deref(),
            Some("https://x.org/o/r")
        );
    }

    #[test]
    fn nearest_git_dir_wins_and_worktree_maps_to_main() {
        let tmp = std::env::temp_dir().join(format!("oboete-repo-{}", std::process::id()));
        let main = tmp.join("main");
        std::fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        std::fs::create_dir_all(main.join("src/deep")).unwrap();
        // No origin: the path.
        assert_eq!(
            key(&main.join("src/deep")),
            main.canonicalize().unwrap().to_string_lossy()
        );

        let wt = tmp.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        std::fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        assert_eq!(key(&wt), main.canonicalize().unwrap().to_string_lossy());

        // With an origin, the main repository, its worktree and a clone elsewhere agree.
        std::fs::write(
            main.join(".git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:o/r.git\n",
        )
        .unwrap();
        let clone = tmp.join("elsewhere/renamed");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        std::fs::write(
            clone.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://github.com/o/r\n",
        )
        .unwrap();
        for dir in [main.join("src/deep"), wt.clone(), clone] {
            assert_eq!(key(&dir), "github.com/o/r", "{}", dir.display());
        }

        // An origin that `.git/config` pulls in with `[include]`, relative to the git directory.
        let inc = tmp.join("inc");
        std::fs::create_dir_all(inc.join(".git")).unwrap();
        std::fs::write(
            inc.join(".git/config"),
            "[core]\n\tbare = false # c \\\n[include]\n\tpath = remo\\\ntes.inc # c\n\tpath = loop\n",
        )
        .unwrap();
        std::fs::write(
            inc.join(".git/remotes.inc"),
            "[remote \"origin\"]\n\turl = git@github.com:o/inc.git\n",
        )
        .unwrap();
        // A file that includes itself stops at the depth limit.
        std::fs::write(inc.join(".git/loop"), "[include]\n\tpath = loop\n").unwrap();
        assert_eq!(key(&inc), "github.com/o/inc");
        // The parent's lines after the include are not in the included file's section.
        std::fs::write(
            inc.join(".git/config"),
            "[include]\n\tpath = upstream.inc\n\turl = https://x.org/not/origin\n",
        )
        .unwrap();
        std::fs::write(
            inc.join(".git/upstream.inc"),
            "[remote \"origin\"]\n\tfetch = x\n",
        )
        .unwrap();
        assert_eq!(key(&inc), inc.canonicalize().unwrap().to_string_lossy());

        let plain = tmp.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(key(&plain), plain.canonicalize().unwrap().to_string_lossy());
        std::fs::remove_dir_all(&tmp).ok();
    }
