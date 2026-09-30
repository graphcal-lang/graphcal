//! Runtime value clones performed by graph references and index accesses.

#[test]
fn indexed_graph_ref_borrows_collection_before_selecting_entry() {
    graphcal_eval::eval_expr::reset_cloned_runtime_node_count();
    let direct_copy = r"
node source: Int[Fin(4), Fin(4)] = for row: Fin(4), column: Fin(4) {
    to_int(row) * 4 + to_int(column)
};
node copied: Int[Fin(4), Fin(4)] = @source;
";
    crate::prepare::compile_and_eval(direct_copy).unwrap();
    assert_eq!(
        graphcal_eval::eval_expr::take_cloned_runtime_node_count(),
        21,
        "the clone observer must count the root, four rows, and 16 leaves"
    );

    let elementwise_copy = r"
node source: Int[Fin(4), Fin(4)] = for row: Fin(4), column: Fin(4) {
    to_int(row) * 4 + to_int(column)
};
node copied: Int[Fin(4), Fin(4)] = for row: Fin(4), column: Fin(4) {
    @source[row, column]
};
";
    crate::prepare::compile_and_eval(elementwise_copy).unwrap();
    assert_eq!(
        graphcal_eval::eval_expr::take_cloned_runtime_node_count(),
        16,
        "index traversal must clone only the 16 selected leaves"
    );
}
