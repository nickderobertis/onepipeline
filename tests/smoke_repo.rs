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
#[path = "smoke/cleanup.rs"]
mod cleanup;

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::cleanup::{Created, Tracked};
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
            std::env::temp_dir().join(format!("onepipeline-scratch-repo-{}", std::process::id()));
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

    /// What `gh repo view` prints for a repository that does not exist. A copy
    /// of `gh`'s wording, reconciled against the real one by the credentialled
    /// smoke, which probes a name nothing creates and requires it read as this.
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
            !description.contains(THROWAWAY) && !description.contains(repo::SCRATCH_SUFFIX),
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

    fn refusal_of(slug: &str) -> String {
        panic_of(|| repo::ensure_repo(slug))
    }

    fn panic_of(run: impl FnOnce() + std::panic::UnwindSafe) -> String {
        let panic = std::panic::catch_unwind(run)
            .expect_err("a refusal fails the smoke rather than being read as an answer");
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
                "bad-gateway",
                1,
                "HTTP 502: Bad Gateway (https://api.github.com/graphql)",
                "GitHub failed answering it",
            ),
            (
                // GitHub's own wording, as `gh pr close` met it in CI run 39891240262.
                "graphql-went-wrong",
                1,
                "API call failed: GraphQL: Something went wrong while executing your query on \
                 2026-10-23T02:50:52Z. Please include `A848:1A14FE:7A60E:D5869:6ADACB8C` when \
                 reporting this issue.",
                "GitHub failed answering it",
            ),
            (
                "unrecognised",
                1,
                "HTTP 422: Validation Failed (https://api.github.com/graphql)",
                "does not recognise",
            ),
        ] {
            let record = stand_in_answering(&scratch.0, name, (status, stderr), (0, 0, ""));
            let refusal = refusal_of(THROWAWAY);
            for carried in [names, stderr, &format!("exit status: {status}")] {
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

        for (name, not_yet) in [
            ("not-yet-visible", "gh: Not Found (HTTP 404)"),
            (
                "not-yet-committed",
                "gh: Git Repository is empty. (HTTP 409)",
            ),
        ] {
            let record = stand_in_answering(&scratch.0, name, (1, NOT_FOUND), (2, 1, not_yet));
            repo::ensure_repo(THROWAWAY);
            let calls: Vec<_> = recorded(&record)
                .into_iter()
                .map(|call| call[0].clone())
                .collect();
            assert_eq!(
                calls,
                ["repo", "repo", "api", "api", "api"],
                "the readme wait asks again while it is answered {not_yet:?}"
            );
        }

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
            "exit status: 1",
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

        // A `gh` that cannot be started is a missing tool, told how to fix it,
        // and never read as an answer about the repository.
        let empty = scratch.0.join("no-gh");
        fs::create_dir(&empty).expect("a PATH directory with no `gh` in it");
        std::env::set_var("PATH", &empty);
        let refusal = refusal_of(THROWAWAY);
        for carried in [
            "`gh` could not be run",
            "Install `gh`",
            "could not tell whether",
        ] {
            assert!(
                refusal.contains(carried),
                "a missing `gh` fails carrying {carried:?}: {refusal}"
            );
        }
    }

    /// A read is asked again while GitHub fails answering it, and only then: a read that
    /// meets GitHub's own failure twice returns what the third asking got, one that meets
    /// nothing else ends after [`repo::READ_ATTEMPTS`] askings carrying the last failure, and
    /// any refusal that is an answer — here a not-found — ends it at the first. That is
    /// what lets the smoke's cleanup read back what a close GitHub failed to answer did.
    ///
    /// Its own test, and so its own process under nextest: `PATH` is process-global.
    #[test]
    fn a_read_is_asked_again_only_while_github_fails_answering_it() {
        let scratch = scratch();
        let read = [
            "api",
            "repos/nobody/onepipeline-throwaway-smoke/git/ref/heads/b",
        ];
        let went_wrong = "API call failed: GraphQL: Something went wrong while executing your \
                          query on 2026-10-23T02:50:52Z.";
        let api_calls = |record: &Path| {
            recorded(record)
                .iter()
                .filter(|call| call[0] == "api")
                .count()
        };

        let record = stand_in_answering(&scratch.0, "recovers", (0, ""), (2, 1, went_wrong));
        assert!(
            repo::gh_read(&read).is_ok(),
            "a read answered on its third asking is answered"
        );
        assert_eq!(
            api_calls(&record),
            3,
            "asked until it was answered, then no more"
        );

        let record = stand_in_answering(&scratch.0, "never", (0, ""), (99, 1, went_wrong));
        let refusal = repo::gh_read(&read).expect_err("a read GitHub never answers fails");
        assert_eq!(refusal.cause, repo::Cause::Upstream, "{refusal}");
        assert!(refusal.to_string().contains(went_wrong), "{refusal}");
        assert_eq!(
            api_calls(&record),
            repo::READ_ATTEMPTS as usize,
            "asked a bounded number of times"
        );

        let not_found = "gh: Not Found (HTTP 404)";
        let record = stand_in_answering(&scratch.0, "answered", (0, ""), (99, 1, not_found));
        let refusal = repo::gh_read(&read).expect_err("a not-found is an answer");
        assert_eq!(refusal.cause, repo::Cause::NotFound, "{refusal}");
        assert_eq!(api_calls(&record), 1, "an answer is never asked again");
    }

    /// Where the cleanup stand-in keeps the scratch repository it plays: `pulls/<n>/` holds
    /// each pull request's `state`, `head` and `title`, `branches/<name>` each branch, and
    /// `close-error` / `delete-error`, when present, are what a close or a delete is refused
    /// with — doing what it was asked first only where `close-takes` / `delete-takes` is
    /// there, as GitHub sometimes does behind its own failure.
    const STATE_ENV: &str = "ONEPIPELINE_SMOKE_GH_STATE";

    /// The cleanup's `gh` stand-in: one fixed program answering the five calls
    /// [`Created::clean`] makes from the files under [`STATE_ENV`], and recording each.
    fn cleanup_stand_in_program() -> String {
        format!(
            r#"#!/bin/sh
s="${STATE_ENV}"
printf '%s\n' "$*" >> "$s/record"
refuse() {{ cat "$s/$1-error" >&2; exit 1; }}
case "$1 $2" in
  'pr list')
    printf '['; sep=''
    for d in "$s"/pulls/*; do
      [ "$(cat "$d/state")" = OPEN ] || continue
      printf '%s{{"number":%s,"title":"%s"}}' "$sep" "$(basename "$d")" "$(cat "$d/title")"
      sep=,
    done
    printf ']\n' ;;
  'pr view')
    d="$s/pulls/$3"
    printf '{{"state":"%s","headRefName":"%s"}}\n' "$(cat "$d/state")" "$(cat "$d/head")" ;;
  'pr close')
    if [ -f "$s/close-error" ]; then
      [ -f "$s/close-takes" ] && printf CLOSED > "$s/pulls/$3/state"
      refuse close
    fi
    printf CLOSED > "$s/pulls/$3/state" ;;
  'api -X')
    b="${{4#*/git/refs/heads/}}"
    if [ -f "$s/delete-error" ]; then
      [ -f "$s/delete-takes" ] && rm -f "$s/branches/$b"
      refuse delete
    fi
    rm -f "$s/branches/$b" ;;
  'api '*)
    case "$2" in
      */git/ref/heads/*)
        [ -f "$s/branches/${{2#*/git/ref/heads/}}" ] && {{ echo '{{}}'; exit 0; }}
        echo 'gh: Not Found (HTTP 404)' >&2; exit 1 ;;
      */git/matching-refs/heads/*)
        p="${{2#*/git/matching-refs/heads/}}"
        printf '['; sep=''
        for f in $(cd "$s/branches" && find . -type f | sed 's|^\./||'); do
          case "$f" in "$p"*) printf '%s{{"ref":"refs/heads/%s"}}' "$sep" "$f"; sep=, ;; esac
        done
        printf ']\n' ;;
    esac ;;
  *) echo "the cleanup stand-in was asked something it does not play: $*" >&2; exit 2 ;;
