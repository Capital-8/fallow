#![cfg(all(feature = "std", feature = "utf16"))]

use regress::{CompileBudget, CompileError, CompileLimit, Regex};

#[test]
fn compile_work_exhaustion_is_explicit_and_sticky() {
    let mut budget = CompileBudget::new(2, 1_000_000, 100);
    let result = Regex::from_unicode_with_limits("abc".chars().map(u32::from), "", &mut budget);
    assert!(
        matches!(result, Err(CompileError::Limit(CompileLimit::Work))),
        "{result:?}"
    );
    assert!(matches!(
        Regex::from_unicode_with_limits("a".chars().map(u32::from), "", &mut budget),
        Err(CompileError::Limit(CompileLimit::Work))
    ));
}

fn assert_derived_storage_refused(pattern: &str) {
    let mut control = CompileBudget::new(1_000_000, 1_000_000, 100);
    let ordinary =
        Regex::from_unicode_with_limits("abcdefghij".chars().map(u32::from), "u", &mut control)
            .unwrap();
    assert_eq!(ordinary.find("abcdefghij").unwrap().range(), 0..10);
    let mut budget = CompileBudget::new(1_000_000, 4_096, 100);
    let result = Regex::from_unicode_with_limits(pattern.chars().map(u32::from), "u", &mut budget);
    assert!(
        matches!(result, Err(CompileError::Limit(CompileLimit::Allocation))),
        "{pattern}: {result:?}"
    );
}

#[test]
fn compile_property_storage_is_refused_before_materialization() {
    assert_derived_storage_refused(r"\p{Letter}");
}

#[test]
fn compile_unroll_storage_is_refused_before_materialization() {
    assert_derived_storage_refused("(?:(?:abcdef){4}){4}");
}

#[test]
fn compile_input_refuses_before_reading_without_work() {
    use std::{cell::Cell, rc::Rc};
    let reads = Rc::new(Cell::new(0));
    let observed = Rc::clone(&reads);
    let input = "x".chars().map(move |ch| {
        observed.set(observed.get() + 1);
        u32::from(ch)
    });
    let mut budget = CompileBudget::new(0, 1_000_000, 100);
    let result = Regex::from_unicode_with_limits(input, "", &mut budget);
    assert!(matches!(
        result,
        Err(CompileError::Limit(CompileLimit::Work))
    ));
    assert_eq!(reads.get(), 0, "the denied input read must not execute");

    let mut control = CompileBudget::new(1_000_000, 1_000_000, 100);
    let input = "x".chars().map(|ch| {
        reads.set(reads.get() + 1);
        u32::from(ch)
    });
    let regex = Regex::from_unicode_with_limits(input, "", &mut control).unwrap();
    assert_eq!(reads.get(), 1);
    assert_eq!(regex.find("x").unwrap().range(), 0..1);
}

#[test]
fn derived_alternation_depth_is_bounded_without_parentheses() {
    let pattern = vec!["a"; 100].join("|");
    let mut budget = CompileBudget::new(1_000_000, 1_000_000, 8);
    let result = Regex::from_unicode_with_limits(pattern.chars().map(u32::from), "", &mut budget);
    assert!(
        matches!(result, Err(CompileError::Limit(CompileLimit::Depth))),
        "{result:?}"
    );
}

#[test]
fn caller_cannot_disable_derived_depth_ceiling() {
    for no_opt in [false, true] {
        let flags = regress::Flags {
            no_opt,
            ..regress::Flags::default()
        };
        let pattern = vec!["a"; 512].join("|");
        let mut budget = CompileBudget::new(100_000_000, 1_000_000_000, usize::MAX);
        let result =
            Regex::from_unicode_with_limits(pattern.chars().map(u32::from), flags, &mut budget);
        assert!(matches!(
            result,
            Err(CompileError::Limit(CompileLimit::Depth))
        ));
        assert!(matches!(
            Regex::from_unicode_with_limits("a".chars().map(u32::from), flags, &mut budget),
            Err(CompileError::Limit(CompileLimit::Depth))
        ));
    }
}

#[test]
fn bounded_parser_preserves_named_alternative_priority_and_lookbehind() {
    for no_opt in [false, true] {
        for (pattern, flags, text) in [
            (r"(?:(?<x>a)|(?<x>b))\k<x>", "", "bb"),
            (r"(?<=ab)(c|cd)", "", "abcd"),
            (r"[a\q{bc|bd}]", "v", "bd"),
        ] {
            let mut flags = regress::Flags::from(flags);
            flags.no_opt = no_opt;
            let mut budget = CompileBudget::new(100_000_000, 1_000_000_000, 1_000);
            let bounded =
                Regex::from_unicode_with_limits(pattern.chars().map(u32::from), flags, &mut budget)
                    .unwrap();
            let ordinary = Regex::with_flags(pattern, flags).unwrap();
            assert_eq!(
                format!("{:?}", bounded.find(text)),
                format!("{:?}", ordinary.find(text))
            );
        }
    }
}

