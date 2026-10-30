use super::capture_propagation_visits;
use super::propagate::count_star_name_checks;
use super::test_support::{
    build, export, module, named_import, re_export, type_export, type_import,
};
use crate::graph::{ExportNamespace, ExportSymbol, ModuleGraph};
use fallow_types::discover::FileId;

fn reference_namespaces(export: &ExportSymbol) -> Vec<ExportNamespace> {
    let mut namespaces: Vec<_> = export
        .references
        .iter()
        .map(|reference| reference.namespace)
        .collect();
    namespaces.sort_unstable_by_key(|namespace| match namespace {
        ExportNamespace::Type => 0,
        ExportNamespace::Value => 1,
    });
    namespaces
}

fn export_states(
    graph: &ModuleGraph,
    module: usize,
    name: &str,
) -> Vec<(bool, Vec<ExportNamespace>)> {
    let mut states: Vec<_> = graph.modules[module]
        .exports
        .iter()
        .filter(|export| export.name.matches_str(name))
        .map(|export| (export.is_type_only, reference_namespaces(export)))
        .collect();
    states.sort_unstable_by_key(|(is_type_only, _)| !*is_type_only);
    states
}

/// A single star checks each imported source name exactly once.
#[test]
fn star_name_checks_count_each_imported_name() {
    let mut consumer = module(0, "a_consumer");
    consumer.resolved_imports = vec![named_import(1, "first"), named_import(1, "second")];
    let mut barrel = module(1, "b_barrel");
    barrel.re_exports = vec![re_export(2, "*", "*")];
    let mut source = module(2, "c_source");
    source.exports = vec![export("first"), export("second"), export("unused")].into();

    let (graph, checks) = count_star_name_checks(|| build(&[consumer, barrel, source], 0));

    assert_eq!(checks, 2, "the star must check both imported names");
    assert_eq!(
        reference_namespaces(&graph.modules[2].exports[0]),
        vec![ExportNamespace::Value]
    );
    assert_eq!(
        reference_namespaces(&graph.modules[2].exports[1]),
        vec![ExportNamespace::Value]
    );
    assert!(graph.modules[2].exports[2].references.is_empty());
}

/// Star references revisit an earlier barrel and reach its leaf export.
#[test]
fn star_change_requeues_an_earlier_barrel() {
    let mut inner = module(0, "a_inner");
    inner.re_exports = vec![re_export(1, "*", "*")];
    let mut leaf = module(1, "b_leaf");
    leaf.exports = vec![export("Item")].into();
    let mut outer = module(2, "c_outer");
    outer.re_exports = vec![re_export(0, "*", "*")];
    let mut consumer = module(3, "d_consumer");
    consumer.resolved_imports = vec![named_import(2, "Item")];

    let (graph, visits) = capture_propagation_visits(|| build(&[inner, leaf, outer, consumer], 3));

    let inner_edge = (FileId(0), FileId(1));
    let outer_edge = (FileId(2), FileId(0));
    let first_inner = visits.iter().position(|visit| *visit == inner_edge);
    let first_outer = visits.iter().position(|visit| *visit == outer_edge);
    assert!(
        first_inner < first_outer,
        "the inner star must run before the outer star, so only a requeue can forward the reference: {visits:?}"
    );
    assert_eq!(
        visits.iter().filter(|visit| **visit == inner_edge).count(),
        2,
        "the outer star change must requeue the inner star once: {visits:?}"
    );
    assert_eq!(
        reference_namespaces(&graph.modules[1].exports[0]),
        vec![ExportNamespace::Value],
        "the earlier inner star must forward the reference added by the outer star"
    );
    let stub = graph.modules[0]
        .exports
        .iter()
        .find(|export| export.name.matches_str("Item"))
        .expect("the inner barrel must synthesize Item");
    assert!(!stub.is_type_only);
    assert_eq!(reference_namespaces(stub), vec![ExportNamespace::Value]);
}

/// A barrel with a local type synthesizes its forwarded value separately.
#[test]
fn star_synthesizes_value_beside_an_existing_type() {
    let mut consumer = module(0, "a_consumer");
    consumer.resolved_imports = vec![named_import(1, "Item")];
    let mut barrel = module(1, "b_barrel");
    barrel.re_exports = vec![re_export(2, "*", "*")];
    let mut source = module(2, "c_source");
    source.exports = vec![type_export("Item")].into();
    source.re_exports = vec![re_export(3, "*", "*")];
    let mut leaf = module(3, "d_leaf");
    leaf.exports = vec![export("Item")].into();

    let graph = build(&[consumer, barrel, source, leaf], 0);

    assert_eq!(
        export_states(&graph, 2, "Item"),
        vec![(true, vec![]), (false, vec![ExportNamespace::Value]),],
        "the forwarded value needs its own stub beside the unused local type"
    );
    assert_eq!(
        export_states(&graph, 3, "Item"),
        vec![(false, vec![ExportNamespace::Value])]
    );
}

