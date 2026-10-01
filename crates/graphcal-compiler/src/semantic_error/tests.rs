use std::collections::BTreeMap;

/// Every family module's source, scanned for its `code()` arms.
const FAMILY_SOURCES: &[(&str, &str)] = &[
    ("attribute", include_str!("attribute.rs")),
    ("domain", include_str!("domain.rs")),
    ("graph", include_str!("graph.rs")),
    ("structure", include_str!("structure.rs")),
    ("visibility", include_str!("visibility.rs")),
    ("name", include_str!("name.rs")),
    ("index", include_str!("index.rs")),
    ("plugin", include_str!("plugin.rs")),
    // CATALOG
];

/// Variant name → stable code, read from each family's `code()` match.
pub fn family_code_catalog() -> BTreeMap<String, String> {
    let mut catalog = BTreeMap::new();
    for (family, source) in FAMILY_SOURCES {
        let Some((body, _)) = source
            .split_once("fn code(&self)")
            .and_then(|(_, rest)| rest.split_once("\n    }\n"))
        else {
            panic!("family `{family}` has no `code()` method");
        };
        for line in body.lines() {
            let Some((arm, code)) = line.trim().split_once(" => \"graphcal::") else {
                continue;
            };
            let variant = arm
                .strip_prefix("Self::")
                .and_then(|rest| rest.split([' ', '(', '{']).next())
                .unwrap_or_else(|| panic!("unexpected code arm in `{family}`: {line}"));
            let code = code.trim_end_matches([',', '"']);
            let previous = catalog.insert(variant.to_owned(), code.to_owned());
            assert!(
                previous.is_none(),
                "variant `{variant}` appears in more than one family"
            );
        }
    }
    catalog
}

#[test]
fn every_family_contributes_codes_with_its_own_prefix() {
    let catalog = family_code_catalog();
    assert!(!catalog.is_empty());
    for (variant, code) in &catalog {
        assert!(
            code.len() == 4 && code[1..].chars().all(|c| c.is_ascii_digit()),
            "malformed code `{code}` for `{variant}`"
        );
    }
}
