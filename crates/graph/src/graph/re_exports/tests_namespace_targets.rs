use rustc_hash::FxHashMap;

use super::test_support::{
    ambient_imports, build_without_entry_points, discovered_files, export, is_used, module,
    named_import, re_export,
};
use super::{ExposedNamespaceTargets, Exposure};
use crate::graph::ModuleGraph;
use crate::resolve::ResolvedReplacedModuleTarget;
use fallow_types::discover::{EntryPoint, EntryPointSource, FileId};
use fallow_types::extract::MemberAccess;

#[test]
fn ambient_namespace_forwards_default_namespace_exports_without_entry_points() {
    for (observation, named_used, default_namespace_used) in [
        (None, false, false),
        (Some(false), true, false),
        (Some(true), true, true),
    ] {
        let mut ambient = module(0, "ambient");
        let mut leaf = module(1, "leaf");
        leaf.exports = vec![export("leafValue")].into();
        let mut target = module(2, "target");
        target.exports = vec![export("namedValue")].into();
        target.re_exports = vec![re_export(&leaf, "*", "default")];
        if let Some(namespace_object) = observation {
            ambient.resolved_imports = ambient_imports(&target, namespace_object);
        }

        let graph = build_without_entry_points(&[ambient, leaf, target]);

        assert_eq!(
            is_used(&graph, 2, "namedValue"),
            named_used,
            "ambient observation {observation:?} must preserve the named surface"
        );
        assert_eq!(
            is_used(&graph, 1, "leafValue"),
            default_namespace_used,
            "ambient observation {observation:?}: only a namespace object forwards \
             the target's default namespace"
        );
    }
}

#[test]
fn namespace_exposure_includes_default_after_reaching_the_same_module_through_a_star() {
    for namespace_first in [false, true] {
        let mut ambient = module(0, "ambient");
        let mut leaf = module(1, "leaf");
        leaf.exports = vec![export("leafValue")].into();
        let mut member = module(2, "member");
        member.re_exports = vec![re_export(&leaf, "*", "default")];
        let mut seed = module(3, "seed");
        let star = re_export(&member, "*", "*");
        let namespace = re_export(&member, "*", "ns");
        seed.re_exports = if namespace_first {
            vec![namespace, star]
        } else {
            vec![star, namespace]
        };
        ambient.resolved_imports = ambient_imports(&seed, false);

        let graph = build_without_entry_points(&[ambient, leaf, member, seed]);

        assert!(
            is_used(&graph, 1, "leafValue"),
            "namespace_first = {namespace_first}: namespace exposure must include default"
        );
    }
}

/// Build a namespace re-export with route provenance on and no entry point.
/// Route provenance is only tracked when a test root replaces a module. With
/// `consumer`, the test root reads one member of the namespace.
fn namespace_re_export_with_route_provenance(consumer: bool) -> ModuleGraph {
    let mut test_root = module(0, "root.test");
    if consumer {
        test_root.resolved_imports = vec![named_import(2, "ns")];
        test_root.member_accesses = vec![MemberAccess {
            object: "ns".to_string(),
            member: "sourceValue".to_string(),
        }]
        .into();
    }
    let mut source = module(1, "source");
    source.exports = vec![export("sourceValue")].into();
    let mut barrel = module(2, "barrel");
    barrel.re_exports = vec![re_export(&source, "*", "ns")];
    let replaced = module(3, "replaced");
    let modules = [test_root, source, barrel, replaced];
    let files = discovered_files(&modules);
    let test_roots = [EntryPoint {
        path: modules[0].path.clone(),
        source: EntryPointSource::TestFile,
    }];

    let graph = ModuleGraph::build_with_reachability_roots_and_replacements(
        &modules,
        &[ResolvedReplacedModuleTarget {
            source_file: FileId(0),
            target_file: FileId(3),
        }],
        &[],
        &[],
        &test_roots,
        &files,
    );
    assert!(graph.modules.iter().all(|module| !module.is_entry_point()));
    graph
}

/// With nothing observed and no entry point, a namespace re-export has no
/// consumer to credit, so Phase 2c must not record a reference route for it.
#[test]
fn unobserved_namespace_re_export_records_no_reference_route() {
    let graph = namespace_re_export_with_route_provenance(false);

    assert!(!is_used(&graph, 1, "sourceValue"));
    assert!(
        graph.reference_routes.graphs.is_empty(),
        "no consumer can use a route for the unobserved namespace re-export"
    );
}

/// The same fixture with a consumer records a route, which shows that route
/// provenance is on and the empty result above is not a fixture accident.
#[test]
fn consumed_namespace_re_export_records_a_reference_route() {
    let graph = namespace_re_export_with_route_provenance(true);

    assert!(is_used(&graph, 1, "sourceValue"));
    assert!(
        !graph.reference_routes.graphs.is_empty(),
        "the consumer's namespace member access must record its route"
    );
}

/// Drain `stack` like the exposure walk, where every member re-exports the
/// next member of a cycle at `exposure`. Returns the number of pops, or
/// `None` when the walk is still running after `budget` pops.
fn walk_cycle(
    closure: &mut ExposedNamespaceTargets,
    stack: &mut Vec<(FileId, Exposure)>,
    cycle: &[FileId],
    exposure: Exposure,
    budget: usize,
) -> Option<usize> {
    let mut pops = 0;
    while let Some((file_id, _)) = stack.pop() {
        pops += 1;
        if pops > budget {
            return None;
        }
        let position = cycle.iter().position(|&member| member == file_id)?;
        let next = cycle[(position + 1) % cycle.len()];
        closure.record(stack, next, exposure);
    }
    Some(pops)
}

#[test]
fn closure_walk_visits_each_cycle_member_once_per_exposure() {
    const BUDGET: usize = 16;
    let cycle = [FileId(0), FileId(1)];
    for exposure in [Exposure::StarSurface, Exposure::NamespaceObject] {
        let mut closure = ExposedNamespaceTargets {
            members: FxHashMap::default(),
        };
        let mut stack = Vec::new();
        closure.record(&mut stack, cycle[0], exposure);

        let pops = walk_cycle(&mut closure, &mut stack, &cycle, exposure, BUDGET);

        assert_eq!(
            pops,
            Some(cycle.len()),
            "exposure {exposure:?}: a revisit at the same exposure must not walk again"
        );
    }
}

#[test]
fn closure_record_rewalks_a_member_only_when_its_exposure_widens() {
    let member = FileId(0);
    for (first, second, rewalk) in [
        (Exposure::StarSurface, Exposure::NamespaceObject, true),
        (Exposure::NamespaceObject, Exposure::StarSurface, false),
    ] {
        let mut closure = ExposedNamespaceTargets {
            members: FxHashMap::default(),
        };
        let mut stack = Vec::new();
        closure.record(&mut stack, member, first);
        stack.clear();

        closure.record(&mut stack, member, second);

        let expected: Vec<_> = if rewalk {
            vec![(member, Exposure::NamespaceObject)]
        } else {
            Vec::new()
        };
        assert_eq!(stack, expected, "{first:?} then {second:?}");
        assert!(
            closure.exposes_name(member, "default"),
            "{first:?} then {second:?}: the wider exposure must stay recorded"
        );
    }
}
