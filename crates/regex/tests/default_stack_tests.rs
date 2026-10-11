#![cfg(feature = "std")]

use regress::Regex;

const CHILD_CASE: &str = "FALLOW_REGRESS_STACK_CASE";

fn pattern(kind: &str, depth: usize) -> (String, &'static str, std::ops::Range<usize>) {
    let (prefix, suffix, flags, range) = match kind {
        "group" => ("(?:", ")", "u", 0..1),
        "capture" => ("(", ")", "u", 0..1),
        "lookahead" => ("(?=", ")", "u", 0..0),
        "lookbehind" => ("(?<=", ")", "u", 1..1),
        "modifier" => ("(?i:", ")", "u", 0..1),
        "class" => ("[", "]", "v", 0..1),
        "union" => ("[b", "]", "v", 0..1),
        "intersection" => ("[a&&", "]", "v", 0..1),
        // The even-depth controls alternate the complement within {a, b} back to {a}.
        "subtraction" => ("[[ab]--", "]", "v", 0..1),
        _ => panic!("unknown case {kind}"),
    };
    (
        format!("{}a{}", prefix.repeat(depth), suffix.repeat(depth)),
        flags,
        range,
    )
}

fn run_case(case: &str) {
    let fields: Vec<_> = case.split(':').collect();
    let kind = fields[1];
    let depth = fields[2].parse().unwrap();
    let (pattern, flags, range) = pattern(kind, depth);
    if fields[0] == "unlimited" {
        let result = Regex::with_flags(&pattern, flags);
        if depth == 5000 {
            assert_eq!(
                result.unwrap_err().text,
                "Regular expression is too deeply nested"
            );
        } else {
            let compiled = result.unwrap();
            // Deep lookaround execution already exceeds the upstream default stack.
            // This control checks the supported parser depth; shallow cases check matches.
            if depth == 200 && matches!(kind, "lookahead" | "lookbehind") {
                return;
            }
            let found = compiled.find("a").unwrap();
            assert_eq!(found.range(), range);
            if kind == "capture" {
                assert_eq!(found.captures, vec![Some(0..1); depth]);
            }
        }
        return;
    }
    #[cfg(feature = "utf16")]
    {
        use regress::{CompileBudget, CompileError, CompileLimit, MatchBudget, MatchError};

        let mut budget = CompileBudget::new(100_000_000, 64_000_000, 128);
        let result =
            Regex::from_unicode_with_limits(pattern.chars().map(u32::from), flags, &mut budget);
        if depth < 128 {
            let compiled = result.unwrap();
            let text = [u16::from(b'a')];
            let mut matching = MatchBudget::new(1_000_000);
            let found = compiled
                .find_from_utf16_with_budget(&text, 0, &mut matching)
                .unwrap()
                .unwrap();
            assert_eq!(found.range(), range);
            if kind == "capture" {
                assert_eq!(found.captures, vec![Some(0..1); depth]);
            }
            let mut exhausted = MatchBudget::new(0);
            assert!(matches!(
                compiled.find_from_utf16_with_budget(&text, 0, &mut exhausted),
                Err(MatchError::BudgetExceeded)
            ));
        } else {
            assert!(
                matches!(result, Err(CompileError::Limit(CompileLimit::Depth))),
                "{result:?}"
            );
            let remaining = (budget.remaining_work(), budget.remaining_allocation());
            let next =
                Regex::from_unicode_with_limits("a".chars().map(u32::from), "u", &mut budget);
            assert!(
                matches!(next, Err(CompileError::Limit(CompileLimit::Depth))),
                "{next:?}"
            );
            assert_eq!(
                (budget.remaining_work(), budget.remaining_allocation()),
                remaining
            );
        }
    }
    #[cfg(not(feature = "utf16"))]
    panic!("bounded controls require utf16");
}

#[test]
fn default_stack_compilation() {
    if let Ok(case) = std::env::var(CHILD_CASE) {
        run_case(&case);
        return;
    }
    let executable = std::env::current_exe().unwrap();
    let modes = if cfg!(feature = "utf16") {
        &["unlimited", "bounded"][..]
    } else {
        &["unlimited"][..]
    };
    let mut failures = Vec::new();
    for mode in modes {
        for kind in [
            "group",
            "capture",
            "lookahead",
            "lookbehind",
            "modifier",
            "class",
            "union",
            "intersection",
            "subtraction",
        ] {
            let depths: &[usize] = if *mode == "bounded" {
                &[8, 64, 126, 200, 5000]
            } else {
                &[8, 200, 5000]
            };
            for depth in depths {
                let case = format!("{mode}:{kind}:{depth}");
                #[allow(
                    clippy::disallowed_methods,
                    reason = "Isolate stack-abort regressions on the consumer's default test-thread stack."
                )]
                let output = std::process::Command::new(&executable)
                    .args(["--exact", "default_stack_compilation", "--nocapture"])
                    .env(CHILD_CASE, &case)
                    // Repository test configuration must not hide a consumer's default stack.
                    .env_remove("RUST_MIN_STACK")
                    .output()
                    .unwrap();
                if !output.status.success() {
                    failures.push(format!(
                        "{case}: {}\n{}\n{}",
                        output.status,
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
