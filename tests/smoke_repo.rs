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
    /// probe and the readme wait: read from the environment `ensure_repo` hands
    /// every `gh`, so the script is one fixed program and no path is ever
    /// spliced into shell source.
    const RECORD_ENV: &str = "ONEPIPELINE_SMOKE_GH_RECORD";
    const PROBE_ENV: &str = "ONEPIPELINE_SMOKE_GH_PROBE";
    const PROBE_STDERR_ENV: &str = "ONEPIPELINE_SMOKE_GH_PROBE_STDERR";
    /// How many readme-wait calls fail, and with what; every later one succeeds.
    const API_FAILURES_ENV: &str = "ONEPIPELINE_SMOKE_GH_API_FAILURES";
    const API_EXIT_ENV: &str = "ONEPIPELINE_SMOKE_GH_API_EXIT";
    const API_STDERR_ENV: &str = "ONEPIPELINE_SMOKE_GH_API_STDERR";

    /// What `gh repo view` prints for a repository that does not exist.
    const NOT_FOUND: &str = "GraphQL: Could not resolve to a Repository";

    /// What separates one argument from the next in the record: the unit
    /// separator, which no argument here carries, so a description with spaces
    /// in it comes back as one argument. The stand-in is given it as the octal
    /// escape `printf` reads, spelled from this one value.
    const SEPARATOR: char = '\u{1f}';

    /// The `gh` stand-in: one fixed program, reading the variables above,
    /// recording one invocation per line.
    fn stand_in_program() -> String {
        let separator = format!("\\{:03o}", SEPARATOR as u32);
        format!(
            "#!/bin/sh\n\
             printf '%s{separator}' \"$@\" >> \"${RECORD_ENV}\"\n\
             printf '\\n' >> \"${RECORD_ENV}\"\n\
             case \"$1 $2\" in\n  \
               'repo view') printf '%s\\n' \"${PROBE_STDERR_ENV}\" >&2; \
             exit \"${PROBE_ENV}\" ;;\n  \
               'api '*) calls=$(grep -c '^api' \"${RECORD_ENV}\"); \
             if [ \"$calls\" -le \"${{{API_FAILURES_ENV}:-0}}\" ]; then \
             printf '%s\\n' \"${API_STDERR_ENV}\" >&2; exit \"${API_EXIT_ENV}\"; fi; \
             exit 0 ;;\n  \
               *) exit 0 ;;\n\
             esac\n"
        )
    }

    /// A `gh` that records what it was asked and answers the `repo view` probe
    /// as `present` says, and everything else — the create, the readme wait —
    /// as done. Put first on `PATH`, so `ensure_repo`'s own `Command::new("gh")`
    /// resolves to it and nothing here can reach GitHub.
    fn stand_in(root: &Path, name: &str, present: bool) -> PathBuf {
        let probe = if present { (0, "") } else { (1, NOT_FOUND) };
        stand_in_answering(root, name, probe, (0, 0, ""))
    }

    /// A `gh` that answers the probe with `probe` (exit status, stderr) and the
    /// first `api.0` readme-wait calls with `api.1` and `api.2` (exit status,
    /// stderr), every later one as done.
    fn stand_in_answering(
        root: &Path,
        name: &str,
        probe: (i32, &str),
        api: (u32, i32, &str),
    ) -> PathBuf {
        let bin = root.join(name);
        fs::create_dir(&bin).expect("the stand-in has a bin directory");
        onepipeline_testfakes::executable(&bin.join("gh"), stand_in_program());
        let record = bin.join("record");
        std::env::set_var(RECORD_ENV, &record);
        std::env::set_var(PROBE_ENV, probe.0.to_string());
        std::env::set_var(PROBE_STDERR_ENV, probe.1);
        std::env::set_var(API_FAILURES_ENV, api.0.to_string());
        std::env::set_var(API_EXIT_ENV, api.1.to_string());
        std::env::set_var(API_STDERR_ENV, api.2);
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
                line.split(SEPARATOR)
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

    /// What `ensure_repo` panicked with, or a failure naming that it returned.
    fn refusal_of(slug: &str) -> String {
        let panic = std::panic::catch_unwind(|| repo::ensure_repo(slug))
            .expect_err("a refusal from `gh` fails the smoke rather than being read as an answer");
        match panic.downcast::<String>() {
            Ok(message) => *message,
            Err(panic) => (*panic.downcast::<&str>().expect("a panic message")).to_owned(),
        }
    }

    /// Only GitHub's explicit "not found" leads to a create. A rate limit, a
    /// rejected credential, a network failure or anything unrecognised fails the
    /// smoke carrying `gh`'s own exit status and stderr, and nothing is created
    /// — the create would only meet the existing repository and report its 422.
    /// The readme wait reads its refusals the same way: an empty or not yet
    /// visible repository is waited through, anything else is the answer.
    ///
    /// A test of its own beside the one above: `PATH` is process-global, and
    /// every recipe here runs this binary under nextest, which gives each test
    /// its own process. Within this test the cases run one after another.
    #[test]
    fn only_a_not_found_probe_creates_and_every_other_refusal_fails_carrying_it() {
        let scratch = scratch();
        let probe = ["repo", "view", THROWAWAY, "--json", "name"];

        for (name, status, stderr, names) in [
            (
                "rate-limited",
                1,
                "GraphQL: API rate limit already exceeded for user ID 1234.",
                "rate limit",
            ),
            (
                "unauthenticated",
                4,
                "To get started with GitHub CLI, please run:  gh auth login",
                "authentication",
            ),
            (
                "bad-credentials",
                1,
                "HTTP 401: Bad credentials (https://api.github.com/graphql)",
                "authentication",
            ),
            (
                "offline",
                1,
                "Post \"https://api.github.com/graphql\": dial tcp: lookup api.github.com: \
                 no such host",
                "transport",
            ),
            (
                "unrecognised",
                1,
                "HTTP 502: Bad Gateway (https://api.github.com/graphql)",
                "does not recognise",
            ),
        ] {
            let record = stand_in_answering(&scratch.0, name, (status, stderr), (0, 0, ""));
            let refusal = refusal_of(THROWAWAY);
            for carried in [names, stderr, &format!("exited {status}")] {
                assert!(
                    refusal.contains(carried),
                    "a {name} probe fails carrying {carried:?}: {refusal}"
                );
            }
            let invocations = recorded(&record);
            let [only] = &invocations[..] else {
                panic!("a {name} probe is never followed by a create: {invocations:?}")
            };
            assert_eq!(argv(only), probe);
        }

        // A repository that is not there yet, then empty, is waited through.
        let record = stand_in_answering(
            &scratch.0,
            "not-yet",
            (1, NOT_FOUND),
            (2, 1, "gh: Git Repository is empty. (HTTP 409)"),
        );
        repo::ensure_repo(THROWAWAY);
        let calls: Vec<_> = recorded(&record)
            .into_iter()
            .map(|call| call[0].clone())
            .collect();
        assert_eq!(
            calls,
            ["repo", "repo", "api", "api", "api"],
            "an empty repository is probed again until its first commit lands"
        );

        // Any other refusal ends the wait at once, carrying what `gh` said —
        // answered that way for longer than the whole wait, so a wait that
        // went on would fail on its own deadline rather than on the refusal.
        let limited = "gh: API rate limit exceeded for user ID 1234. (HTTP 403)";
        let record =
            stand_in_answering(&scratch.0, "wait-limited", (1, NOT_FOUND), (99, 1, limited));
        let started = std::time::Instant::now();
        let refusal = refusal_of(THROWAWAY);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "a refused readme wait fails promptly, not after {:?}",
            started.elapsed()
        );
        for carried in [
            "waiting for its first commit",
            "rate limit",
            limited,
            "exited 1",
        ] {
            assert!(
                refusal.contains(carried),
                "a refused readme wait fails carrying {carried:?}: {refusal}"
            );
        }
        let calls: Vec<_> = recorded(&record)
            .into_iter()
            .map(|call| call[0].clone())
            .collect();
        assert_eq!(
            calls,
            ["repo", "repo", "api"],
            "the refusal is not waited through"
        );
    }
}