esac
"#
        )
    }

    /// A scratch repository holding one open pull request #7, titled with `STAMP`, from the
    /// branch `run-x/a` that is still there — what a journey leaves for its cleanup — with a
    /// close and a delete each refused with `refused` and taken or not as `takes` says.
    fn cleanup_repository(root: &Path, name: &str, refused: &str, takes: (bool, bool)) -> PathBuf {
        let state = root.join(name);
        let pull = state.join("pulls").join("7");
        fs::create_dir_all(&pull).expect("the stand-in's pull request");
        fs::write(pull.join("state"), "OPEN").expect("its state");
        fs::write(pull.join("head"), "run-x/a").expect("its head");
        fs::write(pull.join("title"), format!("feat: a ({STAMP})")).expect("its title");
        fs::create_dir_all(state.join("branches").join("run-x")).expect("the branches");
        fs::write(state.join("branches").join("run-x").join("a"), "").expect("its branch");
        for (what, taken) in [("close", takes.0), ("delete", takes.1)] {
            fs::write(state.join(format!("{what}-error")), refused).expect("the refusal");
            if taken {
                fs::write(state.join(format!("{what}-takes")), "").expect("the marker");
            }
        }
        let bin = state.join("bin");
        fs::create_dir(&bin).expect("the stand-in has a bin directory");
        onepipeline_testfakes::executable(&bin.join("gh"), cleanup_stand_in_program());
        std::env::set_var(STATE_ENV, &state);
        let host_path = std::env::var_os("PATH").expect("the host has a PATH");
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&host_path));
        std::env::set_var(
            "PATH",
            std::env::join_paths(paths).expect("the test PATH joins"),
        );
        state
    }

    const STAMP: &str = "run-x";

    /// The journey's own last step, [`Created::clean`], run against the stand-in for what
    /// one host recorded opening: #7 from `run-x/a`.
    fn clean() -> (Vec<String>, Vec<String>) {
        let mut created = Created {
            slug: THROWAWAY.to_owned(),
            stamp: STAMP.to_owned(),
            prefixes: vec!["run-x/".to_owned()],
            cleaned: false,
        };
        let tracked = Tracked {
            pulls: [7].into(),
            branches: ["run-x/a".to_owned()].into(),
        };
        let cleanup = created.clean(&tracked);
        (cleanup.observed, cleanup.failures)
    }

    /// A close or a delete GitHub fails to answer is decided by the cleanup's read-back, not
    /// by the refusal: GitHub's own failure — the wording `gh pr close` met in CI run
    /// 39891240262 — over a close and a delete that both happened is a finished cleanup, and
    /// the same failure over a close that did not, or a delete that did not, fails it naming
    /// what the read-back still found. A refusal that is an answer about the request fails
    /// it however the read-back reads, as it always did.
    ///
    /// Its own test, and so its own process under nextest: `PATH` is process-global.
    #[test]
    fn a_cleanup_github_failed_to_answer_is_decided_by_what_it_reads_back() {
        let scratch = scratch();
        let went_wrong = "API call failed: GraphQL: Something went wrong while executing your \
                          query on 2026-10-23T02:50:52Z.";

        cleanup_repository(&scratch.0, "both-taken", went_wrong, (true, true));
        let (observed, failures) = clean();
        assert!(
            failures.is_empty(),
            "a close and a delete that happened behind GitHub's failure finish the cleanup: \
             {failures:#?} (observed {observed:#?})"
        );
        for step in [
            "closing #7, left to the read-back",
            "deleting run-x/a, left to the read-back",
            r#"read back from GitHub: closed [7] of {7}; gone ["run-x/a"] of {"run-x/a"}"#,
        ] {
            assert!(
                observed.iter().any(|seen| seen.contains(step)),
                "the cleanup records {step:?}: {observed:#?}"
            );
        }

        cleanup_repository(&scratch.0, "close-lost", went_wrong, (false, true));
        let (_, failures) = clean();
        for found in [
            "#7 reads OPEN after the cleanup",
            "still open after the cleanup: {7}",
        ] {
            assert!(
                failures.iter().any(|failure| failure.contains(found)),
                "a close the read-back finds undone fails the cleanup with {found:?}: \
                 {failures:#?}"
            );
        }

        cleanup_repository(&scratch.0, "delete-lost", went_wrong, (true, false));
        let (_, failures) = clean();
        for found in [
            "run-x/a is still on the remote after the cleanup",
            r#"branches under the prefix after the cleanup: {"run-x/a"}"#,
        ] {
            assert!(
                failures.iter().any(|failure| failure.contains(found)),
                "a delete the read-back finds undone fails the cleanup with {found:?}: \
                 {failures:#?}"
            );
        }

        let invalid = "HTTP 422: Validation Failed (https://api.github.com/graphql)";
        cleanup_repository(&scratch.0, "answered", invalid, (true, true));
        let (_, failures) = clean();
        for found in ["closing #7: ", "deleting run-x/a: ", invalid] {
            assert!(
                failures.iter().any(|failure| failure.contains(found)),
                "a refusal that answers about the request fails the cleanup with {found:?}: \
                 {failures:#?}"
            );
        }
    }

    /// A synthetic scratch identity, owned by nobody: every `gh` it reaches is
    /// the stand-in, so it is never looked up anywhere real.
    const CONFIGURED: &str = "hiddenco/quietharbor-smoke";

    /// The scratch repository is configuration, read from one named key and held
    /// in no tracked file. Unset — or set to nothing but whitespace — it is a
    /// refusal naming the key before any `gh` is asked anything, never a skip and
    /// never a fallback. Set, it is the repository the smoke's first host
    /// operation reaches, as the stand-in recorded it. A name without the scratch
    /// suffix is refused before it is touched.
    ///
    /// Its own test, and so its own process under nextest: the key and `PATH`
    /// are both process-global.
    #[test]
    fn the_scratch_repository_is_read_from_its_key_and_refused_naming_it_when_unset() {
        let scratch = scratch();

        for (name, value) in [("unset", None), ("blank", Some("  "))] {
            let record = stand_in(&scratch.0, name, true);
            match value {
                Some(value) => std::env::set_var(repo::SMOKE_REPO_ENV, value),
                None => std::env::remove_var(repo::SMOKE_REPO_ENV),
            }
            let refusal = panic_of(|| repo::ensure_repo(&repo::scratch_repo()));
            assert!(
                refusal.contains("ONEPIPELINE_SMOKE_REPO is not set"),
                "an {name} key is refused naming it: {refusal}"
            );
            assert!(
                !record.exists(),
                "an {name} key reaches no host at all: {:?}",
                fs::read_to_string(&record)
            );
        }

        std::env::set_var(repo::SMOKE_REPO_ENV, "hiddenco/quietharbor");
        let record = stand_in(&scratch.0, "not-scratch", true);
        let refusal = panic_of(|| repo::ensure_repo(&repo::scratch_repo()));
        assert!(
            refusal.contains("is not a scratch repository"),
            "a name without the scratch suffix is refused: {refusal}"
        );
        assert!(!record.exists(), "a refused name reaches no host");

        std::env::set_var(repo::SMOKE_REPO_ENV, format!(" {CONFIGURED}\n"));
        let record = stand_in(&scratch.0, "configured", true);
        let slug = repo::scratch_repo();
        assert_eq!(
            slug, CONFIGURED,
            "the key's value, trimmed, is the identity"
        );
        repo::ensure_repo(&slug);
        let invocations = recorded(&record);
        let [probe] = &invocations[..] else {
            panic!("the configured repository is probed and reused: {invocations:?}")
        };
        assert_eq!(
            argv(probe),
            ["repo", "view", CONFIGURED, "--json", "name"],
            "the host is asked about the configured repository and no other"
        );
    }
}