#[test]
fn existing_parser_cap_and_syntax_errors_are_preserved() {
    for pattern in [
        "[".to_string(),
        format!("{}a{}", "(?:".repeat(300), ")".repeat(300)),
    ] {
        let mut budget = CompileBudget::new(100_000_000, 1_000_000_000, 1_000);
        let result =
            Regex::from_unicode_with_limits(pattern.chars().map(u32::from), "", &mut budget);
        assert!(matches!(result, Err(CompileError::Syntax(_))), "{result:?}");
        assert_eq!(budget.exhausted(), None);
    }
}

#[test]
fn ordinary_bounded_compilation_preserves_matching() {
    for (pattern, flags, text) in [
        (r"(?<=api/)(?<part>.+)$", "", "api/name"),
        (r"^\p{Script=Greek}+$", "u", "Σα"),
        (r"^(a+)\1$", "", "aaaa"),
    ] {
        let mut budget = CompileBudget::new(100_000_000, 1_000_000_000, 1_000);
        let bounded =
            Regex::from_unicode_with_limits(pattern.chars().map(u32::from), flags, &mut budget)
                .unwrap();
        let ordinary = Regex::with_flags(pattern, flags).unwrap();
        let text: Vec<_> = text.encode_utf16().collect();
        assert_eq!(
            format!("{:?}", bounded.find_from_utf16(&text, 0).next()),
            format!("{:?}", ordinary.find_from_utf16(&text, 0).next())
        );
    }
}

#[test]
fn compile_unicode_case_expansion_obeys_shared_work_limit() {
    for no_opt in [false, true] {
        let mut flags = regress::Flags::from("ui");
        flags.no_opt = no_opt;
        let mut control = CompileBudget::new(10_000, 8_000_000, 128);
        let simple =
            Regex::from_unicode_with_limits("[a]".chars().map(u32::from), flags, &mut control)
                .unwrap();
        assert_eq!(simple.find("A").unwrap().range(), 0..1);
        let mut budget = CompileBudget::new(10_000, 8_000_000, 128);
        let result = Regex::from_unicode_with_limits(
            r"[\u0000-\u{10ffff}]".chars().map(u32::from),
            flags,
            &mut budget,
        );
        assert!(
            matches!(result, Err(CompileError::Limit(CompileLimit::Work))),
            "case expansion did not refuse derived work"
        );
        assert!(matches!(
            Regex::from_unicode_with_limits("a".chars().map(u32::from), flags, &mut budget),
            Err(CompileError::Limit(CompileLimit::Work))
        ));
    }
}

#[test]
fn compile_unicode_string_property_storage_is_bounded() {
    for no_opt in [false, true] {
        let mut flags = regress::Flags::from("v");
        flags.no_opt = no_opt;
        let mut control = CompileBudget::new(100_000_000, 4_096, 128);
        let small = Regex::from_unicode_with_limits(
            r"[\q{ab|a}]".chars().map(u32::from),
            flags,
            &mut control,
        )
        .unwrap();
        assert_eq!(small.find("ab").unwrap().range(), 0..2);
        let mut budget = CompileBudget::new(100_000_000, 4_096, 128);
        let result = Regex::from_unicode_with_limits(
            r"[\p{RGI_Emoji}]".chars().map(u32::from),
            flags,
            &mut budget,
        );
        assert!(
            matches!(result, Err(CompileError::Limit(CompileLimit::Allocation))),
            "static string payload was not refused before copying"
        );
    }
}

#[test]
fn bounded_compilation_preserves_group_and_loop_caps() {
    let flags = regress::Flags {
        no_opt: true,
        ..regress::Flags::default()
    };
    for (atom, message) in [
        ("()", "Capture group count limit exceeded"),
        ("a?", "Loop count limit exceeded"),
    ] {
        let allowed = atom.repeat(65_535);
        let mut budget = CompileBudget::new(1_000_000_000, 1_000_000_000, 128);
        let regex =
            Regex::from_unicode_with_limits(allowed.chars().map(u32::from), flags, &mut budget)
                .unwrap();
        assert_eq!(regex.find("").unwrap().range(), 0..0);
        assert_eq!(budget.exhausted(), None);
        let denied = atom.repeat(65_536);
        let mut budget = CompileBudget::new(1_000_000_000, 1_000_000_000, 128);
        let error =
            Regex::from_unicode_with_limits(denied.chars().map(u32::from), flags, &mut budget)
                .unwrap_err();
        match error {
            CompileError::Syntax(error) => assert_eq!(error.to_string(), message),
            other => panic!("expected original syntax cap, got {other:?}"),
        }
        assert_eq!(budget.exhausted(), None);
    }
}

