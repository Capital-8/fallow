#![cfg(all(feature = "std", feature = "utf16"))]

use regress::{CompileBudget, MatchBudget, Regex};

fn compile(pattern: &str, flags: &str, bounded: bool) -> Regex {
    if bounded {
        let mut budget = CompileBudget::new(100_000_000, 1_000_000_000, 1_000);
        Regex::from_unicode_with_limits(pattern.chars().map(u32::from), flags, &mut budget).unwrap()
    } else {
        Regex::with_flags(pattern, flags).unwrap()
    }
}
#[test]
fn saved_node_rollup_cases_keep_match_ranges_and_captures() {
    for bounded in [false, true] {
        // literal-dollar-and-match
        {
            let regex = compile("a", "", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[97], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..1)])
            );
        }
        // prefix-match-suffix
        {
            let regex = compile("(b)", "", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[97, 98, 99], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(1..2), Some(1..2)])
            );
        }
        // numeric-capture-fallback
        {
            let regex = compile("(a)(b)?", "", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[97], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..1), Some(0..1), None])
            );
        }
        // named-present-unmatched-missing
        {
            let regex = compile("(?<x>a)(?<y>b)?", "", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[97], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..1), Some(0..1), None])
            );
            assert_eq!(actual.as_ref().unwrap().named_group("x"), Some(0..1));
            assert_eq!(actual.as_ref().unwrap().named_group("y"), None);
        }
        // named-token-with-no-named-group
        {
            let regex = compile("a", "", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[97], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..1)])
            );
        }
        // unterminated-named-token
        {
            let regex = compile("(?<x>a)", "", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[97], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..1), Some(0..1)])
            );
            assert_eq!(actual.as_ref().unwrap().named_group("x"), Some(0..1));
        }
        // first-match-only
        {
            let regex = compile("a", "", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[97, 97], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..1)])
            );
        }
        // zero-width
        {
            let regex = compile("^", "", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[109, 111, 100], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..0)])
            );
        }
        // lookbehind
        {
            let regex = compile("(?<=api/)(?<part>.+)$", "", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[97, 112, 105, 47, 110, 97, 109, 101], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(4..8), Some(4..8)])
            );
            assert_eq!(actual.as_ref().unwrap().named_group("part"), Some(4..8));
        }
        // case-insensitive
        {
            let regex = compile("^NODE:MODULE$", "i", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(
                    &[110, 111, 100, 101, 58, 109, 111, 100, 117, 108, 101],
                    0,
                    &mut budget,
                )
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..11)])
            );
        }
        // dot-all
        {
            let regex = compile("^a.b$", "s", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[97, 10, 98], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..3)])
            );
        }
        // multiline
        {
            let regex = compile("^b$", "m", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[97, 10, 98, 10, 99], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(2..3)])
            );
        }
        // indices-flag
        {
            let regex = compile("(a)", "d", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[97], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..1), Some(0..1)])
            );
        }
        // legacy-code-unit
        {
            let regex = compile(".", "", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[55357, 56832], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..1)])
            );
        }
        // unicode-code-point
        {
            let regex = compile(".", "u", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_utf16_with_budget(&[55357, 56832], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..2)])
            );
        }
        // legacy-single-unit-anchor-no-match
        {
            let regex = compile("^.$", "", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_ucs2_with_budget(&[55357, 56832], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                None
            );
        }
        // unicode-single-code-point-anchor
        {
            let regex = compile("^.$", "u", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_utf16_with_budget(&[55357, 56832], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..2)])
            );
        }
        // unicode-property
        {
            let regex = compile("^\\p{Script=Greek}+$", "u", bounded);
            let mut budget = MatchBudget::new(100_000);
            let actual = regex
                .find_from_utf16_with_budget(&[931, 945], 0, &mut budget)
                .unwrap();
            assert_eq!(
                actual
                    .as_ref()
                    .map(|value| value.groups().collect::<Vec<_>>()),
                Some(vec![Some(0..2)])
            );
        }
    }
}
