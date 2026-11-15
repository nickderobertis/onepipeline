//! Prototype (spike-git-reads, never lands): every mode answers each read exactly
//! as git does — status, standard output and standard error — on real
//! repositories, and answers the common ones without starting git.

use super::*;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static ANSWERED: AtomicUsize = AtomicUsize::new(0);

const MODES: [&str; 5] = [
    "batched",
    "native",
    "reuse",
    "reuse+batched",
    "reuse+native",
];

fn env_for_tests(root: &Path) {
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            std::env::remove_var(name);
        }
    }
    std::env::set_var("HOME", root);
    std::env::set_var("XDG_CONFIG_HOME", root);
    std::env::set_var("ONEVCS_HOME", root.join("onevcs"));
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    let global = root.join("gitconfig");
    std::fs::write(
        &global,
        "[user]\n\tname = Spike\n\temail = spike@example.invalid\n[init]\n\tdefaultBranch = main\n[advice]\n\tdetachedHead = false\n",
    )
    .unwrap();
    std::env::set_var("GIT_CONFIG_GLOBAL", &global);
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// git's own answer, from a process of its own.
fn truth(cwd: &Path, args: &[&str], env: &[(String, String)]) -> (i32, String, String) {
    let mut command = Command::new("git");
    command.args(args).current_dir(cwd).stdin(Stdio::null());
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// What this crate answers under `mode`, counting the answers no git process gave.
fn asked(mode: &str, cwd: &Path, args: &[&str], env: &[(String, String)]) -> (i32, String, String) {
    let mode = Mode::parse(mode).unwrap();
    with_mode(mode, || {
        let call = Call::new(args, Some(cwd), env, false);
        if call.answer().is_some() {
            ANSWERED.fetch_add(1, Ordering::Relaxed);
        }
        let output = crate::git::run_with_env(args, Some(cwd), env).unwrap();
        (output.status, output.stdout, output.stderr)
    })
}

/// Every query, in every mode, twice (the second time from reuse where it may),
/// equals git's own answer.
fn agree(cwd: &Path, queries: &[Vec<String>], env: &[(String, String)]) {
    for mode in MODES {
        for query in queries {
            let args: Vec<&str> = query.iter().map(String::as_str).collect();
            for round in 0..2 {
                let expected = truth(cwd, &args, env);
                let actual = asked(mode, cwd, &args, env);
                if env.is_empty() {
                    with_mode(Mode::parse(mode).unwrap(), || {
                        let read = EngineRead::new(&args, cwd);
                        let answer = read.answer().unwrap_or_else(|| {
                            let answer = truth(cwd, &args, env);
                            read.ran(Some(answer.0), answer.1.as_bytes(), answer.2.as_bytes());
                            answer
                        });
                        assert_eq!(answer, expected, "engine adapter: {mode} {args:?}");
                    });
                }
                assert_eq!(
                    actual,
                    expected,
                    "{mode} answered `git {}` in {} differently from git (round {round})",
                    args.join(" "),
                    cwd.display()
                );
            }
        }
        batched::release();
        reuse::forget();
    }
}

fn queries(words: &[&str]) -> Vec<Vec<String>> {
    words
        .iter()
        .map(|line| line.split(' ').map(str::to_owned).collect())
        .collect()
}

struct World {
    _root: tempfile::TempDir,
    origin: PathBuf,
    clone: PathBuf,
    first: String,
    second: String,
    tree: String,
}

/// An origin with two commits and an annotated tag, and a clone of it with
/// remote-tracking refs and `origin/HEAD`. Old enough that reuse may remember.
fn world() -> World {
    let root = tempfile::tempdir().unwrap();
    env_for_tests(root.path());
    let origin = root.path().join("origin");
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "-q", "-b", "main"]);
    std::fs::write(origin.join("a.txt"), "one\n").unwrap();
    git(&origin, &["add", "a.txt"]);
    git(&origin, &["commit", "-q", "-m", "first"]);
    let first = git(&origin, &["rev-parse", "HEAD"]);
    std::fs::write(origin.join("b.txt"), "two\n").unwrap();
    git(&origin, &["add", "b.txt"]);
    git(&origin, &["commit", "-q", "-m", "second"]);
    let second = git(&origin, &["rev-parse", "HEAD"]);
    let tree = git(&origin, &["rev-parse", "HEAD^{tree}"]);
    git(&origin, &["tag", "-a", "v1", "-m", "v1", &first]);
    git(&origin, &["branch", "feature", &first]);
    let clone = root.path().join("clone");
    git(
        root.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    std::fs::create_dir(clone.join("sub")).unwrap();
    std::thread::sleep(Duration::from_millis(80));
    World {
        _root: root,
        origin,
        clone,
        first,
        second,
        tree,
    }
}

