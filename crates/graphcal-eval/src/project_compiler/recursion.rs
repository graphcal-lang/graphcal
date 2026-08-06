//! Cycle detection for inline-DAG instance edges.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "recursion checking consumes project compiler diagnostics and names"
)]
use super::*;

use graphcal_compiler::desugar::desugared_ast::DeclKind;

/// Check for recursive DAG instantiation.
///
/// Builds a dependency graph of inline DAGs and detects cycles.
/// Returns an error if a DAG directly or indirectly includes itself.
pub(in crate::project_compiler) fn check_dag_recursion(
    dag_definitions: &HashMap<DeclName, &graphcal_compiler::desugar::desugared_ast::DagDecl>,
    file_src: &NamedSource<Arc<String>>,
) -> Result<(), CompileError> {
    fn dfs<'a>(
        node: &'a DeclName,
        deps: &HashMap<&'a DeclName, Vec<&'a DeclName>>,
        visited: &mut HashSet<&'a DeclName>,
        in_stack: &mut HashSet<&'a DeclName>,
        path: &mut Vec<&'a DeclName>,
    ) -> Option<Vec<String>> {
        if in_stack.contains(node) {
            #[expect(
                clippy::expect_used,
                reason = "DFS invariant: in_stack ⇒ node is on path"
            )]
            let cycle_start = path
                .iter()
                .position(|n| *n == node)
                .expect("DFS invariant: in_stack ⇒ node is on path");
            let mut cycle: Vec<String> = path[cycle_start..]
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            cycle.push(node.to_string());
            return Some(cycle);
        }
        if visited.contains(node) {
            return None;
        }
        visited.insert(node);
        in_stack.insert(node);
        path.push(node);

        if let Some(neighbors) = deps.get(node) {
            for &neighbor in neighbors {
                if let Some(cycle) = dfs(neighbor, deps, visited, in_stack, path) {
                    return Some(cycle);
                }
            }
        }

        in_stack.remove(node);
        path.pop();
        None
    }

    // Build adjacency list: dag_name -> set of dag names it includes.
    let mut deps: HashMap<&DeclName, Vec<&DeclName>> = HashMap::new();
    for (name, dag) in dag_definitions {
        let mut includes = Vec::new();
        for decl in &dag.body {
            if let DeclKind::Include(inc) = &decl.kind
                && inc.path.segments.len() == 1
            {
                let target = DeclName::from_atom(inc.path.segments[0].name.clone());
                if let Some((target_name, _)) = dag_definitions.get_key_value(&target) {
                    includes.push(target_name);
                }
            }
        }
        deps.insert(name, includes);
    }

    let mut visited: HashSet<&DeclName> = HashSet::new();
    let mut in_stack: HashSet<&DeclName> = HashSet::new();
    for name in dag_definitions.keys() {
        if let Some(cycle) = dfs(name, &deps, &mut visited, &mut in_stack, &mut Vec::new()) {
            let cycle_str = cycle.join(" -> ");
            return Err(CompileError::Eval(GraphcalError::EvalError {
                message: format!("recursive DAG instantiation: {cycle_str}"),
                src: file_src.clone(),
                span: dag_definitions[name].span.into(),
            }));
        }
    }
    Ok(())
}
