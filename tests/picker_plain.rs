//! Fixtures for the picker's plain-language sentences (SPEC-jev-picker v2 §3):
//! every recipe `plain` phrase and every rendered reason template must pass
//! A0's checker with the shipped lists, and the templates here must be the
//! ones `src/launch.rs` renders.
//!
//! The checker and its lists are A0's real ones, compiled into this test with
//! `#[path]`; the sentences are the spec's day-one phrases and templates.

#[allow(dead_code)]
#[path = "../src/contracts.rs"]
mod contracts;
#[allow(dead_code)]
#[path = "../src/plain.rs"]
mod plain;

use plain::{Glossary, check};

/// The five templates, exactly as `src/launch.rs` writes them.
const PICKED: &str = "{job} looks like {clause}, so it runs on {plain}.";
const DEFAULT: &str = "{job} looks like ordinary work, so it runs on {plain}.";
const PINNED: &str = "You chose {plain} for {job}.";
const FALLBACK: &str =
    "{job} runs on {plain}, the usual choice, because the picker did not answer.";
const SHADOW: &str = "{job} runs on {plain}; the picker would have chosen {pick}.";

/// The spec's day-one recipe phrases.
const PHRASES: [&str; 6] = [
    "the usual coding helper",
    "the web research helper",
    "the strongest design helper",
    "the second opinion helper",
    "the careful number helper",
    "the hardest problem helper",
];

/// The job noun of each role (SPEC-jev-picker v2 §3).
const JOBS: [&str; 5] = [
    "this task",
    "this review",
    "this second opinion",
    "this draft",
    "this lookup",
];

/// The reason clause of each shipped non-default recipe.
const CLAUSES: [&str; 5] = [
    "design or spec work",
    "number or engine work",
    "the hardest number work",
    "web research",
    "a second opinion on a draft",
];

fn render(template: &str, job: &str, clause: &str, plain: &str, pick: &str) -> String {
    template
        .replace("{job}", job)
        .replace("{clause}", clause)
        .replace("{plain}", plain)
        .replace("{pick}", pick)
}

fn assert_plain(sentence: &str) {
    let result = check(sentence, &Glossary::default());
    assert!(result.passed(), "{sentence}: {:?}", result.violations);
}

#[test]
fn the_templates_in_this_fixture_are_the_ones_launch_writes() {
    let source = include_str!("../src/launch.rs");
    for template in [PICKED, DEFAULT, PINNED, FALLBACK, SHADOW] {
        assert!(
            source.contains(template),
            "src/launch.rs no longer contains `{template}`"
        );
    }
    for job in JOBS {
        assert!(
            source.contains(&format!("\"{job}\"")),
            "src/launch.rs no longer contains the job noun `{job}`"
        );
    }
    for clause in CLAUSES {
        assert!(
            source.contains(&format!("\"{clause}\"")),
            "src/launch.rs no longer contains the clause `{clause}`"
        );
    }
}

#[test]
fn every_recipe_phrase_passes_as_a_birth_sentence() {
    for phrase in PHRASES {
        assert_plain(phrase);
    }
}

#[test]
fn every_rendered_reason_passes_as_a_birth_sentence() {
    for job in JOBS {
        for plain in PHRASES {
            assert_plain(&render(DEFAULT, job, "", plain, ""));
            assert_plain(&render(PINNED, job, "", plain, ""));
            assert_plain(&render(FALLBACK, job, "", plain, ""));
            for pick in PHRASES {
                assert_plain(&render(SHADOW, job, "", plain, pick));
            }
        }
        for clause in CLAUSES {
            for plain in PHRASES {
                assert_plain(&render(PICKED, job, clause, plain, ""));
            }
        }
    }
}

#[test]
fn the_compact_reason_fits_eighty_characters() {
    for job in JOBS {
        for plain in PHRASES {
            let compact = format!("{job} runs on {plain}");
            assert!(
                compact.len() <= 80,
                "{compact} is {} characters",
                compact.len()
            );
            assert_plain(&compact);
        }
    }
}

#[test]
fn a_failing_fixture_still_fails() {
    // The test would pass vacuously if the checker accepted anything.
    let result = check("the biorhythm helper", &Glossary::default());
    assert!(!result.passed());
}
