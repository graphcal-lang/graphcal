#![cfg(test)]

use graphcal_fmt::format_source;

#[test]
fn compact_empty_and_qualified_dependency_lists_round_trip() {
    for (source, expected) in [
        (
            "node x:Length=todo<Length>{};",
            "node x: Length = todo<Length> {};\n",
        ),
        (
            "node x:Length=todo<Length>{@a,@module.child::b,};",
            "node x: Length = todo<Length> { @a, @module.child::b };\n",
        ),
    ] {
        assert_eq!(format_source(source).unwrap(), expected);
        assert_eq!(format_source(expected).unwrap(), expected);
    }
}

#[test]
fn comments_and_multiline_lists_are_preserved() {
    for source in [
        "node x: Length = todo<Length> { // opening\n// before\n@first, // after\n@second,\n// last\n}; // end\n",
        "node x: Length = todo<Length> {\n// empty\n};\n",
        "node long_name: Length = todo<Length> { @first_long_dependency_name, @second_long_dependency_name, @third_long_dependency_name };\n",
        "node x: Length = todo<Length> // marker\n{ @a };\n",
        "node x: Length = 2.0 * todo<Length[Fin(2)]> { @a }[0];\n",
    ] {
        let formatted = format_source(source).unwrap();
        assert_eq!(formatted, format_source(&formatted).unwrap());
        for comment in [
            "// opening",
            "// before",
            "// after",
            "// last",
            "// end",
            "// empty",
            "// marker",
        ] {
            if source.contains(comment) {
                assert!(formatted.contains(comment), "lost {comment}: {formatted}");
            }
        }
    }
}
