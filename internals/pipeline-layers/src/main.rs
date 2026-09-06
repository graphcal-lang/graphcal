mod analyze;
#[cfg(test)]
mod analyze_tests;
mod config;
mod discover;
mod model;
mod ratchet;

use analyze::{analyze, violations};
use anyhow::{Context, Result, bail};
use config::{Baseline, RoleMap};
use discover::{SourceTree, module_ids};
use model::{Edge, EdgeKey, Role};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Check,
    InitBaseline,
    Prune,
    Report,
    InitRoleMap,
}

impl Command {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "check" => Ok(Self::Check),
            "init-baseline" => Ok(Self::InitBaseline),
            "prune" => Ok(Self::Prune),
            "report" => Ok(Self::Report),
            "init-role-map" => Ok(Self::InitRoleMap),
            other => bail!(
                "unknown command {other:?}; use check, report, init-baseline, init-role-map, or prune"
            ),
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("pipeline-layers: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let (command, repo) = parse_args(env::args().skip(1))?;
    let map_path = repo.join("internals/pipeline-layers/role-map.toml");
    let baseline_path = repo.join("internals/pipeline-layers/baseline.toml");
    match command {
        Command::Check => check(&repo, &map_path, &baseline_path),
        Command::InitBaseline => init_baseline(&repo, &map_path, &baseline_path),
        Command::Prune => prune(&repo, &map_path, &baseline_path),
        Command::Report => report(&repo, &map_path),
        Command::InitRoleMap => init_role_map(&repo, &map_path),
    }
}

fn parse_args<I>(mut args: I) -> Result<(Command, PathBuf)>
where
    I: Iterator<Item = String>,
{
    let raw_command = args.next().unwrap_or_else(|| "check".to_owned());
    let command = Command::parse(&raw_command)?;
    let repo = args
        .next()
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    if args.next().is_some() {
        bail!("too many arguments; expected [{raw_command}] [repo]");
    }
    Ok((command, repo))
}

fn load(repo: &Path, map_path: &Path) -> Result<(SourceTree, RoleMap, Vec<Edge>, usize)> {
    let tree = SourceTree::discover(repo)?;
    let roles = RoleMap::load(map_path)?;
    roles.validate(&module_ids(&tree))?;
    let analysis = analyze(&tree).map_err(|error| anyhow::anyhow!(error))?;
    let total_edges = analysis.edges.len();
    let forbidden = violations(&analysis, &roles);
    Ok((tree, roles, forbidden, total_edges))
}

fn check(repo: &Path, map_path: &Path, baseline_path: &Path) -> Result<()> {
    let (tree, roles, current, total_edges) = load(repo, map_path)?;
    let baseline = Baseline::load(baseline_path)?;
    let result = ratchet::compare(&current, &baseline);
    println!(
        "modules: {} | edges: {} | forbidden: {} | baseline: {}",
        tree.modules.len(),
        total_edges,
        current.len(),
        baseline.exceptions.len()
    );
    if !result.stale.is_empty() || !result.new.is_empty() {
        result
            .stale
            .iter()
            .for_each(|key| println!("STALE exception: {}", key_display(key, &roles)));
        result
            .new
            .iter()
            .for_each(|key| println!("NEW forbidden edge: {}", key_display(key, &roles)));
        current
            .iter()
            .filter(|edge| result.new.contains(&edge.key))
            .for_each(print_edge);
        bail!(
            "dependency ratchet failed ({} new, {} stale)",
            result.new.len(),
            result.stale.len()
        );
    }
    println!("dependency ratchet: PASS");
    Ok(())
}

fn report(repo: &Path, map_path: &Path) -> Result<()> {
    let (tree, roles, current, _) = load(repo, map_path)?;
    println!("modules: {}", tree.modules.len());
    current.iter().for_each(|edge| {
        println!(
            "{} [{}] -> {} [{}] ({})",
            edge.key.from,
            role_name(&roles, &edge.key.from),
            edge.key.to,
            role_name(&roles, &edge.key.to),
            edge_kind(edge.key.test_only)
        );
        print_evidence(edge);
    });
    Ok(())
}

fn init_role_map(repo: &Path, map_path: &Path) -> Result<()> {
    let tree = SourceTree::discover(repo)?;
    let roles = RoleMap::load(map_path)?;
    let mut output = String::from(
        "# Explicit role for every discovered source module. Review new entries before accepting them.\n",
    );
    module_ids(&tree).into_iter().try_for_each(|id| {
        let role = roles
            .nearest_role_for(&id)
            .ok_or_else(|| anyhow::anyhow!("cannot infer a role for new root module {id}"))?;
        output.push_str("\n[[module]]\n");
        output.push_str(&format!(
            "package = {:?}\npath = [{}]\nrole = {:?}\n",
            id.package.name(),
            toml_strings(&id.path),
            role.name()
        ));
        Ok::<(), anyhow::Error>(())
    })?;
    fs::write(map_path, output).with_context(|| format!("write {}", map_path.display()))?;
    println!(
        "wrote explicit role entries for {} modules",
        tree.modules.len()
    );
    Ok(())
}

fn init_baseline(repo: &Path, map_path: &Path, baseline_path: &Path) -> Result<()> {
    if baseline_path.exists() {
        bail!("baseline already exists; use prune to remove debt without approving new edges");
    }
    let (_, roles, current, _) = load(repo, map_path)?;
    write_baseline(baseline_path, &roles, &current)?;
    println!(
        "wrote {} exact baseline exceptions to {}",
        current.len(),
        baseline_path.display()
    );
    Ok(())
}

fn prune(repo: &Path, map_path: &Path, baseline_path: &Path) -> Result<()> {
    let (_, roles, current, _) = load(repo, map_path)?;
    let baseline = Baseline::load(baseline_path)?;
    let result = ratchet::compare(&current, &baseline);
    if !result.new.is_empty() {
        result
            .new
            .iter()
            .for_each(|key| println!("NEW forbidden edge: {}", key_display(key, &roles)));
        bail!(
            "prune refuses to bless {} new forbidden edge(s)",
            result.new.len()
        );
    }
    if result.stale.is_empty() {
        println!("baseline has no stale exceptions");
        return Ok(());
    }
    let retained = baseline
        .exceptions
        .into_iter()
        .filter(|(key, _)| !result.stale.contains(key))
        .collect::<std::collections::BTreeMap<_, _>>();
    write_entries(baseline_path, &retained)?;
    println!("pruned {} stale baseline exceptions", result.stale.len());
    Ok(())
}

fn write_baseline(path: &Path, roles: &RoleMap, edges: &[Edge]) -> Result<()> {
    let entries = edges
        .iter()
        .map(|edge| {
            (
                edge.key.clone(),
                edge_reason(
                    edge,
                    roles.role_for(&edge.key.from),
                    roles.role_for(&edge.key.to),
                ),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    write_entries(path, &entries)
}

fn write_entries(path: &Path, entries: &std::collections::BTreeMap<EdgeKey, String>) -> Result<()> {
    let mut output = String::from(
        "# Exact, edge-specific exceptions. Delete entries as boundaries are removed.\n",
    );
    entries.iter().for_each(|(key, reason)| {
        output.push_str("\n[[exception]]\n");
        output.push_str(&format!(
            "from_package = {:?}\nfrom_path = [{}]\n",
            key.from.package.name(),
            toml_strings(&key.from.path)
        ));
        output.push_str(&format!(
            "to_package = {:?}\nto_path = [{}]\n",
            key.to.package.name(),
            toml_strings(&key.to.path)
        ));
        output.push_str(&format!(
            "test_only = {}\nreason = {:?}\n",
            key.test_only, reason
        ));
    });
    fs::write(path, output).with_context(|| format!("write {}", path.display()))
}

fn edge_reason(edge: &Edge, from: Option<Role>, to: Option<Role>) -> String {
    let (Some(from), Some(to)) = (from, to) else {
        return format!(
            "Observed {} with unmapped role metadata. Planned deletion seam: add an explicit role and remove this edge from the boundary exception.",
            key_display_without_roles(&edge.key)
        );
    };
    let seam = match (from, to) {
        (Role::Contracts, Role::Checking) => {
            "separate shared data/error definitions from their checking/building producers (Phases B-C)"
        }
        (Role::Contracts, Role::Interpreter) => {
            "move runtime-only construction out of the shared contracts crate"
        }
        (Role::Contracts, Role::Loading) => {
            "remove loader knowledge from shared contracts and inject a boundary adapter"
        }
        (Role::Contracts, Role::Facade) => {
            "replace facade reach-through with a stable contracts API"
        }
        (Role::Checking, Role::Loading) => {
            "split checking from source/project loading and pass a typed input boundary"
        }
        (Role::Checking, Role::Facade) => {
            "replace facade reach-through with an owned checking interface"
        }
        (Role::Interpreter, Role::Checking) => {
            "move evaluation consumers to an execution-plan/value interface"
        }
        (Role::Interpreter, Role::Loading) => {
            "remove runtime knowledge of loading and inject loaded artifacts"
        }
        (Role::Interpreter, Role::Facade) => {
            "replace interpreter-to-root calls with an owned runtime interface"
        }
        _ => "extract the dependency behind a typed boundary owned by the importing subsystem",
    };
    let evidence = edge
        .evidence
        .first()
        .map(|item| {
            format!(
                "{}:{} ({})",
                item.file.display(),
                item.line,
                item.form.name()
            )
        })
        .unwrap_or_else(|| "no syntactic evidence recorded".to_owned());
    format!(
        "Observed {} at {evidence}; roles are {} -> {}. Planned deletion seam: {seam}.",
        key_display_without_roles(&edge.key),
        from.name(),
        to.name()
    )
}

fn role_name(roles: &RoleMap, id: &model::ModuleId) -> &'static str {
    roles.role_for(id).map_or("unmapped", Role::name)
}

fn key_display(key: &EdgeKey, roles: &RoleMap) -> String {
    format!(
        "{} [{}] -> {} [{}] ({})",
        key.from,
        role_name(roles, &key.from),
        key.to,
        role_name(roles, &key.to),
        edge_kind(key.test_only)
    )
}

fn print_edge(edge: &Edge) {
    println!("  {}", key_display_without_roles(&edge.key));
    print_evidence(edge);
}

fn key_display_without_roles(key: &EdgeKey) -> String {
    format!("{} -> {} ({})", key.from, key.to, edge_kind(key.test_only))
}

fn edge_kind(test_only: bool) -> &'static str {
    if test_only { "test-only" } else { "production" }
}

fn print_evidence(edge: &Edge) {
    edge.evidence
        .iter()
        .take(5)
        .for_each(|e| println!("    {}:{} {}", e.file.display(), e.line, e.form.name()));
}

fn toml_strings(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| format!("{part:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::{Command, parse_args};
    use std::path::PathBuf;

    #[test]
    fn command_rejects_extra_arguments() {
        assert!(parse_args(["check".into(), ".".into(), "unexpected".into()].into_iter()).is_err());
        assert_eq!(
            parse_args(["report".into()].into_iter()).expect("valid arguments"),
            (Command::Report, PathBuf::from("."))
        );
    }
}
