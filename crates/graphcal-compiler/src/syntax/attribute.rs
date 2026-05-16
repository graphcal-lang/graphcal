//! Attribute names recognized by the language.

/// Known attribute names in the language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeName {
    Assumes,
    ExpectedFail,
    Lazy,
}

impl std::str::FromStr for AttributeName {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "assumes" => Ok(Self::Assumes),
            "expected_fail" => Ok(Self::ExpectedFail),
            "lazy" => Ok(Self::Lazy),
            _ => Err(()),
        }
    }
}
