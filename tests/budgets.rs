//! `budgets.yaml` and the command behind its one budget, held to the checker that reads them:
//! `onebudgetspec-core`, the library the `onebudgetspec` binary is built from, at the release
//! `justfile` installs that binary at.
//!
//! The command runs a whole e2e journey, which `just budgets` measures; what is held here is
//! everything around that journey — the file loading as the budget it states, the result the
//! journey writes being one the checker parses as a value, and the command refusing to report a
//! measurement it did not take.

#[cfg(unix)]
#[path = "e2e/budget_result.rs"]
mod budget_result;

use std::path::PathBuf;

use onebudgetspec_core::{Direction, Measure};
#[cfg(unix)]
use onebudgetspec_core::{Selection, Verdict};

const BUDGET: &str = "linear-requests-per-writeback-settlement";

const JOURNEY: &str = "linear_writeback::consecutive_linear_settlements_each_land_their_own_status_and_metadata_in_their_own_attempt";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[cfg(unix)]
fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("onepipeline-budgets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// The file loads under the pinned checker, and registers the budget as it is meant: the
/// requests a Linear settlement write-back sends, reported by its command, at most two.
#[test]
fn the_budgets_file_registers_the_linear_writeback_budget_for_the_pinned_checker() {
    let loaded = onebudgetspec_core::load(&[root().join("budgets.yaml")], false)
        .unwrap_or_else(|error| panic!("budgets.yaml does not load: {error}"));
    let budget = loaded
        .files()
        .iter()
        .flat_map(|file| &file.contents.budgets)
        .find(|budget| budget.id == BUDGET)
        .unwrap_or_else(|| panic!("budgets.yaml registers no {BUDGET}"));
    assert_eq!(budget.measure, Measure::Reported);
    assert_eq!(budget.unit, "requests");
    assert_eq!(budget.direction, Direction::Max);
    assert!(
        (budget.threshold - 2.0).abs() < f64::EPSILON,
        "{}",
        budget.threshold
    );
    assert_eq!(
        budget.command,
        ["bash", "scripts/linear-writeback-budget.sh"],
        "the budget's command is not the committed script"
    );
    let description = budget.description.as_deref().unwrap_or_default();
    assert!(
        description.contains("production Linear key") && description.contains("no check may reach"),
        "{description}"
    );
}

/// The checker library linked here is the release `just budgets` installs, so what this file
/// proves of it is true of the binary that measures.
#[test]
fn the_linked_checker_is_the_release_the_justfile_installs() {
    let justfile = std::fs::read_to_string(root().join("justfile")).expect("the justfile reads");
    let installed = justfile
        .lines()
        .find_map(|line| line.strip_prefix("onebudgetspec-version := "))
        .map(|version| version.trim().trim_matches('"').to_owned())
        .expect("the justfile names the onebudgetspec release it installs");
    let lock = std::fs::read_to_string(root().join("Cargo.lock")).expect("the lock reads");
    let linked = lock
        .split("[[package]]")
        .find(|package| package.contains("name = \"onebudgetspec-core\""))
        .and_then(|package| {
            package
                .lines()
                .find_map(|line| line.strip_prefix("version = "))
                .map(|version| version.trim_matches('"').to_owned())
        })
        .expect("the lock resolves onebudgetspec-core");
    assert_eq!(installed, linked);
}

/// What the journey writes is a value to the checker: measured through the library's own
/// `check`, over a budget whose command writes exactly the result the journey builds. That
/// command is a POSIX shell line, as the budget's own is, so it runs where a shell does.
#[cfg(unix)]
#[test]
fn the_result_the_journey_writes_is_a_value_the_checker_measures() {
    let dir = scratch("result");
    let written = budget_result::reported(3, "three requests for the second attempt").to_string();
    std::fs::write(dir.join("result.json"), &written).expect("the result is staged");
    std::fs::write(
        dir.join("budgets.yaml"),
        "schema_version: 1\nbudgets:\n  - id: staged\n    measure: reported\n    \
         command: [\"sh\", \"-c\", \"cat result.json > \\\"$ONEBUDGETSPEC_RESULT\\\"\"]\n    \
         unit: requests\n    direction: max\n    threshold: 2\n",
    )
    .expect("a budgets file");
    let loaded = onebudgetspec_core::load(&[dir.join("budgets.yaml")], false)
        .unwrap_or_else(|error| panic!("the staged budgets file does not load: {error}"));
    let report = loaded
        .select(&Selection::default())
        .expect("every budget is selected")
        .check();
    let [result] = report.results.as_slice() else {
        panic!("one budget, one result: {:?}", report.results);
    };
    assert_eq!(result.error, None, "the checker could not read {written}");
    assert_eq!(result.actual, Some(3.0));
    assert_eq!(result.verdict, Verdict::Over);
    assert_eq!(
        result.detail.as_deref(),
        Some("three requests for the second attempt")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The command, run with `just` standing in for the journey it hands to: `behaviour` is the
/// stand-in's shell body, and `result` the file the command is told to write to, if any.
#[cfg(unix)]
fn command(name: &str, behaviour: &str, result: Option<&std::path::Path>) -> std::process::Output {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch(name);
    let just = dir.join("just");
    std::fs::write(&just, format!("#!/bin/sh\n{behaviour}\n")).expect("a stand-in just");
    std::fs::set_permissions(&just, std::fs::Permissions::from_mode(0o755))
        .expect("the stand-in is executable");
    let path = std::env::join_paths(std::iter::once(dir.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("a PATH");
    let mut run = std::process::Command::new("bash");
    run.arg(root().join("scripts/linear-writeback-budget.sh"))
        .current_dir(root())
        .env("PATH", path)
        .env_remove("ONEBUDGETSPEC_RESULT");
    if let Some(result) = result {
        run.env("ONEBUDGETSPEC_RESULT", result);
    }
    run.output().expect("the budget command runs")
}

/// Run outside the checker, the command refuses before it runs anything; run by it, a journey
/// that fails, or passes and writes nothing, fails the command rather than reporting a value it
/// never measured, and one that writes its result passes.
#[cfg(unix)]
#[test]
fn the_budget_command_reports_only_a_measurement_its_journey_took() {
    let stderr =
        |output: &std::process::Output| String::from_utf8_lossy(&output.stderr).into_owned();

    let unset = command("unset", "echo ran >&2; exit 0", None);
    assert_eq!(unset.status.code(), Some(2), "{}", stderr(&unset));
    assert!(
        stderr(&unset).contains("ONEBUDGETSPEC_RESULT is not set"),
        "{}",
        stderr(&unset)
    );
    assert!(
        !stderr(&unset).contains("ran"),
        "it ran the journey with nowhere to write"
    );

    let dir = scratch("results");
    let failing = command("failing", "exit 4", Some(&dir.join("failing.json")));
    assert_eq!(failing.status.code(), Some(4), "{}", stderr(&failing));

    let silent = dir.join("silent.json");
    std::fs::write(&silent, "").expect("a fresh, empty result file");
    let silent = command("silent", "exit 0", Some(&silent));
    assert_eq!(silent.status.code(), Some(1), "{}", stderr(&silent));
    assert!(
        stderr(&silent).contains("wrote no result"),
        "{}",
        stderr(&silent)
    );

    // The stand-in writes a result only when it was asked for exactly the journey that measures
    // this budget, through the recipe that runs e2e journeys, with the result file reaching it.
    let measured = dir.join("measured.json");
    std::fs::write(&measured, "").expect("a fresh, empty result file");
    let wrote = command(
        "wrote",
        &format!(
            "[ \"$1\" = test-e2e ] && [ \"$2\" = 'test(={JOURNEY})' ] && [ $# -eq 2 ] \\\n  \
             && printf '{{\"value\": 2}}' > \"$ONEBUDGETSPEC_RESULT\""
        ),
        Some(&measured),
    );
    assert_eq!(wrote.status.code(), Some(0), "{}", stderr(&wrote));
    assert_eq!(
        std::fs::read_to_string(&measured).expect("the result reads"),
        r#"{"value": 2}"#
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The journey the command asks for is one the e2e binary has: a filter naming a test that is
/// not there would select nothing.
#[test]
fn the_budget_command_names_a_journey_the_e2e_binary_holds() {
    let journeys = std::fs::read_to_string(root().join("tests/e2e/linear_writeback.rs"))
        .expect("the journeys read");
    let (module, name) = JOURNEY
        .split_once("::")
        .expect("a module-qualified journey");
    assert_eq!(module, "linear_writeback");
    assert!(
        journeys.contains(&format!("#[test]\nfn {name}()")),
        "tests/e2e/linear_writeback.rs holds no journey {name}"
    );
    let script = std::fs::read_to_string(root().join("scripts/linear-writeback-budget.sh"))
        .expect("the command reads");
    assert!(
        script.contains(&format!("journey='{JOURNEY}'")),
        "the command runs another journey than {JOURNEY}"
    );
}
