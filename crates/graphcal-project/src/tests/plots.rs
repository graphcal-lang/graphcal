//! Containment of plot and composition property failures.

#[test]
fn ordinary_plot_and_composition_property_failures_remain_contained() {
    let result = crate::prepare::compile_and_eval("node value: Length = 1.0 m; plot good = { mark: line, encode: { x: 1.0, y: 2.0 } }; plot broken = { mark: line { stroke_width: 1.0 / 0.0 }, encode: { x: 1.0, y: 2.0 } }; plot bad_width = { mark: line, encode: { x: 1.0, y: 2.0 }, width: -1.0 }; figure comparison = { plots: [good], title: \"Valid\" }; layer overlay = { plots: [good], width: 1.0 / 0.0 };").unwrap();
    assert!(result.nodes().next().unwrap().1.is_ok());
    assert_eq!(result.plots.len(), 1);
    assert_eq!(result.figures.len(), 1);
    assert_eq!(result.plot_errors.len(), 3);
    assert!(result.presentation_diagnostics.is_empty());
}