fn reads(w: &World) -> Vec<Vec<String>> {
    let (first, second, tree) = (&w.first, &w.second, &w.tree);
    let missing = "0123456789abcdef0123456789abcdef01234567";
    queries(&[
        "show-ref --verify --quiet refs/heads/main",
        "show-ref --verify --quiet refs/remotes/origin/main",
        "show-ref --verify --quiet refs/remotes/origin/HEAD",
        "show-ref --verify --quiet refs/heads/missing",
        "show-ref --verify --quiet refs/tags/v1",
        "symbolic-ref --short refs/remotes/origin/HEAD",
        "symbolic-ref -q refs/remotes/origin/HEAD",
        "symbolic-ref --short HEAD",
        "symbolic-ref --quiet --short HEAD",
        "symbolic-ref -q refs/heads/main",
        "rev-parse --abbrev-ref HEAD",
        "rev-parse --is-inside-work-tree",
        "rev-parse --verify origin/main",
        "rev-parse --verify refs/heads/main",
        "rev-parse --verify refs/remotes/origin/main",
        "rev-parse --verify --quiet refs/heads/missing",
        "rev-parse --verify missing",
        "rev-parse --verify MERGE_HEAD^{commit}",
        "rev-parse --verify HEAD^{commit}",
        "rev-parse --verify v1",
        "rev-parse --verify v1^{commit}",
        "rev-parse HEAD",
        "rev-parse --git-path hooks",
        "rev-parse --git-common-dir",
        "remote get-url origin",
        "config --get core.hooksPath",
        "log --format=%H origin/main --",
        "for-each-ref --format=%(refname:short) refs/heads",
    ])
    .into_iter()
    .chain(queries(&[
        &format!("rev-parse --verify {first}^{{commit}}"),
        &format!("rev-parse --verify --quiet {missing}^{{commit}}"),
        &format!("rev-parse --verify {missing}"),
        &format!("cat-file -e {second}^{{commit}}"),
        &format!("cat-file -e {first}"),
        &format!("cat-file -e {missing}"),
        &format!("cat-file -e {tree}^{{commit}}"),
        "cat-file -e origin/main",
        &format!("merge-base --is-ancestor {first} {second}"),
        &format!("merge-base --is-ancestor {second} {first}"),
        &format!("merge-base --is-ancestor {first} origin/main"),
        &format!("merge-base --is-ancestor {first} {first}"),
        &format!("diff --name-only --no-renames -z {first} {second}"),
    ]))
    .collect()
}

#[test]
fn every_mode_answers_a_plain_clone_as_git_does() {
    let w = world();
    let before = ANSWERED.load(Ordering::Relaxed);
    agree(&w.clone, &reads(&w), &[]);
    agree(
        &w.clone.join("sub"),
        &queries(&["rev-parse --is-inside-work-tree", "rev-parse HEAD"]),
        &[],
    );
    agree(
        &w.clone.join(".git"),
        &queries(&["rev-parse --is-inside-work-tree"]),
        &[],
    );
    assert!(
        ANSWERED.load(Ordering::Relaxed) - before > 100,
        "the prototypes answered almost nothing without git"
    );
}

#[test]
fn every_mode_answers_packed_refs_as_git_does() {
    let w = world();
    git(&w.clone, &["pack-refs", "--all"]);
    std::thread::sleep(Duration::from_millis(80));
    agree(&w.clone, &reads(&w), &[]);
}

#[test]
fn every_mode_answers_a_linked_worktree_as_git_does() {
    let w = world();
    let linked = w.clone.parent().unwrap().join("linked");
    git(
        &w.clone,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            linked.to_str().unwrap(),
            "origin/main",
        ],
    );
    std::thread::sleep(Duration::from_millis(80));
    agree(&linked, &reads(&w), &[]);
}

#[test]
fn every_mode_answers_a_clone_borrowing_objects_through_alternates_as_git_does() {
    let w = world();
    let shared = w.clone.parent().unwrap().join("shared");
    git(
        w.clone.parent().unwrap(),
        &[
            "clone",
            "-q",
            "--shared",
            w.origin.to_str().unwrap(),
            shared.to_str().unwrap(),
        ],
    );
    assert!(shared.join(".git/objects/info/alternates").exists());
    std::thread::sleep(Duration::from_millis(80));
    agree(&shared, &reads(&w), &[]);
}

