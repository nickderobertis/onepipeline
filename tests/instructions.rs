//! The producer's own instruction, at both of the sites a consumer meets it.
//!
//! Both renderings are this crate's **own API** rather than a side effect of a
//! dispatch: `Node::rendered_task_with` composes the block a node's task carries,
//! and `instruction::arrival_note` composes the note a fast-adoption node is sent
//! when its releases arrive. Everything here drives those two, exactly as the
//! engine drives them.
//!
//! What the engine puts *into* them — which template each dependency resolved to,
//! and what version this run has been answered with — is driven end to end
//! against a real repository and a real probe by `tests/e2e/adoption.rs`.

use onepipeline::instruction::{arrival_note, InstructionTemplate, DEFAULT_INSTRUCTION, VARIABLES};
use onepipeline::plan::{
    CrossRepoReference, Node, CROSS_REPO_REFERENCES_HEADING, OBSERVED_STATE_FRAMING,
};

/// A template, or the refusal reading it earned.
fn template(source: &str) -> InstructionTemplate {
    InstructionTemplate::try_from(source.to_owned())
        .unwrap_or_else(|refused| panic!("`{source}` is not a template: {refused}"))
}

/// One dependency, with a **distinct** value in every cell, so a variable
/// rendering the wrong one is visible rather than plausible.
fn dependency(instruction: &str) -> CrossRepoReference {
    CrossRepoReference {
        dependency: "engine".into(),
        repository: "github.com/owner/engine".into(),
        branch: "onevcs/s-1".into(),
        commit: "9f3c1ab".into(),
        release_target: "crate".into(),
        version: "0.13.0".into(),
        instruction: template(instruction),
    }
}

/// The node whose task carries the block.
fn consumer() -> Node {
    Node {
        id: "consumer".into(),
        task: Some("## What\nship it".into()),
        ..Node::default()
    }
}

/// The block one rendered task carries, from its heading on.
fn block(task: &str) -> String {
    task.split_once(CROSS_REPO_REFERENCES_HEADING)
        .map(|(_, rest)| rest.to_owned())
        .unwrap_or_else(|| panic!("the rendered task carries no reference block:\n{task}"))
}

/// A template naming every variable renders every cell, at **both** sites.
///
/// One list, published as [`VARIABLES`], and both sites render from the same row
/// — so this fails if either site is handed something narrower than the other,
/// and it fails if this crate publishes a variable name nothing reads.
#[test]
fn every_variable_is_available_at_both_render_sites() {
    let naming_all: String = VARIABLES
        .iter()
        .map(|variable| format!("{variable}={{{{{variable}}}}}"))
        .collect::<Vec<String>>()
        .join(" ");
    let reference = dependency(&naming_all);
    let expected = "dependency=engine repository=github.com/owner/engine branch=onevcs/s-1 \
                    commit=9f3c1ab target=crate version=0.13.0";

    let task = consumer().rendered_task_with(std::slice::from_ref(&reference));
    assert!(
        block(&task).contains(expected),
        "the block did not render every variable:\n{task}"
    );

    let note = arrival_note(std::slice::from_ref(&reference));
    assert!(
        note.contains(expected),
        "the arrival note did not render every variable:\n{note}"
    );
}

/// A producer that declares no template is answered by the engine's own
/// instruction, at both sites and out of one place.
///
/// This is the whole of what a repository that has not adopted this sees, so it
/// is checked as the exact sentence rather than as a substring of one.
#[test]
fn a_producer_declaring_nothing_gets_the_engines_own_instruction_at_both_sites() {
    let undeclared = CrossRepoReference {
        instruction: InstructionTemplate::default(),
        ..dependency("anything at all")
    };
    assert_eq!(
        InstructionTemplate::default().as_str(),
        DEFAULT_INSTRUCTION,
        "the default is composed rather than stated in one place"
    );

    let task = consumer().rendered_task_with(std::slice::from_ref(&undeclared));
    assert!(
        block(&task).contains(&format!("\n{DEFAULT_INSTRUCTION}\n")),
        "the block carries no instruction for a producer that declared none:\n{task}"
    );
    assert!(
        arrival_note(std::slice::from_ref(&undeclared)).ends_with(DEFAULT_INSTRUCTION),
        "the note carries no instruction for a producer that declared none"
    );

    // Two dependencies, neither declaring anything: one sentence between them
    // rather than the same one twice.
    let note = arrival_note(&[undeclared.clone(), undeclared]);
    assert_eq!(note.matches(DEFAULT_INSTRUCTION).count(), 1, "{note}");
}

