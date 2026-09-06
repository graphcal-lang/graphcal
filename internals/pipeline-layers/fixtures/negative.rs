mod contracts {
    pub struct Token;
}

mod checking {
    pub fn checked() -> crate::contracts::Token { crate::contracts::Token }
}

mod facade {
    pub use crate::checking::checked;
}

mod interpreter {
    pub fn execute() { crate::facade::checked(); }
}