#[test]
fn every_mode_leaves_ambiguous_and_dangling_names_to_git() {
    let w = world();
    // A branch and a tag of one name, a ref named like an object, and an
    // `origin/HEAD` that points at a branch that is gone.
    git(&w.clone, &["branch", "v1", &w.second]);
    git(
        &w.clone,
        &["update-ref", &format!("refs/heads/{}", w.first), &w.second],
    );
    git(
        &w.clone,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/gone",
        ],
    );
    std::fs::write(w.clone.join("HEAD"), "a file named HEAD\n").unwrap();
    std::thread::sleep(Duration::from_millis(80));
    let first = &w.first;
    agree(
        &w.clone,
        &queries(&[
            "rev-parse --verify v1",
            "rev-parse --verify v1^{commit}",
            "rev-parse HEAD",
            "rev-parse --abbrev-ref HEAD",
            "symbolic-ref --short refs/remotes/origin/HEAD",
            "show-ref --verify --quiet refs/remotes/origin/HEAD",
            "rev-parse --verify origin",
            &format!("rev-parse --verify {first}"),
            &format!("cat-file -e {first}^{{commit}}"),
        ]),
        &[],
    );
}

#[test]
fn every_mode_leaves_a_relocated_repository_to_git() {
    let w = world();
    let elsewhere = w.origin.join(".git").to_string_lossy().into_owned();
    let env = vec![("GIT_DIR".to_owned(), elsewhere)];
    let before = ANSWERED.load(Ordering::Relaxed);
    agree(&w.clone, &reads(&w), &env);
    assert_eq!(
        ANSWERED.load(Ordering::Relaxed),
        before,
        "GIT_DIR reads must be git's"
    );
}

