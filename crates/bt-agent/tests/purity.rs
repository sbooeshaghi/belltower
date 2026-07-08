use serde_json::Value;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Command;

#[test]
fn bt_agent_internal_dependency_closure_stays_pure() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("bt-agent should live under crates/<name> in the workspace root");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());

    let output = Command::new(cargo)
        .current_dir(workspace_root)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--manifest-path",
        ])
        .arg(workspace_root.join("Cargo.toml"))
        .output()
        .expect("cargo metadata should run");

    assert!(
        output.status.success(),
        "cargo metadata failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let metadata: Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata should return valid json");
    let packages = metadata["packages"]
        .as_array()
        .expect("metadata packages should be an array");
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .expect("metadata resolve.nodes should be an array");

    let package_name_by_id = packages
        .iter()
        .filter_map(|package| {
            Some((
                package.get("id")?.as_str()?.to_owned(),
                package.get("name")?.as_str()?.to_owned(),
            ))
        })
        .collect::<HashMap<_, _>>();

    let bt_agent_id = package_name_by_id
        .iter()
        .find_map(|(id, name)| (name == "bt-agent").then_some(id.clone()))
        .expect("bt-agent package id should exist in cargo metadata");

    let dependency_ids_by_package = nodes
        .iter()
        .filter_map(|node| {
            let id = node.get("id")?.as_str()?.to_owned();
            let dependencies = node
                .get("dependencies")?
                .as_array()?
                .iter()
                .filter_map(|dependency| dependency.as_str().map(ToOwned::to_owned))
                .collect::<Vec<_>>();
            Some((id, dependencies))
        })
        .collect::<HashMap<_, _>>();

    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::from([bt_agent_id.clone()]);

    while let Some(package_id) = queue.pop_front() {
        if !visited.insert(package_id.clone()) {
            continue;
        }
        if let Some(dependencies) = dependency_ids_by_package.get(&package_id) {
            queue.extend(dependencies.iter().cloned());
        }
    }

    let allowed_internal = BTreeSet::from([
        "bt-agent".to_owned(),
        "bt-core".to_owned(),
        "bt-tools".to_owned(),
    ]);

    let disallowed_internal = visited
        .into_iter()
        .filter_map(|package_id| package_name_by_id.get(&package_id).cloned())
        .filter(|name| name.starts_with("bt-") || name == "belltower")
        .filter(|name| !allowed_internal.contains(name))
        .collect::<Vec<_>>();

    assert!(
        disallowed_internal.is_empty(),
        "bt-agent internal dependency closure must stay pure. allowed internal crates: {:?}. found disallowed internal crates in closure: {:?}",
        allowed_internal,
        disallowed_internal
    );
}
