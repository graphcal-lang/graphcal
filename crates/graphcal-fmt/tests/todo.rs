#![cfg(test)]

use graphcal_fmt::format_source;

#[test]
fn compact_empty_and_qualified_dependency_lists_round_trip() {
    for (source, expected) in [
        ("node x:Length=todo{};", "node x: Length = todo {};\n"),
        (
            "node x:Length=todo{@a,@module.child::b,};",
            "node x: Length = todo { @a, @module.child::b };\n",
        ),
    ] {
        assert_eq!(format_source(source).unwrap(), expected);
        assert_eq!(format_source(expected).unwrap(), expected);
    }
}

#[test]
fn comments_and_multiline_lists_are_preserved() {
    for source in [
        "node x: Length = todo { // opening\n// before\n@first, // after\n@second,\n// last\n}; // end\n",
        "node x: Length = todo {\n// empty\n};\n",
        "node long_name: Length = todo { @first_long_dependency_name, @second_long_dependency_name, @third_long_dependency_name };\n",
        "node x: Length = todo // marker\n{ @a };\n",
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
