#![cfg(all(feature = "std", feature = "utf16"))]

use regress::{Flags, MatchBudget, MatchError, Regex};

fn exhausted(pattern: &str, flags: &str, text: &str, work: usize) {
    let regex = Regex::with_flags(pattern, flags).unwrap();
    let text: Vec<u16> = text.encode_utf16().collect();
    for unicode in [false, true] {
        let mut budget = MatchBudget::new(work);
        let result = if unicode {
            regex.find_from_utf16_with_budget(&text, 0, &mut budget)
        } else {
            regex.find_from_ucs2_with_budget(&text, 0, &mut budget)
        };
        assert!(
            matches!(result, Err(MatchError::BudgetExceeded)),
            "{pattern}: {result:?}"
        );
        assert!(budget.is_exhausted());
        assert_eq!(budget.remaining(), 0);
        let easy = Regex::new("a").unwrap();
        assert!(matches!(
            easy.find_from_ucs2_with_budget(&[97], 0, &mut budget),
            Err(MatchError::BudgetExceeded)
        ));
    }
}

#[test]
fn nested_and_overlapping_backtracking_stops() {
    for pattern in ["^(a+)+$", "^(a|aa)+$", "^(a+)+?$", "^(a|aa)+?$"] {
        exhausted(pattern, "", &format!("{}!", "a".repeat(100)), 10_000);
    }
}

#[test]
fn optimized_greedy_and_lazy_loops_consume_budget() {
    for pattern in ["^a+$", "^a+?$", "^.+$", "^.*?$", "^[a-z]+$", "^a{10000}$"] {
        exhausted(pattern, "", &"a".repeat(10_000), 100);
    }
}

#[test]
fn nested_and_negative_lookaround_cannot_hide_exhaustion() {
    for pattern in [
        "^(?=(a+)+$)a",
        "^(?!(a+)+$)a",
        "^(?=(?!(a+)+$))a",
        "(?<=(a+)+)!",
    ] {
        exhausted(pattern, "", &format!("{}!", "a".repeat(100)), 100);
    }
}

#[test]
fn backreference_bulk_comparison_is_budgeted() {
    // The reference is reached after its capture scan. Its full range cannot
    // bypass the allowance even when the comparison is implemented in bulk.
    for flags in ["", "i", "iu"] {
        exhausted(r"^(a{300})\1$", flags, &"a".repeat(600), 450);
    }
}

#[test]
fn unanchored_candidate_scan_and_shared_calls_consume_one_budget() {
    exhausted("z", "", &"a".repeat(1_000), 100);
    let regex = Regex::new("a").unwrap();
    let mut budget = MatchBudget::new(32);
    let first = regex
        .find_from_ucs2_with_budget(&[97], 0, &mut budget)
        .unwrap()
        .unwrap();
    assert_eq!(first.range, 0..1);
    let remaining = budget.remaining();
    assert!(remaining < 32);
    loop {
        if regex
            .find_from_ucs2_with_budget(&[97], 0, &mut budget)
            .is_err()
        {
            break;
        }
        assert!(budget.remaining() < remaining);
    }
    assert!(budget.is_exhausted());
}

#[test]
fn normal_results_keep_captures_and_unicode_semantics() {
    for (pattern, flags, text) in [
        ("(?<x>a)(?<y>b)?", "", "a"),
        (r"(?<=api/)(?<part>.+)$", "", "api/name"),
        (r"^(a+)\1$", "", "aaaa"),
        (r"^(Σ+)\1$", "iu", "ΣσΣσ"),
        (r"^\p{Script=Greek}+$", "u", "Σα"),
        ("^NODE:MODULE$", "i", "node:module"),
        ("^a.b$", "s", "a\nb"),
        ("^b$", "m", "a\nb\nc"),
        (".", "", "😀"),
        (".", "u", "😀"),
        ("^.$", "", "😀"),
        ("^.$", "u", "😀"),
        ("^", "", "mod"),
        ("no", "", "yes"),
    ] {
        for no_opt in [false, true] {
            let mut flags = Flags::from(flags);
            flags.no_opt = no_opt;
            let regex = Regex::with_flags(pattern, flags).unwrap();
            let text: Vec<u16> = text.encode_utf16().collect();
            for unicode in [false, true] {
                let expected = if unicode {
                    regex.find_from_utf16(&text, 0).next()
                } else {
                    regex.find_from_ucs2(&text, 0).next()
                };
                let mut budget = MatchBudget::new(100_000);
                let actual = if unicode {
                    regex.find_from_utf16_with_budget(&text, 0, &mut budget)
                } else {
                    regex.find_from_ucs2_with_budget(&text, 0, &mut budget)
                }
                .unwrap();
                assert_eq!(format!("{actual:?}"), format!("{expected:?}"), "{pattern}");
            }
        }
    }
}

#[test]
fn initial_position_scan_is_bounded_before_matching() {
    let regex = Regex::new("a").unwrap();
    let text = vec![97; 1_100];
    let mut budget = MatchBudget::new(20);
    assert!(matches!(
        regex.find_from_utf16_with_budget(&text, 1_000, &mut budget),
        Err(MatchError::BudgetExceeded)
    ));
}

#[test]
fn copying_named_capture_metadata_is_bounded() {
    let regex = Regex::new(&format!("^(?<{}>a)$", "x".repeat(1_000))).unwrap();
    let mut budget = MatchBudget::new(20);
    assert!(matches!(
        regex.find_from_ucs2_with_budget(&[97], 0, &mut budget),
        Err(MatchError::BudgetExceeded)
    ));
}
