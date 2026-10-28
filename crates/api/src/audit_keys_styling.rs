//! Attribution keys of styling findings.

use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};

use super::relative_key_path;

/// Attribution key set for every styling finding in a health report.
pub fn styling_keys(report: &fallow_output::HealthReport, root: &Path) -> FxHashSet<String> {
    styling_ledger_keys(&report.styling_findings, root)
        .into_iter()
        .collect()
}

/// Identity key for one styling finding:
/// `styling:<code>:<sub kind>:<path>:<value>`.
///
/// Line numbers are excluded, so a finding survives unrelated edits above it
/// (issue #3956). Use [`styling_ledger_keys`] to tell repeated findings apart.
pub fn styling_finding_key(finding: &fallow_output::StylingFinding, root: &Path) -> String {
    format!(
        "styling:{}:{}:{}:{}",
        finding.code,
        finding.sub_kind,
        relative_key_path(Path::new(&finding.path), root),
        finding.value
    )
}

/// Attribution keys of styling findings, in the order of `findings`.
///
/// Findings with the same identity key get an occurrence suffix (`#2`, `#3`)
/// in source line order. Thus a file with two equal findings keeps two keys,
/// and a line shift does not change which key a finding gets.
pub fn styling_ledger_keys(findings: &[fallow_output::StylingFinding], root: &Path) -> Vec<String> {
    let mut keys: Vec<String> = findings
        .iter()
        .map(|finding| styling_finding_key(finding, root))
        .collect();
    let mut order: Vec<usize> = (0..findings.len()).collect();
    order.sort_by_key(|&index| findings[index].line);
    let mut seen: FxHashMap<String, usize> = FxHashMap::default();
    for index in order {
        let occurrence = seen.entry(keys[index].clone()).or_default();
        *occurrence += 1;
        if *occurrence > 1 {
            keys[index] = format!("{}#{occurrence}", keys[index]);
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::styling_ledger_keys;

    fn styling_finding(path: &str, line: u32, value: &str) -> fallow_output::StylingFinding {
        fallow_output::StylingFinding {
            code: "css-selector-complexity".to_string(),
            sub_kind: "high-specificity".to_string(),
            path: path.to_string(),
            line,
            value: value.to_string(),
            effective_severity: fallow_output::StylingFindingSeverity::Warn,
            blast_radius: None,
            confidence: None,
            agent_disposition: None,
            nearest_token: None,
            fix_hint: None,
            actions: Vec::new(),
        }
    }

    /// Issue #3956: a line shift keeps the styling keys, and repeated
    /// findings get occurrence suffixes in source line order.
    #[test]
    fn styling_ledger_keys_ignore_line_shifts_and_count_repeats() {
        let root = PathBuf::from("/project");
        let base = [
            styling_finding("src/a.css", 1, "specificity 1-2-0"),
            styling_finding("src/a.css", 4, "specificity 1-2-0"),
            styling_finding("src/b.css", 2, "specificity 1-2-0"),
        ];
        let head = [
            styling_finding("src/b.css", 2, "specificity 1-2-0"),
            styling_finding("src/a.css", 7, "specificity 1-2-0"),
            styling_finding("src/a.css", 3, "specificity 1-2-0"),
        ];

        let base_keys = styling_ledger_keys(&base, &root);
        let head_keys = styling_ledger_keys(&head, &root);

        assert_eq!(
            base_keys,
            vec![
                "styling:css-selector-complexity:high-specificity:src/a.css:specificity 1-2-0",
                "styling:css-selector-complexity:high-specificity:src/a.css:specificity 1-2-0#2",
                "styling:css-selector-complexity:high-specificity:src/b.css:specificity 1-2-0",
            ]
        );
        assert_eq!(head_keys[1], base_keys[1]);
        assert_eq!(head_keys[2], base_keys[0]);
        assert_eq!(head_keys[0], base_keys[2]);
    }
}
