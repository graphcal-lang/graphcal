# Instruction for Claude

- The goal of this project is to create type-safe, unit-aware, Git-friendly reactive programming language for engineering calculations.
- Because we target engineering projects as one of the use cases, we prioritize safety over usability. We prefer explicitness over implicitness.
  - e.g., no implicit type/unit conversion, no implicit type inference, no implicit null propagation, etc.
  - Remember Mars Climate Orbiter failure due to unit mismatch.
- This project is not yet published, so breaking changes are acceptable for simpler/clever design and implementation.
- The language design documents are in the `design/` directory. The `design/README.md` has links to all the design docs.
  - Some ideas on features are documented in `.design/IDEAS.md`.
  - The implementation phases are in `design/phases/` directory and summarized in `design/phases/README.md`.
  - `design/codebase-reading-guide.md` has information for new contributors to understand the codebase.
- The detailed discussion file can be found in `.local/` directory. Note that these are raw notes and may contain incomplete or obsolete information.
