#![cfg(all(feature = "std", feature = "utf16"))]
use regress::{CompileBudget, Flags, MatchBudget, Regex};
use std::ops::Range;

// Expectations follow pinned TC39 folding/complement semantics.
// Node and JavaScriptCore results, including Node q discrepancies, are saved separately.
fn check(pattern: &str, flags: &str, text: &str, expected: Option<Vec<Option<Range<usize>>>>) {
    let text: Vec<_> = text.encode_utf16().collect();
    for no_opt in [false, true] {
        for bounded in [false, true] {
            let mut flags = Flags::from(flags);
            flags.no_opt = no_opt;
            let regex = if bounded {
                let mut budget = CompileBudget::new(100_000_000, 1_000_000_000, 128);
                Regex::from_unicode_with_limits(pattern.chars().map(u32::from), flags, &mut budget)
                    .unwrap()
            } else {
                Regex::with_flags(pattern, flags).unwrap()
            };
            let actual = regex
                .find_from_utf16(&text, 0)
                .next()
                .map(|m| m.groups().collect::<Vec<_>>());
            assert_eq!(
                actual, expected,
                "pattern={pattern} text={text:?} no_opt={no_opt} bounded={bounded}"
            );
            let mut budget = MatchBudget::new(1_000_000);
            let actual = regex
                .find_from_utf16_with_budget(&text, 0, &mut budget)
                .unwrap()
                .map(|m| m.groups().collect::<Vec<_>>());
            assert_eq!(
                actual, expected,
                "bounded match pattern={pattern} text={text:?} no_opt={no_opt} bounded={bounded}"
            );
        }
    }
}
#[test]
fn nested_case_00() {
    check(r###"[[^\q{a}]]"###, r###"v"###, r###""###, None);
    check(r###"[[^\q{a}]]"###, r###"v"###, r###"a"###, None);
    check(
        r###"[[^\q{a}]]"###,
        r###"v"###,
        r###"A"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[^\q{a}]]"###,
        r###"v"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[^\q{a}]]"###,
        r###"v"###,
        r###"B"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[^\q{a}]]"###,
        r###"v"###,
        r###"aA"###,
        Some(vec![Some(1..2)]),
    );
    check(
        r###"[[^\q{a}]]"###,
        r###"v"###,
        r###"Aa"###,
        Some(vec![Some(0..1)]),
    );
}

#[test]
fn nested_case_01() {
    check(r###"[[^\q{a}]]"###, r###"vi"###, r###""###, None);
    check(r###"[[^\q{a}]]"###, r###"vi"###, r###"a"###, None);
    check(r###"[[^\q{a}]]"###, r###"vi"###, r###"A"###, None);
    check(
        r###"[[^\q{a}]]"###,
        r###"vi"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[^\q{a}]]"###,
        r###"vi"###,
        r###"B"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[[^\q{a}]]"###, r###"vi"###, r###"aA"###, None);
    check(r###"[[^\q{a}]]"###, r###"vi"###, r###"Aa"###, None);
}

#[test]
fn nested_case_02() {
    check(r###"[[^a]]"###, r###"v"###, r###""###, None);
    check(r###"[[^a]]"###, r###"v"###, r###"a"###, None);
    check(
        r###"[[^a]]"###,
        r###"v"###,
        r###"A"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[^a]]"###,
        r###"v"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[^a]]"###,
        r###"v"###,
        r###"B"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[^a]]"###,
        r###"v"###,
        r###"aA"###,
        Some(vec![Some(1..2)]),
    );
    check(
        r###"[[^a]]"###,
        r###"v"###,
        r###"Aa"###,
        Some(vec![Some(0..1)]),
    );
}

#[test]
fn nested_case_03() {
    check(r###"[[^a]]"###, r###"vi"###, r###""###, None);
    check(r###"[[^a]]"###, r###"vi"###, r###"a"###, None);
    check(r###"[[^a]]"###, r###"vi"###, r###"A"###, None);
    check(
        r###"[[^a]]"###,
        r###"vi"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[^a]]"###,
        r###"vi"###,
        r###"B"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[[^a]]"###, r###"vi"###, r###"aA"###, None);
    check(r###"[[^a]]"###, r###"vi"###, r###"Aa"###, None);
}

#[test]
fn nested_case_04() {
    check(r###"[^\q{a}]"###, r###"v"###, r###""###, None);
    check(r###"[^\q{a}]"###, r###"v"###, r###"a"###, None);
    check(
        r###"[^\q{a}]"###,
        r###"v"###,
        r###"A"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^\q{a}]"###,
        r###"v"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^\q{a}]"###,
        r###"v"###,
        r###"B"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^\q{a}]"###,
        r###"v"###,
        r###"aA"###,
        Some(vec![Some(1..2)]),
    );
    check(
        r###"[^\q{a}]"###,
        r###"v"###,
        r###"Aa"###,
        Some(vec![Some(0..1)]),
    );
}

#[test]
fn nested_case_05() {
    check(r###"[^\q{a}]"###, r###"vi"###, r###""###, None);
    check(r###"[^\q{a}]"###, r###"vi"###, r###"a"###, None);
    check(r###"[^\q{a}]"###, r###"vi"###, r###"A"###, None);
    check(
        r###"[^\q{a}]"###,
        r###"vi"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^\q{a}]"###,
        r###"vi"###,
        r###"B"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[^\q{a}]"###, r###"vi"###, r###"aA"###, None);
    check(r###"[^\q{a}]"###, r###"vi"###, r###"Aa"###, None);
}

#[test]
fn nested_case_06() {
    check(r###"[^a]"###, r###"v"###, r###""###, None);
    check(r###"[^a]"###, r###"v"###, r###"a"###, None);
    check(
        r###"[^a]"###,
        r###"v"###,
        r###"A"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^a]"###,
        r###"v"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^a]"###,
        r###"v"###,
        r###"B"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^a]"###,
        r###"v"###,
        r###"aA"###,
        Some(vec![Some(1..2)]),
    );
    check(
        r###"[^a]"###,
        r###"v"###,
        r###"Aa"###,
        Some(vec![Some(0..1)]),
    );
}

#[test]
fn nested_case_07() {
    check(r###"[^a]"###, r###"vi"###, r###""###, None);
    check(r###"[^a]"###, r###"vi"###, r###"a"###, None);
    check(r###"[^a]"###, r###"vi"###, r###"A"###, None);
    check(
        r###"[^a]"###,
        r###"vi"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^a]"###,
        r###"vi"###,
        r###"B"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[^a]"###, r###"vi"###, r###"aA"###, None);
    check(r###"[^a]"###, r###"vi"###, r###"Aa"###, None);
}

#[test]
fn nested_case_08() {
    check(r###"[[\q{a}]]"###, r###"v"###, r###""###, None);
    check(
        r###"[[\q{a}]]"###,
        r###"v"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[[\q{a}]]"###, r###"v"###, r###"A"###, None);
    check(r###"[[\q{a}]]"###, r###"v"###, r###"b"###, None);
    check(r###"[[\q{a}]]"###, r###"v"###, r###"B"###, None);
    check(
        r###"[[\q{a}]]"###,
        r###"v"###,
        r###"aA"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[\q{a}]]"###,
        r###"v"###,
        r###"Aa"###,
        Some(vec![Some(1..2)]),
    );
}

#[test]
fn nested_case_09() {
    check(r###"[[\q{a}]]"###, r###"vi"###, r###""###, None);
    check(
        r###"[[\q{a}]]"###,
        r###"vi"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[\q{a}]]"###,
        r###"vi"###,
        r###"A"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[[\q{a}]]"###, r###"vi"###, r###"b"###, None);
    check(r###"[[\q{a}]]"###, r###"vi"###, r###"B"###, None);
    check(
        r###"[[\q{a}]]"###,
        r###"vi"###,
        r###"aA"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[\q{a}]]"###,
        r###"vi"###,
        r###"Aa"###,
        Some(vec![Some(0..1)]),
    );
}

#[test]
fn nested_case_10() {
    check(r###"[[a]]"###, r###"v"###, r###""###, None);
    check(
        r###"[[a]]"###,
        r###"v"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[[a]]"###, r###"v"###, r###"A"###, None);
    check(r###"[[a]]"###, r###"v"###, r###"b"###, None);
    check(r###"[[a]]"###, r###"v"###, r###"B"###, None);
    check(
        r###"[[a]]"###,
        r###"v"###,
        r###"aA"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[a]]"###,
        r###"v"###,
        r###"Aa"###,
        Some(vec![Some(1..2)]),
    );
}

#[test]
fn nested_case_11() {
    check(r###"[[a]]"###, r###"vi"###, r###""###, None);
    check(
        r###"[[a]]"###,
        r###"vi"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[a]]"###,
        r###"vi"###,
        r###"A"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[[a]]"###, r###"vi"###, r###"b"###, None);
    check(r###"[[a]]"###, r###"vi"###, r###"B"###, None);
    check(
        r###"[[a]]"###,
        r###"vi"###,
        r###"aA"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[a]]"###,
        r###"vi"###,
        r###"Aa"###,
        Some(vec![Some(0..1)]),
    );
}

#[test]
fn nested_case_12() {
    check(r###"[^[^\q{a}]&&[A]]"###, r###"v"###, r###""###, None);
    check(
        r###"[^[^\q{a}]&&[A]]"###,
        r###"v"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[^[^\q{a}]&&[A]]"###, r###"v"###, r###"A"###, None);
    check(
        r###"[^[^\q{a}]&&[A]]"###,
        r###"v"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^\q{a}]&&[A]]"###,
        r###"v"###,
        r###"B"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^\q{a}]&&[A]]"###,
        r###"v"###,
        r###"aA"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^\q{a}]&&[A]]"###,
        r###"v"###,
        r###"Aa"###,
        Some(vec![Some(1..2)]),
    );
}

#[test]
fn nested_case_13() {
    check(r###"[^[^\q{a}]&&[A]]"###, r###"vi"###, r###""###, None);
    check(
        r###"[^[^\q{a}]&&[A]]"###,
        r###"vi"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^\q{a}]&&[A]]"###,
        r###"vi"###,
        r###"A"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^\q{a}]&&[A]]"###,
        r###"vi"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^\q{a}]&&[A]]"###,
        r###"vi"###,
        r###"B"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^\q{a}]&&[A]]"###,
        r###"vi"###,
        r###"aA"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^\q{a}]&&[A]]"###,
        r###"vi"###,
        r###"Aa"###,
        Some(vec![Some(0..1)]),
    );
}

#[test]
fn nested_case_14() {
    check(r###"[^[^a]&&[A]]"###, r###"v"###, r###""###, None);
    check(
        r###"[^[^a]&&[A]]"###,
        r###"v"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[^[^a]&&[A]]"###, r###"v"###, r###"A"###, None);
    check(
        r###"[^[^a]&&[A]]"###,
        r###"v"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^a]&&[A]]"###,
        r###"v"###,
        r###"B"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^a]&&[A]]"###,
        r###"v"###,
        r###"aA"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^a]&&[A]]"###,
        r###"v"###,
        r###"Aa"###,
        Some(vec![Some(1..2)]),
    );
}

#[test]
fn nested_case_15() {
    check(r###"[^[^a]&&[A]]"###, r###"vi"###, r###""###, None);
    check(
        r###"[^[^a]&&[A]]"###,
        r###"vi"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^a]&&[A]]"###,
        r###"vi"###,
        r###"A"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^a]&&[A]]"###,
        r###"vi"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^a]&&[A]]"###,
        r###"vi"###,
        r###"B"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^a]&&[A]]"###,
        r###"vi"###,
        r###"aA"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^[^a]&&[A]]"###,
        r###"vi"###,
        r###"Aa"###,
        Some(vec![Some(0..1)]),
    );
}