/// A type source never gains a value stub for a sibling star's value.
#[test]
fn star_keeps_sibling_value_references_off_a_type_source() {
    for source_has_star in [false, true] {
        for triggering_is_type_only in [false, true] {
            let mut consumer = module(0, "a_consumer");
            let mut type_import = type_import(1, "Item");
            type_import.info.local_name = "ItemType".to_string();
            consumer.resolved_imports = vec![type_import, named_import(1, "Item")];
            let mut barrel = module(1, "b_barrel");
            barrel.re_exports = vec![
                {
                    let mut star = re_export(2, "*", "*");
                    star.info.is_type_only = triggering_is_type_only;
                    star
                },
                re_export(3, "*", "*"),
            ];
            let mut source = module(2, "c_source");
            source.exports = vec![type_export("Item")].into();
            if source_has_star {
                source.re_exports = vec![re_export(4, "*", "*")];
            }
            let mut value_leaf = module(3, "d_value_leaf");
            value_leaf.exports = vec![export("Item")].into();
            let empty_leaf = module(4, "e_empty_leaf");

            let graph = build(&[consumer, barrel, source, value_leaf, empty_leaf], 0);

            assert_eq!(
                export_states(&graph, 2, "Item"),
                vec![(true, vec![ExportNamespace::Type])],
                "only the type resolves through this source: source_has_star={source_has_star}, triggering_is_type_only={triggering_is_type_only}"
            );
            assert_eq!(
                export_states(&graph, 3, "Item"),
                vec![(false, vec![ExportNamespace::Value])]
            );
        }
    }
}

/// A normal self-alias of a type receives a type stub through an outer star.
#[test]
fn star_synthesizes_type_for_a_self_re_exported_alias() {
    let mut consumer = module(0, "a_consumer");
    consumer.resolved_imports = vec![type_import(1, "Item")];
    let mut barrel = module(1, "b_barrel");
    barrel.re_exports = vec![re_export(2, "*", "*")];
    let mut source = module(2, "c_source");
    source.exports = vec![type_export("Origin")].into();
    source.re_exports = vec![re_export(2, "Origin", "Item"), re_export(3, "*", "*")];
    let empty_leaf = module(3, "d_empty_leaf");

    let graph = build(&[consumer, barrel, source, empty_leaf], 0);

    assert_eq!(
        export_states(&graph, 2, "Item"),
        vec![(true, vec![ExportNamespace::Type]), (false, vec![])],
        "the value-shaped alias stub cannot stand in for its originating type"
    );
    assert_eq!(
        export_states(&graph, 2, "Origin"),
        vec![(true, vec![ExportNamespace::Type])]
    );
}

/// A shadowing barrel type cannot create a stub on an ambiguous type source.
#[test]
fn star_keeps_a_shadowed_type_off_an_ambiguous_source() {
    let mut consumer = module(0, "a_consumer");
    let mut type_import = type_import(2, "Item");
    type_import.info.local_name = "ItemType".to_string();
    consumer.resolved_imports = vec![named_import(1, "Item"), type_import];
    let mut value_barrel = module(1, "b_value_barrel");
    value_barrel.re_exports = vec![re_export(3, "*", "*")];
    let mut type_barrel = module(2, "c_type_barrel");
    type_barrel.exports = vec![type_export("Item")].into();
    type_barrel.re_exports = vec![re_export(3, "*", "*")];
    let mut source = module(3, "d_source");
    source.re_exports = vec![
        re_export(4, "*", "*"),
        re_export(5, "*", "*"),
        re_export(6, "*", "*"),
    ];
    let mut first_type = module(4, "e_first_type");
    first_type.exports = vec![type_export("Item")].into();
    let mut second_type = module(5, "f_second_type");
    second_type.exports = vec![type_export("Item")].into();
    let mut value_leaf = module(6, "g_value_leaf");
    value_leaf.exports = vec![export("Item")].into();

    let graph = build(
        &[
            consumer,
            value_barrel,
            type_barrel,
            source,
            first_type,
            second_type,
            value_leaf,
        ],
        0,
    );

    assert_eq!(
        export_states(&graph, 3, "Item"),
        vec![(false, vec![ExportNamespace::Value])],
        "the earlier value star creates only a value stub; the later barrel's own type must not add a type stub"
    );
    assert_eq!(
        export_states(&graph, 2, "Item"),
        vec![(true, vec![ExportNamespace::Type])]
    );
    assert_eq!(export_states(&graph, 4, "Item"), vec![(true, vec![])]);
    assert_eq!(export_states(&graph, 5, "Item"), vec![(true, vec![])]);
    assert_eq!(
        export_states(&graph, 6, "Item"),
        vec![(false, vec![ExportNamespace::Value])]
    );
}
