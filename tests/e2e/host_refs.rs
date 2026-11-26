//! What the `fake-gh` double reports about a branch is what its origin holds at the moment
//! it is asked, however git keeps that branch's ref.
//!
//! The double reads a change request's head, and whether a base exists, off the bare
//! origin's ref files rather than by starting git for each question. These journeys hold
//! those reads to git's own answers across each way the files hold a ref — a loose file,
//! `packed-refs` alone, a loose file newer than its packed line, a symbolic ref, and a ref
//! deleted — asking the compiled double through the `gh` argv `onevcs` itself passes.

use crate::harness::{double, git, World};
use serde_json::Value;
use std::path::Path;

/// The double's answer for a head it cannot see.
const PLACEHOLDER: &str = "0123456789abcdef0123456789abcdef01234567";

/// Run the compiled double with `args` in `world`, answering its exit status and stdout.
fn gh(world: &World, args: &[&str]) -> (bool, String) {
    let output = world
        .cmd_on(&double("fake-gh"), args)
        .output()
        .expect("the host double runs");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

/// The head the double reports for change request `number`.
fn reported_head(world: &World, number: &str) -> String {
    let (ok, stdout) = gh(
        world,
        &[
            "pr",
            "view",
            number,
            "--repo",
            "owner/service",
            "--json",
            "headRefOid",
        ],
    );
    assert!(ok, "the host refused to view {number}: {stdout}");
    let view: Value = serde_json::from_str(&stdout).expect("the host answers JSON");
    view["headRefOid"]
        .as_str()
        .expect("a head is reported")
        .to_owned()
}

/// Open a change request from `head` into `main`.
fn open(world: &World, head: &str) {
    let (ok, stdout) = gh(
        world,
        &[
            "pr",
            "create",
            "--repo",
            "owner/service",
            "--head",
            head,
            "--base",
            "main",
            "--title",
            "feat: carry the change",
            "--body",
            "The change.",
        ],
    );
    assert!(ok, "the host refused to open {head}: {stdout}");
}

/// Commit in `checkout` and push it to `branch` on its origin, answering the commit.
fn pushed(world: &World, checkout: &Path, branch: &str, subject: &str) -> String {
    git(world, checkout, &["commit", "--allow-empty", "-m", subject]);
    git(
        world,
        checkout,
        &[
            "push",
            "--quiet",
            "origin",
            &format!("HEAD:refs/heads/{branch}"),
        ],
    );
    git(world, checkout, &["rev-parse", "HEAD"])
        .trim()
        .to_owned()
}

#[test]
fn a_change_requests_head_is_its_branchs_tip_however_the_origin_holds_the_ref() {
    let world = World::new("host-refs-head");
    let repository = world.repository("local-direct", &[]);
    let origin = &repository.origin;
    let first = pushed(&world, &repository.checkout, "feature", "feat: one");
    open(&world, "feature");
    assert!(origin.join("refs/heads/feature").is_file());
    assert_eq!(reported_head(&world, "1"), first, "a loose ref");

    git(&world, origin, &["pack-refs", "--all"]);
    assert!(
        !origin.join("refs/heads/feature").exists(),
        "packing left the loose ref in place, so the packed read is unproven"
    );
    assert_eq!(reported_head(&world, "1"), first, "a packed ref alone");

    let second = pushed(&world, &repository.checkout, "feature", "feat: two");
    let packed = std::fs::read_to_string(origin.join("packed-refs")).expect("packed-refs");
    assert!(packed.contains(&format!("{first} refs/heads/feature")));
    assert_eq!(
        reported_head(&world, "1"),
        second,
        "a loose ref newer than the packed line it shadows"
    );

    git(
        &world,
        origin,
        &["symbolic-ref", "refs/heads/alias", "refs/heads/feature"],
    );
    open(&world, "alias");
    assert_eq!(reported_head(&world, "2"), second, "a symbolic ref");

    git(
        &world,
        &repository.checkout,
        &["push", "--quiet", "origin", "--delete", "feature"],
    );
    assert_eq!(reported_head(&world, "1"), PLACEHOLDER, "a deleted ref");
    assert_eq!(
        reported_head(&world, "2"),
        PLACEHOLDER,
        "a symbolic ref to a deleted one"
    );
}

#[test]
fn a_retarget_is_refused_onto_a_base_the_origin_does_not_hold_and_taken_onto_a_packed_one() {
    let world = World::new("host-refs-base");
    let repository = world.repository("local-direct", &[]);
    pushed(&world, &repository.checkout, "feature", "feat: one");
    open(&world, "feature");
    let retarget = |base: &str| {
        gh(
            &world,
            &[
                "api",
                "repos/owner/service/pulls/1",
                "--method",
                "PATCH",
                "-f",
                &format!("base={base}"),
            ],
        )
        .0
    };
    assert!(!retarget("stack"), "a base the origin does not hold");

    pushed(&world, &repository.checkout, "stack", "feat: under it");
    git(&world, &repository.origin, &["pack-refs", "--all"]);
    assert!(!repository.origin.join("refs/heads/stack").exists());
    assert!(retarget("stack"), "a base held only in packed-refs");
}
