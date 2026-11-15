//! Measurement-only public-API equivalence journey; no existing test is changed.
use privateterms::{
    Action, LocatorKind, Matcher, ResolvedRule, TermMode, TermRule, STARTUP_CONTRIBUTORS,
    STARTUP_CUT_ENV,
};

fn matcher(cut: bool, mode: TermMode, case_sensitive: bool) -> Matcher {
    if cut {
        std::env::set_var(STARTUP_CUT_ENV, STARTUP_CONTRIBUTORS);
    } else {
        std::env::set_var(STARTUP_CUT_ENV, "baseline");
    }
    Matcher::new(
        ["secret", "σ", "café", "İ", "owner/repo", "ﬃ"]
            .into_iter()
            .enumerate()
            .map(|(i, term)| ResolvedRule {
                rule: TermRule {
                    term: term.into(),
                    mode,
                    case_sensitive,
                },
                action: if i % 2 == 0 {
                    Action::Replace("public".into())
                } else {
                    Action::Reject
                },
                origin: "caller".into(),
            })
            .collect(),
        &["public secret".into()],
    )
    .expect("valid rules")
}

fn main() {
    std::env::set_var(STARTUP_CUT_ENV, "unsupported");
    assert!(
        Matcher::new(Vec::new(), &[]).is_err(),
        "an unknown cut must be refused"
    );
    let baseline = matcher(false, TermMode::Substring, false);
    std::env::remove_var(STARTUP_CUT_ENV);
    let default = Matcher::new(baseline.rules().to_vec(), &["public secret".into()])
        .expect("the unset selector constructs a matcher");
    assert_eq!(
        default.replace("ＳＥＣＲＥＴ"),
        baseline.replace("ＳＥＣＲＥＴ")
    );
    let mut checks = 0;
    for mode in [
        TermMode::Substring,
        TermMode::WholeWord,
        TermMode::OwnerNameOnly,
    ] {
        for sensitive in [false, true] {
            let before = matcher(false, mode, sensitive);
            let after = matcher(true, mode, sensitive);
            let texts = [
                "SECRET",
                "a secret here",
                "public secret",
                "prefixsecret",
                "σ ς Σ",
                "café cafe\u{301}",
                "İ i\u{307}",
                "owner/repo",
                "ﬃ ffi",
                "ＳＥＣＲＥＴ",
                "se\u{200b}cret",
                "nothing private",
            ];
            for text in texts.into_iter().map(str::to_owned).chain(
                (0..=0x10ffff)
                    .step_by(1024)
                    .filter_map(char::from_u32)
                    .map(|c| format!("{c} secret {c}")),
            ) {
                assert_eq!(
                    before.find(&text),
                    after.find(&text),
                    "find {mode:?} {sensitive} {text:?}"
                );
                assert_eq!(
                    before.hits(&text),
                    after.hits(&text),
                    "hits {mode:?} {sensitive} {text:?}"
                );
                assert_eq!(
                    before.replace(&text),
                    after.replace(&text),
                    "replace {mode:?} {sensitive} {text:?}"
                );
                assert_eq!(
                    before.screen_locators(&[(LocatorKind::Path, &text)], 1),
                    after.screen_locators(&[(LocatorKind::Path, &text)], 1),
                    "locators {mode:?} {sensitive} {text:?}"
                );
                checks += 1;
            }
            assert!(!before.find("nothing private").contains(&0));
            if mode == TermMode::Substring && !sensitive {
                assert_eq!(after.replace("ＳＥＣＲＥＴ").text, "public");
                assert!(after.find("public secret").is_empty());
                assert!(!after.replace("Σ").remaining.is_empty());
            }
        }
    }
    println!("contributors bypass: {checks} inputs have identical find, hits, replace and locator results across all modes and case policies");
}
