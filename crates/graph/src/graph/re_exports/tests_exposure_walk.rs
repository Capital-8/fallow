use std::path::PathBuf;

use super::module_path_and_display;
use super::test_support::{
    ambient_imports, build, build_without_entry_points, export, module, re_export, symbol,
};
use fallow_types::discover::FileId;

/// An ambient namespace exposes a default namespace and its transitive star exports.
#[test]
fn ambient_namespace_default_re_export_credits_deep_exports() {
    let mut ambient = module(0, "ambient");
    ambient.resolved_imports = ambient_imports(1, true);
    let mut seed = module(1, "seed");
    seed.re_exports = vec![re_export(2, "*", "default")];
    let mut surface = module(2, "surface");
    surface.re_exports = vec![re_export(3, "*", "*")];
    let mut deep = module(3, "deep");
    deep.exports = vec![export("used")].into();
    let resolved_modules = vec![ambient, seed, surface, deep];

    let graph = build_without_entry_points(&resolved_modules);

    assert!(!graph.modules[2].is_reachable());
    assert!(
        !symbol(&graph, 3, "used").references.is_empty(),
        "the ambient default namespace should expose the deep star export"
    );
}

/// A star surface exposes named namespace re-exports and their transitive exports.
#[test]
fn ambient_star_chain_namespace_re_export_credits_deep_exports() {
    let mut ambient = module(0, "ambient");
    ambient.resolved_imports = ambient_imports(1, false);
    let mut seed = module(1, "seed");
    seed.re_exports = vec![re_export(2, "*", "*")];
    let mut member = module(2, "member");
    member.re_exports = vec![re_export(3, "*", "ns")];
    let mut surface = module(3, "surface");
    surface.re_exports = vec![re_export(4, "*", "*")];
    let mut deep = module(4, "deep");
    deep.exports = vec![export("used")].into();
    let resolved_modules = vec![ambient, seed, member, surface, deep];

    let graph = build_without_entry_points(&resolved_modules);

    assert!(!graph.modules[3].is_reachable());
    assert!(
        !symbol(&graph, 4, "used").references.is_empty(),
        "the ambient star chain should expose the nested namespace's star export"
    );
}

/// A named re-export leaves unrelated exports on its source and star sources unused.
#[test]
fn named_re_export_keeps_unrelated_star_exports_unused() {
    let mut ambient = module(0, "ambient");
    ambient.resolved_imports = ambient_imports(1, false);
    let mut seed = module(1, "seed");
    seed.re_exports = vec![re_export(2, "a", "a")];
    let mut source = module(2, "source");
    source.exports = vec![export("a"), export("b")].into();
    source.re_exports = vec![re_export(3, "*", "*")];
    let mut deep = module(3, "deep");
    deep.exports = vec![export("unrelated")].into();
    let resolved_modules = vec![ambient, seed, source, deep];

    let graph = build_without_entry_points(&resolved_modules);

    assert!(!symbol(&graph, 2, "a").references.is_empty());
    assert!(symbol(&graph, 2, "b").references.is_empty());
    assert!(
        symbol(&graph, 3, "unrelated").references.is_empty(),
        "a named re-export should not expose the source's unrelated star exports"
    );
}

/// A re-export source with an unknown file id does not panic the propagation.
#[test]
fn out_of_range_re_export_source_preserves_valid_references() {
    let mut entry = module(0, "entry");
    entry.re_exports = vec![re_export(99, "*", "*"), re_export(1, "used", "used")];
    let mut source = module(1, "source");
    source.exports = vec![export("used"), export("unused")].into();
    let resolved_modules = vec![entry, source];

    let graph = build(&resolved_modules, 0);

    assert!(!symbol(&graph, 1, "used").references.is_empty());
    assert!(symbol(&graph, 1, "unused").references.is_empty());
}

/// A file id just beyond the module list displays a placeholder path.
#[test]
fn module_path_at_module_count_uses_placeholder() {
    let resolved_modules = vec![module(0, "entry")];
    let graph = build_without_entry_points(&resolved_modules);

    assert_eq!(
        module_path_and_display(FileId(0), &graph.modules),
        (
            PathBuf::from("/project/entry.ts"),
            "/project/entry.ts".to_string()
        )
    );
    assert_eq!(
        module_path_and_display(FileId(1), &graph.modules),
        (PathBuf::from("<file id 1>"), "<file id 1>".to_string())
    );
}
