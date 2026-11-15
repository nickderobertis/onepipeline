//! The slug rules, the heading reading and each destination kind's section form, held to
//! the contract the README's "Section links" states.

use std::path::PathBuf;

use onetaskgraph_core::section_links::{
    Heading, LinearSectionForm, RecordHeadings, SectionForm, Written, github_slug, github_slugs,
    headings, linear_slug, relative_path, resolve_relative,
};
use onetaskgraph_plugin_api::StoredHeading;

/// The spike's five headings, as a body holds them.
const SPIKE: &str = "Intro.\n\n## What\n\nOne.\n\n## Budgets\n\nTwo.\n\n### Budget demo-quota-seconds\n\nThree.\n\n### Step 2.b: retry (once)\n\nFour.\n\n```md\n## Not a heading\n```\n\n## Budgets\n\nFive.\n";

fn heading(text: &str, ordinal: usize) -> Heading {
    Heading {
        text: text.to_owned(),
        ordinal,
    }
}

#[test]
fn headings_are_read_in_order_outside_fences() {
    assert_eq!(
        headings(SPIKE),
        [
            "What",
            "Budgets",
            "Budget demo-quota-seconds",
            "Step 2.b: retry (once)",
            "Budgets"
        ]
    );
    assert_eq!(
        headings("#not\n####### seven\n  ## Closed ##\n"),
        ["Closed"]
    );
}

#[test]
fn the_github_slug_rule_matches_the_authored_contract_repeats_included() {
    assert_eq!(github_slug("Step 2.b: retry (once)"), "step-2b-retry-once");
    assert_eq!(
        github_slug("Budget demo-quota-seconds"),
        "budget-demo-quota-seconds"
    );
    assert_eq!(github_slug("Under_score & A/B"), "under_score--ab");
    assert_eq!(
        github_slugs(&headings(SPIKE)),
        [
            "what",
            "budgets",
            "budget-demo-quota-seconds",
            "step-2b-retry-once",
            "budgets-1"
        ]
    );
}

#[test]
fn the_linear_slug_rule_matches_what_linear_stored() {
    // The one copied heading link, and the URL slugs Linear itself gave documents so titled.
    assert_eq!(
        linear_slug("Budget demo-quota-seconds"),
        "budget-demo-quota-seconds"
    );
    assert_eq!(linear_slug("Step 2.b: retry (once)"), "step-2b-retry-once");
    assert_eq!(
        linear_slug("Under_score & A/B  x--y"),
        "under-score-and-ab-x-y"
    );
    assert_eq!(linear_slug("Budgets"), "budgets");
}

#[test]
fn a_heading_is_found_by_its_slug_and_its_position_among_headings_of_its_text() {
    let held = RecordHeadings::of(SPIKE);
    assert_eq!(held.by_slug("budgets-1"), Some(heading("Budgets", 1)));
    assert_eq!(held.by_slug("budgets"), Some(heading("Budgets", 0)));
    assert_eq!(held.by_slug("nowhere"), None);
    assert_eq!(
        held.slug_of(&heading("Budgets", 1)).as_deref(),
        Some("budgets-1")
    );
}

#[test]
fn each_destination_writes_its_own_form() {
    let held = RecordHeadings::of(SPIKE);
    let second = heading("Budgets", 1);
    assert_eq!(
        SectionForm::LocalMd.cross(
            "/s/tasks/t.md",
            Some("/s/documents/d.md"),
            &second,
            &held,
            None
        ),
        Written::Link("../tasks/t.md#budgets-1".into())
    );
    assert_eq!(
        SectionForm::GithubProjects.cross(
            "https://github.com/o/r/issues/7",
            None,
            &second,
            &held,
            None
        ),
        Written::Link("https://www.github.com/o/r/issues/7#user-content-budgets-1".into())
    );
    assert_eq!(
        SectionForm::GithubProjects.same(
            Some("https://github.com/o/r/issues/7"),
            &second,
            &held,
            None
        ),
        Written::Link("https://www.github.com/o/r/issues/7#user-content-budgets-1".into())
    );
    assert_eq!(
        SectionForm::GithubProjects.same(None, &second, &held, None),
        Written::Pending,
        "a new issue's own number is not known until it is created"
    );
    let stored = vec![
        StoredHeading {
            text: "Budgets".into(),
            id: "75811e25-a438-45db-8383-3ccbbd7e16cc".into(),
        },
        StoredHeading {
            text: "Budgets".into(),
            id: "a15ed9c1-1094-42c9-806b-494ef7de1ee2".into(),
        },
    ];
    let linear = SectionForm::Linear(LinearSectionForm::HeadingLink);
    assert_eq!(
        linear.same(
            Some("https://linear.app/w/issue/T-1"),
            &second,
            &held,
            Some(&[])
        ),
        Written::Unaddressable("#budgets-1".into()),
        "a missing same-record heading id preserves the authored slug",
    );
    assert_eq!(
        linear.cross(
            "https://linear.app/w/issue/T-1",
            None,
            &second,
            &held,
            Some(&stored)
        ),
        Written::Link("https://linear.app/w/issue/T-1#heading-budgets-a15ed9c1".into())
    );
    assert_eq!(
        linear.cross("https://linear.app/w/issue/T-1", None, &second, &held, None),
        Written::Pending
    );
    assert_eq!(
        linear.cross(
            "https://linear.app/w/issue/T-1",
            None,
            &heading("What", 0),
            &held,
            Some(&stored)
        ),
        Written::Unaddressable("https://linear.app/w/issue/T-1".into())
    );
    // A Linear link is read back by its id alone, whatever its slug says.
    assert_eq!(
        linear.heading_of("heading-wrong-slug-a15ed9c1", &held, Some(&stored)),
        Some(second.clone())
    );
    assert_eq!(
        SectionForm::GithubProjects.heading_of("user-content-budgets-1", &held, None),
        Some(second)
    );
}

