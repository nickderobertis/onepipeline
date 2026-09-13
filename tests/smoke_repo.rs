//! What the real-everything smoke writes on the scratch repository it creates.
//!
//! `just smoke-real` drives `ensure_repo` against the repository that already
//! exists, so it never reaches the `gh repo create` call and cannot prove what a
//! created repository is told about itself. That sentence has cost two people
//! time: the one it used to write — "Nothing here is kept" — read as an
//! invitation to delete, and each of them renamed the repository aside believing
//! they were retiring leaked scratch, while the smoke went on writing to it
//! through the redirect a rename leaves behind. So the sentence is proven here,
//! in the offline tier, by driving the real `ensure_repo` with a `gh` on `PATH`
//! that never reaches GitHub: it answers the probe and records every invocation.

#[cfg(unix)]
#[path = "smoke/repo.rs"]
mod repo;

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use crate::repo;

    /// A slug nothing owns: the probe is answered by the stand-in, so no name
    /// is ever looked up.
    const THROWAWAY: &str = "nobody/onepipeline-throwaway-smoke";

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("the scratch directory is removed");
        }
    }

    fn scratch() -> Scratch {
        let root =
            std::env::temp_dir().join(format!("onepipeline-smoke-repo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("a fresh scratch directory");
        Scratch(root)
    }

    /// Where the stand-in records what it was asked, and how it answers the
    /// probe: read from the environment `ensure_repo` hands every `gh`, so the
    /// script is one fixed program and no path is ever spliced into shell source.
    const RECORD_ENV: &str = "ONEPIPELINE_SMOKE_GH_RECORD";
    const PROBE_ENV: &str = "ONEPIPELINE_SMOKE_GH_PROBE";

    /// The `gh` stand-in: one fixed program, reading the two variables above.
    /// One invocation per line, arguments separated by the unit separator: no
    /// argument here carries either, and a description with spaces in it has
    /// to come back as one argument.
    fn stand_in_program() -> String {
        format!(
            "#!/bin/sh\n\
             printf '%s\\037' \"$@\" >> \"${RECORD_ENV}\"\n\
             printf '\\n' >> \"${RECORD_ENV}\"\n\
             case \"$1 $2\" in\n  \
               'repo view') echo 'GraphQL: Could not resolve to a Repository' >&2; \
             exit \"${PROBE_ENV}\" ;;\n  \
               *) exit 0 ;;\n\
             esac\n"
        )
    }

    /// A `gh` that records what it was asked and answers the `repo view` probe
    /// as `present` says, and everything else — the create, the readme wait —
    /// as done. Put first on `PATH`, so `ensure_repo`'s own `Command::new("gh")`
    /// resolves to it and nothing here can reach GitHub.
    fn stand_in(root: &Path, name: &str, present: bool) -> PathBuf {
        let bin = root.join(name);
        fs::create_dir(&bin).expect("the stand-in has a bin directory");
        let gh = bin.join("gh");
        fs::write(&gh, stand_in_program()).expect("the stand-in is written");
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755))
            .expect("the stand-in is runnable");
        let record = bin.join("record");
        std::env::set_var(RECORD_ENV, &record);
        std::env::set_var(PROBE_ENV, if present { "0" } else { "1" });
        let host_path = std::env::var_os("PATH").expect("the host has a PATH");
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&host_path));
        std::env::set_var(
            "PATH",
            std::env::join_paths(paths).expect("the test PATH joins"),
        );
        record
    }

    /// Every `gh` invocation the stand-in saw, in order, each as its argv.
    fn recorded(record: &Path) -> Vec<Vec<String>> {
        fs::read_to_string(record)
            .expect("the stand-in recorded what it was asked")
            .lines()
            .map(|line| {
                line.split('\u{1f}')
                    .filter(|arg| !arg.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .collect()
    }

    fn argv(invocation: &[String]) -> Vec<&str> {
        invocation.iter().map(String::as_str).collect()
    }

    /// One test rather than two, because `PATH` is process-global: two tests
    /// pointing it at two stand-ins from two threads would each drive the
    /// other's.
    #[test]
    fn a_created_scratch_repository_is_told_it_is_kept_and_an_existing_one_is_reused() {
        let scratch = scratch();

        let record = stand_in(&scratch.0, "absent", false);
        repo::ensure_repo(THROWAWAY);
        let invocations = recorded(&record);
        let [probe, create, readme] = &invocations[..] else {
            panic!("a missing repository is probed, created, and waited for, not {invocations:?}")
        };
        assert_eq!(argv(probe), ["repo", "view", THROWAWAY, "--json", "name"]);
        assert_eq!(
            argv(readme),
            ["api", &format!("repos/{THROWAWAY}/commits/HEAD")],
            "the readme wait is what follows the create"
        );
        let (description, flags) = create
            .split_last()
            .expect("the create invocation carries arguments");
        assert_eq!(
            argv(flags),
            [
                "repo",
                "create",
                THROWAWAY,
                "--private",
                "--add-readme",
                "--description"
            ],
            "a scratch repository is created private, with a readme, and described"
        );

        // What the description says is what a person on the repository's GitHub
        // page acts on: that it is meant to be there, that every run of the
        // smoke uses it, that removing or renaming it retires nothing, and where
        // the code that owns it is.
        assert!(
            !description.contains("Nothing here is kept"),
            "the sentence that invited deletion is gone: {description:?}"
        );
        for said in [
            "Intentional",
            "reused by every run",
            "never deletes it",
            "Deleting or renaming it is no cleanup",
            "redirecting",
            "tests/smoke/main.rs in nickderobertis/onepipeline",
        ] {
            assert!(
                description.contains(said),
                "the description says {said:?}: {description:?}"
            );
        }
        // GitHub's own refusal of a longer one reads "description is too long
        // (maximum is 350 characters)", and the create call that would meet it
        // runs only on an account with no scratch repository yet.
        assert!(
            description.chars().count() < 350,
            "GitHub refuses a description over 350 characters; this one is {}",
            description.chars().count()
        );
        // Named by neither its slug nor its name, so the same sentence stays
        // true after a rename — the case that made it necessary.
        assert!(
            !description.contains(THROWAWAY) && !description.contains("onepipeline-smoke"),
            "the description names no slug: {description:?}"
        );

        let record = stand_in(&scratch.0, "present", true);
        repo::ensure_repo(THROWAWAY);
        let invocations = recorded(&record);
        let [probe] = &invocations[..] else {
            panic!("an existing repository is probed and reused, never recreated: {invocations:?}")
        };
        assert_eq!(argv(probe), ["repo", "view", THROWAWAY, "--json", "name"]);
    }
}