#[test]
fn every_mode_sees_a_ref_another_process_moves_between_two_reads() {
    for mode in MODES {
        let w = world();
        for packed in [false, true] {
            if packed {
                git(&w.clone, &["pack-refs", "--all"]);
                std::thread::sleep(Duration::from_millis(80));
            }
            for args in [
                vec!["rev-parse", "--verify", "refs/remotes/origin/main"],
                vec!["show-ref", "--verify", "--quiet", "refs/heads/later"],
                vec!["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
                vec!["rev-parse", "--abbrev-ref", "HEAD"],
            ] {
                let before = asked(mode, &w.clone, &args, &[]);
                assert_eq!(before, truth(&w.clone, &args, &[]), "{mode} {args:?}");
                assert_eq!(
                    asked(mode, &w.clone, &args, &[]),
                    before,
                    "{mode} {args:?} again"
                );
            }
            // Another process moves every one of them, through git, not through
            // this crate, so nothing here is told.
            let target = if packed { &w.second } else { &w.first };
            git(
                &w.clone,
                &["update-ref", "refs/remotes/origin/main", target],
            );
            git(&w.clone, &["update-ref", "refs/heads/later", &w.first]);
            git(
                &w.clone,
                &[
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    "refs/remotes/origin/feature",
                ],
            );
            git(
                &w.clone,
                &["checkout", "-q", "-b", &format!("moved-{packed}")],
            );
            for args in [
                vec!["rev-parse", "--verify", "refs/remotes/origin/main"],
                vec!["show-ref", "--verify", "--quiet", "refs/heads/later"],
                vec!["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
                vec!["rev-parse", "--abbrev-ref", "HEAD"],
            ] {
                assert_eq!(
                    asked(mode, &w.clone, &args, &[]),
                    truth(&w.clone, &args, &[]),
                    "{mode} answered {args:?} from before another process moved it"
                );
            }
            git(&w.clone, &["update-ref", "-d", "refs/heads/later"]);
            std::thread::sleep(Duration::from_millis(80));
        }
        batched::release();
        reuse::forget();
    }
}

#[test]
fn reuse_answers_a_repeated_read_without_git_and_forgets_on_a_write() {
    let w = world();
    let mode = Mode::parse("reuse").unwrap();
    let args = ["rev-parse", "--git-path", "hooks"];
    let first = with_mode(mode, || {
        Call::new(&args, Some(&w.clone), &[], false).answer()
    });
    assert!(
        first.is_none(),
        "nothing is remembered before the first read"
    );
    asked("reuse", &w.clone, &args, &[]);
    let second = with_mode(mode, || {
        Call::new(&args, Some(&w.clone), &[], false).answer()
    });
    assert_eq!(
        second.map(|o| o.stdout),
        Some(truth(&w.clone, &args, &[]).1)
    );
    with_mode(mode, || {
        crate::git::run(&["config", "core.hooksPath", "elsewhere"], Some(&w.clone)).unwrap()
    });
    let third = with_mode(mode, || {
        Call::new(&args, Some(&w.clone), &[], false).answer()
    });
    assert!(third.is_none(), "a write must forget what reuse held");
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(
        asked("reuse", &w.clone, &args, &[]),
        truth(&w.clone, &args, &[])
    );
}

#[test]
fn batch_readers_end_when_idle_and_are_released_explicitly() {
    let w = world();
    let mode = Mode::parse("batched").unwrap();
    let head = with_mode(mode, || {
        Call::new(&["rev-parse", "HEAD"], Some(&w.clone), &[], false).answer()
    });
    assert_eq!(head.map(|o| o.stdout), Some(format!("{}\n", w.second)));
    let open = || batched_open_count();
    assert_eq!(open(), 1);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(open(), 0, "an idle batch reader must end");
    with_mode(mode, || {
        Call::new(&["rev-parse", "HEAD"], Some(&w.clone), &[], false).answer()
    });
    assert_eq!(open(), 1);
    with_mode(mode, release);
    assert_eq!(open(), 0, "release must end every batch reader");
}

fn batched_open_count() -> usize {
    batched::count()
}

#[test]
fn modes_are_named_as_the_demonstrations_name_them() {
    assert_eq!(Mode::parse("stock"), Some(Mode::STOCK));
    assert_eq!(
        Mode::parse("reuse+native"),
        Some(Mode {
            reuse: true,
            reader: Reader::Native
        })
    );
    assert_eq!(
        Mode::parse("reuse"),
        Some(Mode {
            reuse: true,
            reader: Reader::Git
        })
    );
    assert_eq!(
        Mode::parse("batched"),
        Some(Mode {
            reuse: false,
            reader: Reader::Batched
        })
    );
    assert_eq!(Mode::parse("reuse+"), None);
    assert_eq!(Mode::parse("fast"), None);
}

#[test]
fn spike_safety_failed_symbolic_delete_forgets_a_warm_read() {
    let w = world();
    let args = ["rev-parse", "--git-path", "hooks"];
    for write in [
        vec!["symbolic-ref", "--delete", "refs/heads/absent"],
        vec!["log", "--output=absent-dir/result"],
        vec!["merge-tree"],
        vec!["tag", "--contains=HEAD", "--delete", "absent"],
    ] {
        asked("reuse", &w.clone, &args, &[]);
        with_mode(Mode::parse("reuse").unwrap(), || {
            assert!(Call::new(&args, Some(&w.clone), &[], false)
                .answer()
                .is_some());
            let written = crate::git::run(&write, Some(&w.clone)).unwrap();
            assert_ne!(written.status, 0);
            assert!(
                Call::new(&args, Some(&w.clone), &[], false)
                    .answer()
                    .is_none(),
                "even a failed write attempt invalidates the memo before git runs: {write:?}"
            );
        });
    }
}

#[test]
fn spike_safety_symbolic_cycles_delegate_to_git() {
    let w = world();
    git(
        &w.clone,
        &["symbolic-ref", "refs/heads/loop-a", "refs/heads/loop-b"],
    );
    git(
        &w.clone,
        &["symbolic-ref", "refs/heads/loop-b", "refs/heads/loop-a"],
    );
    agree(&w.clone, &queries(&["symbolic-ref refs/heads/loop-a"]), &[]);
}

#[cfg(unix)]
#[test]
fn spike_safety_unreadable_refs_never_authorize_reuse() {
    use std::os::unix::fs::PermissionsExt;
    let w = world();
    let heads = w.clone.join(".git/refs/heads");
    let permissions = std::fs::metadata(&heads).unwrap().permissions();
    std::fs::set_permissions(&heads, std::fs::Permissions::from_mode(0)).unwrap();
    std::thread::sleep(Duration::from_millis(80));
    let args = ["rev-parse", "--git-path", "hooks"];
    let actual = asked("reuse", &w.clone, &args, &[]);
    let expected = truth(&w.clone, &args, &[]);
    let reused = with_mode(Mode::parse("reuse").unwrap(), || {
        Call::new(&args, Some(&w.clone), &[], false)
            .answer()
            .is_some()
    });
    std::fs::set_permissions(&heads, permissions).unwrap();
    assert_eq!(actual, expected);
    assert!(
        !reused,
        "a partial refs scan cannot authorize an answer without git"
    );
}