#[test]
fn compiler_boundary_matrix_retains_semantics_and_sticky_refusal() {
    // Literal expectations are retained in compiler-boundary-node-modern-results.json.
    type Case<'a> = (
        &'a str,
        &'a str,
        &'a str,
        Option<Vec<Option<core::ops::Range<usize>>>>,
    );
    let cases: Vec<Case<'_>> = vec![
        (
            "^(a+)(a)$",
            "",
            "aaaa",
            Some(vec![Some(0..4), Some(0..3), Some(3..4)]),
        ),
        (
            "^(a+?)(a+)$",
            "",
            "aaaa",
            Some(vec![Some(0..4), Some(0..1), Some(1..4)]),
        ),
        (
            "(?<=ab)(c|cd)",
            "",
            "abcd",
            Some(vec![Some(2..3), Some(2..3)]),
        ),
        (
            "^(?!b)(a|ab)+$",
            "",
            "ababa",
            Some(vec![Some(0..5), Some(4..5)]),
        ),
        (
            "[[\\q{ab|ac|a}]&&[\\q{ac|ab}]]",
            "v",
            "ab",
            Some(vec![Some(0..2)]),
        ),
        (
            "[[\\q{ab|ac|a}]--[\\q{ac}]]",
            "v",
            "ac",
            Some(vec![Some(0..1)]),
        ),
        ("^\\p{Script=Greek}+$", "ui", "Σα", Some(vec![Some(0..2)])),
        ("a{123x}", "", "a{123x}", Some(vec![Some(0..7)])),
        ("a{123x}", "u", "", None),
        ("(?<bad", "", "", None),
    ];
    for no_opt in [false, true] {
        for (pattern, flags, text, expected) in &cases {
            let mut flags = regress::Flags::from(*flags);
            flags.no_opt = no_opt;
            let text: Vec<_> = text.encode_utf16().collect();
            for resource in [CompileLimit::Work, CompileLimit::Allocation] {
                let mut saw_refusal = false;
                let mut saw_semantic_result = false;
                for allowance in [
                    0,
                    1,
                    2,
                    8,
                    32,
                    128,
                    512,
                    1_024,
                    4_096,
                    16_384,
                    65_536,
                    100_000_000,
                ] {
                    let (work, bytes) = match resource {
                        CompileLimit::Work => (allowance, 1_000_000_000),
                        CompileLimit::Allocation => (100_000_000, allowance),
                        CompileLimit::Depth => unreachable!(),
                    };
                    let mut budget = CompileBudget::new(work, bytes, 128);
                    match Regex::from_unicode_with_limits(
                        pattern.chars().map(u32::from),
                        flags,
                        &mut budget,
                    ) {
                        Err(CompileError::Limit(limit)) => {
                            assert_eq!(limit, resource, "{pattern}");
                            assert!(
                                matches!(Regex::from_unicode_with_limits("a".chars().map(u32::from), flags, &mut budget), Err(CompileError::Limit(again)) if again == limit)
                            );
                            saw_refusal = true;
                        }
                        Err(CompileError::Syntax(_)) => {
                            assert_eq!(expected, &None, "{pattern}");
                            assert_eq!(budget.exhausted(), None);
                            saw_semantic_result = true;
                        }
                        Ok(regex) => {
                            let mut match_budget = regress::MatchBudget::new(1_000_000);
                            let result = if flags.unicode {
                                regex
                                    .find_from_utf16_with_budget(&text, 0, &mut match_budget)
                                    .unwrap()
                            } else {
                                regex
                                    .find_from_ucs2_with_budget(&text, 0, &mut match_budget)
                                    .unwrap()
                            };
                            let ranges = result.map(|matched| matched.groups().collect::<Vec<_>>());
                            assert_eq!(&ranges, expected, "{pattern}");
                            assert_eq!(budget.exhausted(), None);
                            saw_semantic_result = true;
                        }
                    }
                }
                assert!(saw_refusal, "missing refusal control for {pattern}");
                assert!(
                    saw_semantic_result,
                    "missing normal semantic control for {pattern}"
                );
            }
        }
    }
}

#[test]
fn duplicate_named_backreference_retains_modern_node_match_range() {
    let pattern = r"(?:(?<part>aa)|(?<part>ab))\k<part>";
    let mut budget = CompileBudget::new(100_000_000, 1_000_000_000, 128);
    let regex =
        Regex::from_unicode_with_limits(pattern.chars().map(u32::from), "", &mut budget).unwrap();
    let text: Vec<_> = "abab".encode_utf16().collect();
    let actual = regex.find_from_ucs2(&text, 0).next().unwrap();
    assert_eq!(
        actual.groups().collect::<Vec<_>>(),
        vec![Some(0..4), None, Some(0..2)]
    );
}
