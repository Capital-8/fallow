#![cfg(all(feature = "std", feature = "utf16"))]
use regress::{CompileBudget, CompileError, Flags, MatchBudget, Regex};
use std::ops::Range;

// Expected ranges come from the separately saved pinned Node ampersand oracle.
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
fn syntax(pattern: &str, flags: &str) {
    for no_opt in [false, true] {
        let mut flags = Flags::from(flags);
        flags.no_opt = no_opt;
        assert!(Regex::with_flags(pattern, flags).is_err(), "{pattern}");
        let mut budget = CompileBudget::new(100_000_000, 1_000_000_000, 128);
        assert!(
            matches!(
                Regex::from_unicode_with_limits(pattern.chars().map(u32::from), flags, &mut budget),
                Err(CompileError::Syntax(_))
            ),
            "{pattern}"
        );
    }
}

#[test]
fn first_character() {
    check(
        r###"[a&]"###,
        r###"v"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a&]"###, r###"v"###, r###"A"###, None);
    check(
        r###"[a&]"###,
        r###"v"###,
        r###"&"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a&]"###, r###"v"###, r###"b"###, None);
    check(r###"[a&]"###, r###"v"###, r###"c"###, None);
    check(r###"[a&]"###, r###"v"###, r###"1"###, None);
    check(
        r###"[a&]"###,
        r###"v"###,
        r###"ab"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a&]"###, r###"v"###, r###"cd"###, None);
    check(r###"[a&]"###, r###"v"###, r###"!"###, None);
    check(r###"[a&]"###, r###"v"###, r###"z"###, None);
}

#[test]
fn legacy_u_control() {
    check(
        r###"[a&]"###,
        r###"u"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a&]"###, r###"u"###, r###"A"###, None);
    check(
        r###"[a&]"###,
        r###"u"###,
        r###"&"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a&]"###, r###"u"###, r###"b"###, None);
    check(r###"[a&]"###, r###"u"###, r###"c"###, None);
    check(r###"[a&]"###, r###"u"###, r###"1"###, None);
    check(
        r###"[a&]"###,
        r###"u"###,
        r###"ab"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a&]"###, r###"u"###, r###"cd"###, None);
    check(r###"[a&]"###, r###"u"###, r###"!"###, None);
    check(r###"[a&]"###, r###"u"###, r###"z"###, None);
}

#[test]
fn later_union_operands() {
    check(
        r###"[a&bc]"###,
        r###"v"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a&bc]"###, r###"v"###, r###"A"###, None);
    check(
        r###"[a&bc]"###,
        r###"v"###,
        r###"&"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[a&bc]"###,
        r###"v"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[a&bc]"###,
        r###"v"###,
        r###"c"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a&bc]"###, r###"v"###, r###"1"###, None);
    check(
        r###"[a&bc]"###,
        r###"v"###,
        r###"ab"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[a&bc]"###,
        r###"v"###,
        r###"cd"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a&bc]"###, r###"v"###, r###"!"###, None);
    check(r###"[a&bc]"###, r###"v"###, r###"z"###, None);
}

#[test]
fn nested_first_set() {
    check(
        r###"[[ab]&]"###,
        r###"v"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[[ab]&]"###, r###"v"###, r###"A"###, None);
    check(
        r###"[[ab]&]"###,
        r###"v"###,
        r###"&"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[[ab]&]"###,
        r###"v"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[[ab]&]"###, r###"v"###, r###"c"###, None);
    check(r###"[[ab]&]"###, r###"v"###, r###"1"###, None);
    check(
        r###"[[ab]&]"###,
        r###"v"###,
        r###"ab"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[[ab]&]"###, r###"v"###, r###"cd"###, None);
    check(r###"[[ab]&]"###, r###"v"###, r###"!"###, None);
    check(r###"[[ab]&]"###, r###"v"###, r###"z"###, None);
}

#[test]
fn class_escape_first() {
    check(r###"[\d&]"###, r###"v"###, r###"a"###, None);
    check(r###"[\d&]"###, r###"v"###, r###"A"###, None);
    check(
        r###"[\d&]"###,
        r###"v"###,
        r###"&"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[\d&]"###, r###"v"###, r###"b"###, None);
    check(r###"[\d&]"###, r###"v"###, r###"c"###, None);
    check(
        r###"[\d&]"###,
        r###"v"###,
        r###"1"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[\d&]"###, r###"v"###, r###"ab"###, None);
    check(r###"[\d&]"###, r###"v"###, r###"cd"###, None);
    check(r###"[\d&]"###, r###"v"###, r###"!"###, None);
    check(r###"[\d&]"###, r###"v"###, r###"z"###, None);
}

#[test]
fn property_first() {
    check(
        r###"[\p{ASCII}&]"###,
        r###"v"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[\p{ASCII}&]"###,
        r###"v"###,
        r###"A"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[\p{ASCII}&]"###,
        r###"v"###,
        r###"&"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[\p{ASCII}&]"###,
        r###"v"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[\p{ASCII}&]"###,
        r###"v"###,
        r###"c"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[\p{ASCII}&]"###,
        r###"v"###,
        r###"1"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[\p{ASCII}&]"###,
        r###"v"###,
        r###"ab"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[\p{ASCII}&]"###,
        r###"v"###,
        r###"cd"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[\p{ASCII}&]"###,
        r###"v"###,
        r###"!"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[\p{ASCII}&]"###,
        r###"v"###,
        r###"z"###,
        Some(vec![Some(0..1)]),
    );
}

#[test]
fn string_first() {
    check(r###"[\q{ab|cd}&]"###, r###"v"###, r###"a"###, None);
    check(r###"[\q{ab|cd}&]"###, r###"v"###, r###"A"###, None);
    check(
        r###"[\q{ab|cd}&]"###,
        r###"v"###,
        r###"&"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[\q{ab|cd}&]"###, r###"v"###, r###"b"###, None);
    check(r###"[\q{ab|cd}&]"###, r###"v"###, r###"c"###, None);
    check(r###"[\q{ab|cd}&]"###, r###"v"###, r###"1"###, None);
    check(
        r###"[\q{ab|cd}&]"###,
        r###"v"###,
        r###"ab"###,
        Some(vec![Some(0..2)]),
    );
    check(
        r###"[\q{ab|cd}&]"###,
        r###"v"###,
        r###"cd"###,
        Some(vec![Some(0..2)]),
    );
    check(r###"[\q{ab|cd}&]"###, r###"v"###, r###"!"###, None);
    check(r###"[\q{ab|cd}&]"###, r###"v"###, r###"z"###, None);
}

#[test]
fn negated_class() {
    check(r###"[^a&]"###, r###"v"###, r###"a"###, None);
    check(
        r###"[^a&]"###,
        r###"v"###,
        r###"A"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[^a&]"###, r###"v"###, r###"&"###, None);
    check(
        r###"[^a&]"###,
        r###"v"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^a&]"###,
        r###"v"###,
        r###"c"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^a&]"###,
        r###"v"###,
        r###"1"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^a&]"###,
        r###"v"###,
        r###"ab"###,
        Some(vec![Some(1..2)]),
    );
    check(
        r###"[^a&]"###,
        r###"v"###,
        r###"cd"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^a&]"###,
        r###"v"###,
        r###"!"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[^a&]"###,
        r###"v"###,
        r###"z"###,
        Some(vec![Some(0..1)]),
    );
}

#[test]
fn disjoint_intersection_control() {
    check(r###"[a&&b]"###, r###"v"###, r###"a"###, None);
    check(r###"[a&&b]"###, r###"v"###, r###"A"###, None);
    check(r###"[a&&b]"###, r###"v"###, r###"&"###, None);
    check(r###"[a&&b]"###, r###"v"###, r###"b"###, None);
    check(r###"[a&&b]"###, r###"v"###, r###"c"###, None);
    check(r###"[a&&b]"###, r###"v"###, r###"1"###, None);
    check(r###"[a&&b]"###, r###"v"###, r###"ab"###, None);
    check(r###"[a&&b]"###, r###"v"###, r###"cd"###, None);
    check(r###"[a&&b]"###, r###"v"###, r###"!"###, None);
    check(r###"[a&&b]"###, r###"v"###, r###"z"###, None);
}

#[test]
fn intersection_control() {
    check(r###"[[ab]&&[bc]]"###, r###"v"###, r###"a"###, None);
    check(r###"[[ab]&&[bc]]"###, r###"v"###, r###"A"###, None);
    check(r###"[[ab]&&[bc]]"###, r###"v"###, r###"&"###, None);
    check(
        r###"[[ab]&&[bc]]"###,
        r###"v"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[[ab]&&[bc]]"###, r###"v"###, r###"c"###, None);
    check(r###"[[ab]&&[bc]]"###, r###"v"###, r###"1"###, None);
    check(
        r###"[[ab]&&[bc]]"###,
        r###"v"###,
        r###"ab"###,
        Some(vec![Some(1..2)]),
    );
    check(r###"[[ab]&&[bc]]"###, r###"v"###, r###"cd"###, None);
    check(r###"[[ab]&&[bc]]"###, r###"v"###, r###"!"###, None);
    check(r###"[[ab]&&[bc]]"###, r###"v"###, r###"z"###, None);
}

#[test]
fn missing_intersection_operand() {
    syntax(r###"[a&&]"###, r###"v"###);
}

#[test]
fn mixed_operator_syntax() {
    syntax(r###"[a&b&&c]"###, r###"v"###);
}

#[test]
fn ampersand_first_control() {
    check(
        r###"[&a]"###,
        r###"v"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[&a]"###, r###"v"###, r###"A"###, None);
    check(
        r###"[&a]"###,
        r###"v"###,
        r###"&"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[&a]"###, r###"v"###, r###"b"###, None);
    check(r###"[&a]"###, r###"v"###, r###"c"###, None);
    check(r###"[&a]"###, r###"v"###, r###"1"###, None);
    check(
        r###"[&a]"###,
        r###"v"###,
        r###"ab"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[&a]"###, r###"v"###, r###"cd"###, None);
    check(r###"[&a]"###, r###"v"###, r###"!"###, None);
    check(r###"[&a]"###, r###"v"###, r###"z"###, None);
}

#[test]
fn negated_string_syntax() {
    syntax(r###"[^\q{ab}&]"###, r###"v"###);
}

#[test]
fn range_first_control() {
    check(
        r###"[a-z&]"###,
        r###"v"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a-z&]"###, r###"v"###, r###"A"###, None);
    check(
        r###"[a-z&]"###,
        r###"v"###,
        r###"&"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[a-z&]"###,
        r###"v"###,
        r###"b"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[a-z&]"###,
        r###"v"###,
        r###"c"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a-z&]"###, r###"v"###, r###"1"###, None);
    check(
        r###"[a-z&]"###,
        r###"v"###,
        r###"ab"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[a-z&]"###,
        r###"v"###,
        r###"cd"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a-z&]"###, r###"v"###, r###"!"###, None);
    check(
        r###"[a-z&]"###,
        r###"v"###,
        r###"z"###,
        Some(vec![Some(0..1)]),
    );
}

#[test]
fn escaped_ampersand_control() {
    check(
        r###"[a\&]"###,
        r###"v"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a\&]"###, r###"v"###, r###"A"###, None);
    check(
        r###"[a\&]"###,
        r###"v"###,
        r###"&"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a\&]"###, r###"v"###, r###"b"###, None);
    check(r###"[a\&]"###, r###"v"###, r###"c"###, None);
    check(r###"[a\&]"###, r###"v"###, r###"1"###, None);
    check(
        r###"[a\&]"###,
        r###"v"###,
        r###"ab"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a\&]"###, r###"v"###, r###"cd"###, None);
    check(r###"[a\&]"###, r###"v"###, r###"!"###, None);
    check(r###"[a\&]"###, r###"v"###, r###"z"###, None);
}

#[test]
fn case_folding() {
    check(
        r###"[a&]"###,
        r###"vi"###,
        r###"a"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[a&]"###,
        r###"vi"###,
        r###"A"###,
        Some(vec![Some(0..1)]),
    );
    check(
        r###"[a&]"###,
        r###"vi"###,
        r###"&"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a&]"###, r###"vi"###, r###"b"###, None);
    check(r###"[a&]"###, r###"vi"###, r###"c"###, None);
    check(r###"[a&]"###, r###"vi"###, r###"1"###, None);
    check(
        r###"[a&]"###,
        r###"vi"###,
        r###"ab"###,
        Some(vec![Some(0..1)]),
    );
    check(r###"[a&]"###, r###"vi"###, r###"cd"###, None);
    check(r###"[a&]"###, r###"vi"###, r###"!"###, None);
    check(r###"[a&]"###, r###"vi"###, r###"z"###, None);
}
