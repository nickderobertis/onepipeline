//! `Engine::create_plan` as a Rust caller links it: the plan graph the command line's
//! `project create` makes from a template's `members`, here built by hand — so the engine's own
//! refusals of a graph that does not hold together are what is under test, not the template's.

use std::collections::BTreeMap;

use onetaskgraph_core::{
    Body, Config, Engine, EngineError, GlobalId, PlanMember, ProjectCreate, ProjectSelector,
    TaskRequest,
};
use onetaskgraph_plugin_api::{NativeId, SecretResolver, SourceName};
use secrecy::SecretString;
use serde_json::json;

struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

fn engine(root: &std::path::Path) -> Engine {
    let config = Config::from_document(json!({
        "sources": {"notes": {"plugin": "local-md", "config": {"root": root}}}
    }))
    .expect("a valid configuration");
    Engine::build(&config, &NoSecrets)
}

fn project() -> ProjectCreate {
    ProjectCreate {
        source: SourceName::new("notes").expect("a name"),
        id: NativeId::from("plan-1"),
        title: "The plan".to_owned(),
        body: Body::plain("The plan.\n"),
        status: None,
        labels: None,
        repositories: None,
        metadata: BTreeMap::new(),
        classification: None,
    }
}

fn member(id: &str, depends_on: &[&str]) -> PlanMember {
    PlanMember {
        id: NativeId::from(id),
        title: format!("Member {id}"),
        body: Body::plain(format!("{id}\n")),
        depends_on: depends_on.iter().map(|id| NativeId::from(*id)).collect(),
        metadata: BTreeMap::new(),
        repositories: Vec::new(),
    }
}

fn files(root: &std::path::Path) -> usize {
    walk(root)
}

fn walk(directory: &std::path::Path) -> usize {
    std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| {
                    let path = entry.path();
                    if path.is_dir() { walk(&path) } else { 1 }
                })
                .sum()
        })
        .unwrap_or(0)
}

#[tokio::test]
async fn a_member_named_twice_or_depending_on_a_later_one_is_refused_before_anything_is_written() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = engine(root.path());
    for (members, says) in [
        (
            vec![member("a", &[]), member("a", &[])],
            "name the member a twice",
        ),
        (
            vec![member("a", &["b"]), member("b", &[])],
            "depends on b, which is not a member created before it",
        ),
    ] {
        let refused = engine
            .create_plan(&project(), &members)
            .await
            .expect_err("a graph that does not hold together");
        assert!(
            matches!(refused, EngineError::Template { .. }),
            "{refused:?}"
        );
        assert!(refused.to_string().contains(says), "{refused}");
        assert_eq!(files(root.path()), 0, "nothing written");
    }
}

#[tokio::test]
async fn a_plan_lands_the_project_then_each_member_with_its_edges() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = engine(root.path());
    let created = engine
        .create_plan(&project(), &[member("a", &[]), member("b", &["a"])])
        .await
        .expect("a plan");
    let notes = |native: &str| GlobalId::new(SourceName::new("notes").unwrap(), native.into());
    assert_eq!(created.project, notes("plan-1"));
    assert_eq!(
        created.members,
        vec![
            (NativeId::from("a"), notes("a")),
            (NativeId::from("b"), notes("b"))
        ]
    );
    let listed = engine
        .tasks(&TaskRequest {
            sources: Vec::new(),
            filters: onetaskgraph_core::Filters::default(),
            project: ProjectSelector::Qualified(notes("plan-1")),
            priorities: Vec::new(),
            commented_since: None,
            metadata: Vec::new(),
            origin: None,
            include_members: false,
            paging: onetaskgraph_core::Paging {
                limit: std::num::NonZeroU32::new(50).expect("nonzero"),
                token: None,
            },
        })
        .await
        .expect("a listing");
    assert_eq!(listed.items.len(), 2);
}
