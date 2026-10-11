#![cfg(all(feature = "std", feature = "utf16"))]
use core::ops::Range;
use regress::{CompileBudget, CompileError, Flags, Regex};

fn compile(pattern: &str, flags: Flags, bounded: bool) -> Result<Regex, CompileError> {
    if bounded {
        let mut budget = CompileBudget::new(100_000_000, 1_000_000_000, 128);
        Regex::from_unicode_with_limits(pattern.chars().map(u32::from), flags, &mut budget)
    } else {
        Regex::with_flags(pattern, flags).map_err(CompileError::Syntax)
    }
}

fn verify(
    pattern: &str,
    flags: &str,
    text: &str,
    syntax_error: bool,
    expected: Option<Vec<Option<Range<usize>>>>,
    named: Option<Range<usize>>,
) {
    for no_opt in [false, true] {
        for bounded in [false, true] {
            let mut flags = Flags::from(flags);
            flags.no_opt = no_opt;
            let control = compile("a", flags, bounded).unwrap();
            assert_eq!(control.find("a").unwrap().range(), 0..1);
            let result = compile(pattern, flags, bounded);
            if syntax_error {
                assert!(
                    matches!(result, Err(CompileError::Syntax(_))),
                    "{pattern}: {result:?}"
                );
                continue;
            }
            let regex = result.unwrap();
            let text: Vec<_> = text.encode_utf16().collect();
            for limited_match in [false, true] {
                let mut budget = regress::MatchBudget::new(1_000_000);
                let actual = match (flags.unicode, limited_match) {
                    (true, true) => regex
                        .find_from_utf16_with_budget(&text, 0, &mut budget)
                        .unwrap(),
                    (false, true) => regex
                        .find_from_ucs2_with_budget(&text, 0, &mut budget)
                        .unwrap(),
                    (true, false) => regex.find_from_utf16(&text, 0).next(),
                    (false, false) => regex.find_from_ucs2(&text, 0).next(),
                };
                assert_eq!(
                    actual.as_ref().map(|m| m.groups().collect::<Vec<_>>()),
                    expected,
                    "{pattern}, no_opt={no_opt}, bounded={bounded}, limited_match={limited_match}"
                );
                assert_eq!(
                    actual.as_ref().and_then(|m| m.named_group("x")),
                    named,
                    "{pattern}"
                );
            }
        }
    }
}

#[test]
fn second() {
    verify(
        "(?:(?<x>aa)|(?<x>ab))\\k<x>",
        "",
        "abab",
        false,
        Some(vec![Some(0..4), None, Some(0..2)]),
        Some(0..2),
    );
}

#[test]
fn first() {
    verify(
        "(?:(?<x>aa)|(?<x>ab))\\k<x>",
        "",
        "aaaa",
        false,
        Some(vec![Some(0..4), Some(0..2), None]),
        Some(0..2),
    );
}

#[test]
fn reversed() {
    verify(
        "(?:(?<x>b)|(?<x>a))\\k<x>",
        "",
        "aa",
        false,
        Some(vec![Some(0..2), None, Some(0..1)]),
        Some(0..1),
    );
}

#[test]
fn mismatch() {
    verify("^(?:(?<x>a)|(?<x>b))\\k<x>$", "", "ab", false, None, None);
}

#[test]
fn unmatched() {
    verify(
        "^(?:(?<x>a)|(?<x>b))?\\k<x>$",
        "",
        "",
        false,
        Some(vec![Some(0..0), None, None]),
        None,
    );
}

#[test]
fn repeated_second() {
    verify(
        "^(?:(?<x>a)|(?<x>b))+\\k<x>$",
        "",
        "abb",
        false,
        Some(vec![Some(0..3), None, Some(1..2)]),
        Some(1..2),
    );
}

#[test]
fn repeated_first() {
    verify(
        "^(?:(?<x>a)|(?<x>b))+\\k<x>$",
        "",
        "baa",
        false,
        Some(vec![Some(0..3), Some(1..2), None]),
        Some(1..2),
    );
}

#[test]
fn repeated_mismatch() {
    verify("^(?:(?<x>a)|(?<x>b))+\\k<x>$", "", "aba", false, None, None);
}

#[test]
fn repeat_empty() {
    verify(
        "^(?:(?<x>a)|(?<x>b))*\\k<x>$",
        "",
        "",
        false,
        Some(vec![Some(0..0), None, None]),
        None,
    );
}

#[test]
fn ahead() {
    verify(
        "(?=(?:(?<x>a)|(?<x>b)))\\k<x>",
        "",
        "b",
        false,
        Some(vec![Some(0..1), None, Some(0..1)]),
        Some(0..1),
    );
}

#[test]
fn negative_ahead() {
    verify(
        "(?!(?:(?<x>a)|(?<x>b)))\\k<x>c",
        "",
        "c",
        false,
        Some(vec![Some(0..1), None, None]),
        None,
    );
}

#[test]
fn behind() {
    verify(
        "(?<=(?:(?<x>a)|(?<x>b)))\\k<x>",
        "",
        "bb",
        false,
        Some(vec![Some(1..2), None, Some(0..1)]),
        Some(0..1),
    );
}

#[test]
fn forward() {
    verify(
        "^\\k<x>(?:(?<x>a)|(?<x>b))$",
        "",
        "b",
        false,
        Some(vec![Some(0..1), None, Some(0..1)]),
        Some(0..1),
    );
}

#[test]
fn forward_branches() {
    verify(
        "^(?:\\k<x>(?<x>a)|\\k<x>(?<x>b))$",
        "",
        "b",
        false,
        Some(vec![Some(0..1), None, Some(0..1)]),
        Some(0..1),
    );
}

#[test]
fn legacy_case() {
    verify(
        "(?:(?<x>a)|(?<x>b))\\k<x>",
        "i",
        "bB",
        false,
        Some(vec![Some(0..2), None, Some(0..1)]),
        Some(0..1),
    );
}

#[test]
fn unicode_case() {
    verify(
        "(?:(?<x>σ)|(?<x>α))\\k<x>",
        "ui",
        "Αα",
        false,
        Some(vec![Some(0..2), None, Some(0..1)]),
        Some(0..1),
    );
}

#[test]
fn invalid_sequence() {
    verify("(?<x>a)(?<x>b)", "", "", true, None, None);
}

#[test]
fn invalid_shared_path() {
    verify("(?:(?<x>a)|b)(?<x>c)", "", "", true, None, None);
}

#[test]
fn invalid_inner_path() {
    verify("(?<x>a)|(?:(?<x>b)(?<x>c))", "", "", true, None, None);
}
