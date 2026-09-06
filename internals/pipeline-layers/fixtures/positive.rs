mod contracts {
    pub struct Token;
}

mod checking {
    use crate::contracts::Token;
    pub fn checked() -> Token { Token }
}

mod interpreter {
    pub fn execute(_: crate::contracts::Token) {}
}

mod facade {
    pub fn run() { crate::interpreter::execute(crate::checking::checked()); }
}
