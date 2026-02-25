//! Command parsing and execution for the interactive shell.
//!
//! Commands are prefixed with `:` (e.g., `:help`, `:list`, `:set x = 5 kg`).

/// A parsed shell command.
pub enum Command<'a> {
    Help,
    Quit,
    Clear,
    List,
    Set { name: &'a str, expr_str: &'a str },
    ClearSet { name: Option<&'a str> },
    Remove { name: &'a str, cascade: bool },
    Graph,
    Type { name: &'a str },
    Unknown(&'a str),
}

/// Parse a command string (without the leading `:`).
pub fn parse_command(input: &str) -> Command<'_> {
    let input = input.trim();
    let (cmd, rest) = input
        .split_once(char::is_whitespace)
        .map_or((input, ""), |(c, r)| (c, r.trim()));

    match cmd {
        "help" | "h" => Command::Help,
        "quit" | "q" => Command::Quit,
        "clear" => Command::Clear,
        "list" | "ls" => Command::List,
        "set" => parse_set_command(rest),
        "clear-set" => {
            if rest.is_empty() {
                Command::ClearSet { name: None }
            } else {
                Command::ClearSet { name: Some(rest) }
            }
        }
        "remove" => {
            if rest.is_empty() {
                Command::Unknown("remove (missing name)")
            } else if let Some(name) = rest.strip_suffix('+') {
                Command::Remove {
                    name: name.trim(),
                    cascade: true,
                }
            } else {
                Command::Remove {
                    name: rest,
                    cascade: false,
                }
            }
        }
        "graph" => Command::Graph,
        "type" | "t" => {
            if rest.is_empty() {
                Command::Unknown("type (missing name)")
            } else {
                Command::Type { name: rest }
            }
        }
        other => Command::Unknown(other),
    }
}

/// Parse a `:set name = expr` command.
fn parse_set_command(rest: &str) -> Command<'_> {
    if let Some((name, expr_str)) = rest.split_once('=') {
        let name = name.trim();
        let expr_str = expr_str.trim();
        if name.is_empty() || expr_str.is_empty() {
            Command::Unknown("set (expected ':set name = expr')")
        } else {
            Command::Set { name, expr_str }
        }
    } else {
        Command::Unknown("set (expected ':set name = expr')")
    }
}

/// The help text displayed by `:help`.
pub const HELP_TEXT: &str = "\
Commands:
  :help, :h            Show this help message
  :quit, :q            Exit the shell
  :clear               Clear all user-entered declarations and overrides
  :list, :ls           List all declarations with current values
  :set <p> = <expr>    Override a param value (session-only)
  :clear-set [<name>]  Remove override(s) — all if no name given
  :remove <name>       Remove a user declaration (error if has dependents)
  :remove <name>+      Remove a user declaration and all its dependents
  :graph               Show the dependency graph
  :type <name>         Show the dimension/type of a declaration

Declarations:
  Enter any Graphcal declaration (param, node, const, dimension, etc.)
  to add it to the session. Duplicate names are an error — use :remove first.

Queries:
  Type a bare name to display its current value.";