/// At a fast node's **first** render nothing has been released, and the block
/// says so without asserting a version.
///
/// The definition of fast adoption rather than a gap: the node is pinned against
/// git, and a template that guards on the version renders the branch that is
/// true there.
#[test]
fn a_block_rendered_before_any_release_asserts_no_version() {
    let guarded = "{{#version}}Pin {{repository}} at {{version}}.{{/version}}\
                   {{^version}}Pin {{repository}} at the branch {{branch}}.{{/version}}";
    let unreleased = CrossRepoReference {
        version: String::new(),
        ..dependency(guarded)
    };
    let pinned = block(&consumer().rendered_task_with(std::slice::from_ref(&unreleased)));
    assert!(
        pinned.contains("| engine | github.com/owner/engine | onevcs/s-1 | 9f3c1ab | crate |  |"),
        "the version cell asserts a version nothing has released:\n{pinned}"
    );
    assert!(
        pinned.contains("Pin against the git references below rather than against a version"),
        "the block does not say what a git pin is for:\n{pinned}"
    );
    assert!(
        pinned.contains("Pin github.com/owner/engine at the branch onevcs/s-1."),
        "the guarded template did not render its unreleased branch:\n{pinned}"
    );
    assert!(
        !pinned.contains("0.13.0"),
        "a version was asserted before one existed:\n{pinned}"
    );

    // The same template, once the release has arrived: the other branch, at both
    // sites, and the block now says the versions are what to pin against.
    let released = dependency(guarded);
    let arrived = block(&consumer().rendered_task_with(std::slice::from_ref(&released)));
    assert!(
        arrived.contains("Pin github.com/owner/engine at 0.13.0."),
        "{arrived}"
    );
    assert!(
        arrived.contains("The work this node depends on is released, at the versions below."),
        "{arrived}"
    );
    assert!(
        arrival_note(&[released]).contains("Pin github.com/owner/engine at 0.13.0."),
        "the note rendered the guard the other way"
    );
}

/// A template that renders to nothing renders **nothing**, rather than a blank
/// paragraph under the list or under the table.
#[test]
fn a_template_that_renders_to_nothing_leaves_no_paragraph_behind() {
    let silent = dependency("{{^version}}only before the release{{/version}}");
    let note = arrival_note(std::slice::from_ref(&silent));
    assert!(
        note.ends_with("- github.com/owner/engine — crate 0.13.0"),
        "{note}"
    );

    let task = consumer().rendered_task_with(std::slice::from_ref(&silent));
    assert!(block(&task).ends_with("| crate | 0.13.0 |\n"), "{task}");
}

/// The rendered instruction **adds no acceptance criterion**, at either site.
///
/// It is enclosed by the frame that says so: the sentence this crate publishes as
/// [`OBSERVED_STATE_FRAMING`] opens the section the instruction is rendered in,
/// and nothing between them starts a section of its own. A rendering that escaped
/// the frame — a note delivered on its own into a running turn, or an instruction
/// appended after the block — fails here.
#[test]
fn the_rendered_instruction_is_enclosed_by_the_observed_state_framing() {
    let instruction = "Bump the version in Cargo.toml and re-run `cargo update -p engine`.";
    let reference = dependency(instruction);

    for (site, rendered) in [
        (
            "the reference block",
            consumer().rendered_task_with(std::slice::from_ref(&reference)),
        ),
        (
            "the arrival note",
            arrival_note(std::slice::from_ref(&reference)),
        ),
    ] {
        let framed = rendered
            .find(OBSERVED_STATE_FRAMING)
            .unwrap_or_else(|| panic!("{site} states no observed-state frame:\n{rendered}"));
        let at = rendered
            .find(instruction)
            .unwrap_or_else(|| panic!("{site} did not render the instruction:\n{rendered}"));
        assert!(
            framed < at,
            "{site} renders the instruction outside the frame:\n{rendered}"
        );
        assert!(
            !rendered[framed..at].contains("\n## "),
            "{site} opens a section between the frame and the instruction:\n{rendered}"
        );
        assert!(
            !rendered[framed..at].to_lowercase().contains("criterion"),
            "{site} reads as a new bar:\n{rendered}"
        );
    }
}

/// A template is external input and is refused **by what is wrong with it**, so a
/// typo fails at the boundary rather than rendering as itself in a worker's task.
#[test]
fn a_template_is_refused_by_the_thing_that_is_wrong_with_it() {
    for (source, expected) in [
        ("Bump {{verison}}.", "names no variable `verison`"),
        ("Bump {{version}", "opens `{{` and never closes it"),
        (
            "{{#version}}out",
            "opens `{{#version}}` and never closes it",
        ),
        (
            "{{/version}}",
            "closes `{{/version}}`, which it never opened",
        ),
        (
            "{{#version}}{{/branch}}",
            "closes `{{/branch}}` where `version` is open",
        ),
        ("   ", "cannot be blank"),
        ("Bump\u{7} it.", "control character"),
    ] {
        let refused = InstructionTemplate::try_from(source.to_owned())
            .expect_err(&format!("`{source}` was read as a template"));
        assert!(
            refused.contains(expected),
            "`{source}` was refused with {refused:?}, which does not say {expected:?}"
        );
    }
    let long = "x".repeat(1_001);
    assert!(InstructionTemplate::try_from(long)
        .expect_err("a template past the bound")
        .contains("at most"));
}