#[test]
fn github_anchors_are_written_before_each_heading_and_stripped_on_reading() {
    let markup = SectionForm::GithubProjects
        .markup()
        .expect("GitHub needs anchors");
    let written = markup.written(SPIKE);
    assert!(written.contains("<a id=\"budgets-1\"></a>\n## Budgets\n\nFive."));
    assert!(written.contains("<a id=\"what\"></a>\n## What\n"));
    assert!(!written.contains("<a id=\"not-a-heading\""));
    assert_eq!(markup.read(&written), SPIKE);
    assert!(
        SectionForm::Linear(LinearSectionForm::HeadingLink)
            .markup()
            .is_none()
    );
}

#[test]
fn relative_paths_are_written_and_read_from_the_linking_file() {
    assert_eq!(relative_path("/a/b/d.md", "/a/b/t.md"), "t.md");
    assert_eq!(
        relative_path("/a/documents/d.md", "/a/tasks/p/t.md"),
        "../tasks/p/t.md"
    );
    // A resolved path is spelled with the platform's separator, so it is held to the file it
    // names, compared as a path, rather than to `/`-separated bytes.
    let resolved = |from: &str, target: &str| resolve_relative(from, target).map(PathBuf::from);
    assert_eq!(
        resolved("/a/documents/d.md", "../tasks/p/t.md"),
        Some(PathBuf::from("/a/tasks/p/t.md"))
    );
    assert_eq!(
        resolved("/a/documents/d.md", "./my%20file.md"),
        Some(PathBuf::from("/a/documents/my file.md"))
    );
    assert_ne!(
        resolved("/a/documents/d.md", "../tasks/p/t.md"),
        Some(PathBuf::from("/a/tasks/t.md"))
    );
    assert_eq!(resolve_relative("/a/d.md", "https://x/y"), None);
    assert_eq!(resolve_relative("/a/d.md", "#slug"), None);
}

#[test]
fn an_anchor_a_person_wrote_and_one_in_a_fenced_example_survive_reading() {
    let markup = SectionForm::GithubProjects
        .markup()
        .expect("GitHub needs anchors");
    let authored = "<a id=\"budget-table\"></a>\n## Budgets\n\nThe table.\n\n```md\n<a id=\"example\"></a>\n## Example\n```\n\n## Done\n";
    let written = markup.written(authored);
    assert!(written.contains("<a id=\"budget-table\"></a>\n<a id=\"budgets\"></a>\n## Budgets"));
    assert!(written.contains("<a id=\"done\"></a>\n## Done"));
    assert_eq!(
        markup.read(&written),
        authored,
        "only the written anchors are taken away"
    );
    assert_eq!(
        markup.read(authored),
        authored,
        "an authored body reads as it is"
    );
}

/// The budget table a design document is authored with, as a copy to Linear writes it.
#[test]
fn every_reference_style_spelling_is_written_inline_and_its_definition_left_out() {
    use onetaskgraph_core::section_links::inline_reference_links as inlined;
    for (authored, wanted) in [
        (
            "| Budget | Limit |\n| --- | --- |\n| [one][1] | 2 |\n| [two][2] | 3 |\n\n\
             [1]: ../tasks/t.md#budget-one\n[2]: ../tasks/t.md#budget-two\n",
            "| Budget | Limit |\n| --- | --- |\n| [one](../tasks/t.md#budget-one) | 2 |\n\
             | [two](../tasks/t.md#budget-two) | 3 |\n",
        ),
        // The collapsed and the shortcut spelling, a label in another case, and one use of many.
        (
            "[Name][] and [name] and [other][NAME].\n\n[name]: https://a.example/b\n",
            "[Name](https://a.example/b) and [name](https://a.example/b) and \
             [other](https://a.example/b).\n",
        ),
        // Mid-body, one blank line stays; a title is left out, angle brackets stay where needed.
        (
            "See [a][a].\n\n[a]: <x (1).md> \"A title\"\n\nThen more.\n",
            "See [a](<x (1).md>).\n\nThen more.\n",
        ),
    ] {
        assert_eq!(inlined(authored), wanted, "{authored:?}");
        assert_eq!(inlined(wanted), wanted, "inlining {wanted:?} again");
    }
    for kept in [
        // No definition, an inline link, and text that only looks like one.
        "[a][b] and [c] and [d](e.md) and `[f][g]`.\n",
        "- [x] done\n\n[y]: is not a definition of anything here.\n",
        "A note.[^1]\n\n[^1]: The footnote.\n",
        // A definition nothing uses is no link, and is left as it was written.
        "Text.\n\n[unused]: x.md\n",
        // Code holds nothing, and an image reference keeps what it uses.
        "`[a][1]`\n\n```\n[a][1]\n[1]: x.md\n```\n",
        "![alt][1]\n\n[1]: https://a.example/i.png\n",
    ] {
        assert_eq!(inlined(kept), kept, "{kept:?}");
    }
    // An image and a link sharing a definition: the link is inlined, and the definition stays.
    assert_eq!(
        inlined("![alt][1] and [link][1]\n\n[1]: a.png\n"),
        "![alt][1] and [link](a.png)\n\n[1]: a.png\n"
    );
}
