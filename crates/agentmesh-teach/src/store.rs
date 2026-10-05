//! Local workflow store under the platform-native `AgentMesh` data directory.
//!
//! `AGENTMESH_HOME` overrides the platform default. Otherwise data lives in:
//!
//! - macOS: `~/Library/Application Support/AgentMesh`
//! - Windows: `%LOCALAPPDATA%\AgentMesh`
//! - Linux: `${XDG_DATA_HOME:-~/.local/share}/agentmesh`
//!
//! ```text
//! $AGENTMESH_HOME/
//! ├── workflows/
//! │   └── whatsapp.read_messages.yaml
//! ├── sessions/
//! └── mcp/
//! ```
//!
//! Workflow ids are restricted to filename-safe characters by the IR
//! validator, so ids map 1:1 to file names. Every load re-validates: the
//! store never serves a workflow the validator would reject.

use std::path::PathBuf;

use crate::{TeachError, Workflow, parse_workflow};

/// Directory holding stored workflows, creating it on demand.
///
/// # Errors
///
/// Returns [`TeachError::Storage`] when no home is known or the directory
/// cannot be created.
pub fn workflows_dir() -> Result<PathBuf, TeachError> {
    workflows_dir_in(&home_dir().ok_or_else(|| {
        TeachError::Storage(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no home directory; set AGENTMESH_HOME",
        ))
    })?)
}

/// Directory holding stored workflows under an explicit home (used by tests).
///
/// # Errors
///
/// Returns [`TeachError::Storage`] when the directory cannot be created.
pub fn workflows_dir_in(home: &std::path::Path) -> Result<PathBuf, TeachError> {
    let dir = home.join("workflows");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// `AgentMesh` data home: `AGENTMESH_HOME` wins, otherwise the native user
/// data directory for the current platform. Returns `None` when no suitable
/// environment-backed user directory can be resolved.
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    if let Some(home) = nonempty_env("AGENTMESH_HOME") {
        return Some(PathBuf::from(home));
    }
    default_home_dir_for(
        std::env::consts::OS,
        nonempty_env("HOME").as_deref(),
        nonempty_env("LOCALAPPDATA").as_deref(),
        nonempty_env("APPDATA").as_deref(),
        nonempty_env("XDG_DATA_HOME").as_deref(),
        nonempty_env("USERPROFILE").as_deref(),
    )
}

fn nonempty_env(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

fn default_home_dir_for(
    os: &str,
    home: Option<&std::ffi::OsStr>,
    local_app_data: Option<&std::ffi::OsStr>,
    app_data: Option<&std::ffi::OsStr>,
    xdg_data_home: Option<&std::ffi::OsStr>,
    user_profile: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    match os {
        "macos" => home.map(|path| {
            PathBuf::from(path)
                .join("Library")
                .join("Application Support")
                .join("AgentMesh")
        }),
        "windows" => local_app_data
            .or(app_data)
            .map(|path| PathBuf::from(path).join("AgentMesh"))
            .or_else(|| {
                user_profile.map(|path| {
                    PathBuf::from(path)
                        .join("AppData")
                        .join("Local")
                        .join("AgentMesh")
                })
            }),
        _ => xdg_data_home
            .map(|path| PathBuf::from(path).join("agentmesh"))
            .or_else(|| {
                home.map(|path| {
                    PathBuf::from(path)
                        .join(".local")
                        .join("share")
                        .join("agentmesh")
                })
            }),
    }
}

/// Saves a workflow as `<id>.yaml`, re-validating first.
///
/// # Errors
///
/// Returns [`TeachError`] when the workflow is invalid or cannot be written.
pub fn save_workflow(workflow: &Workflow) -> Result<PathBuf, TeachError> {
    save_workflow_in(workflow, &workflows_dir()?)
}

/// Directory containing locally learned portable capability packages.
///
/// The directory is a sibling of `workflows/`, so it is discoverable by the
/// default AgentMesh capability catalog.
pub fn capabilities_dir() -> Result<PathBuf, TeachError> {
    let home = home_dir().ok_or_else(|| {
        TeachError::Storage(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no home directory; set AGENTMESH_HOME",
        ))
    })?;
    let dir = home.join("capabilities");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Returns the deterministic live-catalog path for a workflow-derived
/// capability package.
pub fn capability_package_path(id: &str, version: &str) -> Result<PathBuf, TeachError> {
    let mut path = capabilities_dir()?;
    path.push("learned");
    for segment in id.split('.') {
        path.push(segment);
    }
    path.push(format!("{version}.yaml"));
    Ok(path)
}

/// Saves a workflow under an explicit home (used by tests).
///
/// A previous revision is snapshotted under `<dir>/revisions/` before it is
/// overwritten, so teaching over an existing capability never loses the last
/// known-good document.
///
/// # Errors
///
/// Returns [`TeachError`] when the workflow is invalid or cannot be written.
pub fn save_workflow_in(workflow: &Workflow, dir: &std::path::Path) -> Result<PathBuf, TeachError> {
    crate::validate_workflow(workflow)?;
    // A workflow becomes a portable capability only once it has an explicit
    // postcondition. Prepare that package before changing the workflow file,
    // so a contract-generation error cannot leave a half-updated definition.
    let package = if workflow.success.is_empty() {
        None
    } else {
        Some(
            crate::compile_workflow_capability(workflow).map_err(|error| {
                TeachError::Validation(format!("capability package generation failed: {error}"))
            })?,
        )
    };
    let path = dir.join(format!("{}.yaml", workflow.id));
    let previous_workflow = std::fs::read_to_string(&path)
        .ok()
        .and_then(|document| crate::parse_workflow(&document).ok());
    if path.is_file() {
        let revisions = dir.join("revisions");
        std::fs::create_dir_all(&revisions)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or_else(|_| "0".to_string(), |elapsed| elapsed.as_secs().to_string());
        let backup = revisions.join(format!("{}.{}.yaml", workflow.id, stamp));
        if let Ok(previous) = std::fs::read(&path) {
            let _ = std::fs::write(&backup, previous);
        }
    }
    let document = serde_yaml::to_string(workflow).map_err(TeachError::Parse)?;
    std::fs::write(&path, document)?;
    if let Some(package) = package {
        let home = dir.parent().unwrap_or(dir);
        let mut package_path = home.join("capabilities").join("learned");
        for segment in workflow.id.split('.') {
            package_path.push(segment);
        }
        package_path.push(format!("{}.yaml", workflow.version));
        let parent = package_path.parent().expect("package path has a parent");
        std::fs::create_dir_all(parent)?;
        if package_path.is_file() {
            // Keep historical packages outside the live catalog tree so
            // catalog discovery cannot accidentally offer stale revisions.
            let mut revisions = home.join("capability-revisions").join("learned");
            for segment in workflow.id.split('.') {
                revisions.push(segment);
            }
            std::fs::create_dir_all(&revisions)?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or_else(|_| "0".to_string(), |elapsed| elapsed.as_secs().to_string());
            let previous = revisions.join(format!("{}.{}.yaml", workflow.version, stamp));
            let _ = std::fs::copy(&package_path, previous);
        }
        let package_document = serde_yaml::to_string(&package).map_err(TeachError::Parse)?;
        std::fs::write(package_path, package_document)?;
    } else if let Some(previous) = previous_workflow {
        remove_generated_package(&previous.id, &previous.version, dir.parent().unwrap_or(dir))?;
    }
    Ok(path)
}

/// Deletes a stored workflow by id, keeping its last revision on disk.
///
/// # Errors
///
/// Returns [`TeachError::NotFound`] when no file exists.
pub fn delete_workflow_in(id: &str, dir: &std::path::Path) -> Result<PathBuf, TeachError> {
    let path = dir.join(format!("{id}.yaml"));
    if !path.is_file() {
        return Err(TeachError::NotFound(id.to_string()));
    }
    let revisions = dir.join("revisions");
    std::fs::create_dir_all(&revisions)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or_else(|_| "0".to_string(), |elapsed| elapsed.as_secs().to_string());
    let previous_workflow = std::fs::read_to_string(&path)
        .ok()
        .and_then(|document| crate::parse_workflow(&document).ok());
    if let Ok(previous) = std::fs::read(&path) {
        let _ = std::fs::write(revisions.join(format!("{id}.{stamp}.yaml")), previous);
    }
    std::fs::remove_file(&path)?;
    if let Some(workflow) = previous_workflow {
        remove_generated_package(&workflow.id, &workflow.version, dir.parent().unwrap_or(dir))?;
    }
    Ok(path)
}

/// Renames a stored workflow, rewriting its `id` to match the new file name.
///
/// # Errors
///
/// Returns [`TeachError::NotFound`] when the source is missing, and
/// [`TeachError::Validation`] when the destination id already exists.
pub fn rename_workflow_in(
    from: &str,
    to: &str,
    dir: &std::path::Path,
) -> Result<PathBuf, TeachError> {
    let source = dir.join(format!("{from}.yaml"));
    if !source.is_file() {
        return Err(TeachError::NotFound(from.to_string()));
    }
    let document = std::fs::read_to_string(&source)?;
    let mut workflow = crate::parse_workflow(&document)?;
    workflow.id = to.to_string();
    crate::validate_workflow(&workflow)?;
    let destination = dir.join(format!("{to}.yaml"));
    if destination.is_file() {
        return Err(TeachError::Validation(format!(
            "cannot rename {from:?} to {to:?}: destination already exists"
        )));
    }
    save_workflow_in(&workflow, dir)?;
    std::fs::remove_file(&source)?;
    if let Ok(previous) = crate::parse_workflow(&document) {
        remove_generated_package(&previous.id, &previous.version, dir.parent().unwrap_or(dir))?;
    }
    Ok(destination)
}

fn remove_generated_package(
    id: &str,
    version: &str,
    home: &std::path::Path,
) -> Result<(), TeachError> {
    let mut package_path = home.join("capabilities").join("learned");
    for segment in id.split('.') {
        package_path.push(segment);
    }
    package_path.push(format!("{version}.yaml"));
    let Ok(document) = std::fs::read_to_string(&package_path) else {
        return Ok(());
    };
    let Ok(package) = serde_yaml::from_str::<agentmesh_protocol::CapabilityPackage>(&document)
    else {
        return Ok(());
    };
    let generated_here = package.implementations.iter().any(|binding| {
        binding.kind == agentmesh_protocol::ImplementationKind::AgentmeshWorkflow
            && binding.reference == format!("workflow://{id}")
    });
    if generated_here {
        let mut revisions = home.join("capability-revisions").join("learned");
        for segment in id.split('.') {
            revisions.push(segment);
        }
        std::fs::create_dir_all(&revisions)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or_else(|_| "0".to_string(), |elapsed| elapsed.as_secs().to_string());
        let archived = revisions.join(format!("{version}.{stamp}.yaml"));
        let _ = std::fs::copy(&package_path, archived);
        std::fs::remove_file(package_path)?;
    }
    Ok(())
}

/// Imports a workflow document from an arbitrary file into the store,
/// optionally overwriting the stored id.
///
/// # Errors
///
/// Returns [`TeachError`] when the document is invalid or the destination
/// exists without `overwrite`.
pub fn import_workflow_in(
    document: &str,
    dir: &std::path::Path,
    overwrite: bool,
) -> Result<PathBuf, TeachError> {
    let workflow = crate::parse_workflow(document)?;
    let destination = dir.join(format!("{}.yaml", workflow.id));
    if destination.is_file() && !overwrite {
        return Err(TeachError::Validation(format!(
            "workflow {:?} already exists; pass overwrite to replace it",
            workflow.id
        )));
    }
    save_workflow_in(&workflow, dir)
}

/// Loads and re-validates a workflow by id.
///
/// # Errors
///
/// Returns [`TeachError::NotFound`] when no file exists, and
/// [`TeachError::Parse`] or [`TeachError::Validation`] when the stored
/// document is corrupt.
pub fn load_workflow(id: &str) -> Result<Workflow, TeachError> {
    load_workflow_in(id, &workflows_dir()?)
}

/// Loads and re-validates a workflow by id under an explicit home.
///
/// # Errors
///
/// Returns [`TeachError::NotFound`] when no file exists.
pub fn load_workflow_in(id: &str, dir: &std::path::Path) -> Result<Workflow, TeachError> {
    let path = dir.join(format!("{id}.yaml"));
    if !path.is_file() {
        return Err(TeachError::NotFound(id.to_string()));
    }
    let document = std::fs::read_to_string(&path)?;
    parse_workflow(&document)
}

/// Lists every stored workflow that parses and validates, ordered by id.
///
/// # Errors
///
/// Returns [`TeachError`] when the store is unreadable or a stored document
/// fails validation (fail-closed: a corrupt store is reported, not hidden).
pub fn list_workflows() -> Result<Vec<WorkflowSummary>, TeachError> {
    list_workflows_in(&workflows_dir()?)
}

/// Deletes a stored workflow from the default home (snapshots a revision).
///
/// # Errors
///
/// Returns [`TeachError`] when the workflow is missing or cannot be removed.
pub fn delete_workflow(id: &str) -> Result<PathBuf, TeachError> {
    delete_workflow_in(id, &workflows_dir()?)
}

/// Renames a stored workflow in the default home.
///
/// # Errors
///
/// Returns [`TeachError`] when the source is missing or the destination exists.
pub fn rename_workflow(from: &str, to: &str) -> Result<PathBuf, TeachError> {
    rename_workflow_in(from, to, &workflows_dir()?)
}

/// One stored document that failed to parse or validate during a lenient
/// listing: reported, never hidden, but it no longer blocks the rest.
#[derive(Debug, Clone)]
pub struct WorkflowListError {
    /// File that failed.
    pub file: String,
    /// Human-readable reason.
    pub error: String,
}

/// Lists stored workflows, reporting corrupt documents instead of failing.
///
/// The strict [`list_workflows_in`] stays fail-closed for flows that need it;
/// interactive `list --lenient` uses this so one broken YAML cannot hide a
/// whole taught library.
#[must_use]
pub fn list_workflows_lenient(
    dir: &std::path::Path,
) -> (Vec<WorkflowSummary>, Vec<WorkflowListError>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "yaml"))
        .collect();
    entries.sort();
    let mut summaries = Vec::new();
    let mut errors = Vec::new();
    for path in entries {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|document| {
                crate::parse_workflow(&document).map_err(|error| error.to_string())
            }) {
            Ok(workflow) => summaries.push(WorkflowSummary {
                id: workflow.id.clone(),
                description: workflow.description.clone(),
                runtime: workflow.runtime.clone(),
                steps: workflow.steps.len(),
            }),
            Err(error) => errors.push(WorkflowListError { file: name, error }),
        }
    }
    (summaries, errors)
}

/// Lists stored workflows under an explicit home (used by tests).
///
/// # Errors
///
/// Returns [`TeachError`] when the store is unreadable or a document fails.
pub fn list_workflows_in(dir: &std::path::Path) -> Result<Vec<WorkflowSummary>, TeachError> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "yaml"))
        .collect();
    entries.sort();
    let mut summaries = Vec::with_capacity(entries.len());
    for path in entries {
        let document = std::fs::read_to_string(&path)?;
        let workflow = parse_workflow(&document)?;
        summaries.push(WorkflowSummary {
            id: workflow.id.clone(),
            description: workflow.description.clone(),
            runtime: workflow.runtime.clone(),
            steps: workflow.steps.len(),
        });
    }
    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InputDef, InputType, Step, WorkflowAssertion};
    use std::collections::BTreeMap;
    use std::ffi::OsStr;

    #[test]
    fn platform_data_homes_follow_native_conventions() {
        assert_eq!(
            default_home_dir_for(
                "macos",
                Some(OsStr::new("/Users/demo")),
                None,
                None,
                None,
                None,
            ),
            Some(PathBuf::from(
                "/Users/demo/Library/Application Support/AgentMesh"
            ))
        );
        assert_eq!(
            default_home_dir_for(
                "windows",
                None,
                Some(OsStr::new(r"C:\Users\demo\AppData\Local")),
                None,
                None,
                None,
            ),
            Some(PathBuf::from(r"C:\Users\demo\AppData\Local").join("AgentMesh"))
        );
        assert_eq!(
            default_home_dir_for(
                "linux",
                Some(OsStr::new("/home/demo")),
                None,
                None,
                Some(OsStr::new("/srv/user-data")),
                None,
            ),
            Some(PathBuf::from("/srv/user-data/agentmesh"))
        );
        assert_eq!(
            default_home_dir_for(
                "linux",
                Some(OsStr::new("/home/demo")),
                None,
                None,
                None,
                None,
            ),
            Some(PathBuf::from("/home/demo/.local/share/agentmesh"))
        );
    }

    fn sample(id: &str) -> Workflow {
        Workflow {
            version: crate::SUPPORTED_IR_VERSION.to_string(),
            id: id.to_string(),
            description: format!("Sample {id}"),
            runtime: "browser".to_string(),
            inputs: [(
                "contact".to_string(),
                InputDef {
                    input_type: InputType::String,
                    required: Some(true),
                    default: None,
                },
            )]
            .into_iter()
            .collect(),
            steps: vec![Step {
                id: "open".to_string(),
                op: "browser.navigate".to_string(),
                target: None,
                value: None,
                url: Some("https://example.com".to_string()),
                limit: None,
                timeout_ms: None,
                condition: None,
                iterations: None,
                destination: None,
                path: None,
            }],
            outputs: BTreeMap::default(),
            policy: None,
            preconditions: Vec::new(),
            success: Vec::new(),
            failure: Vec::new(),
            recovery: None,
            observed_apis: Vec::new(),
        }
    }

    #[test]
    fn roundtrip_save_load_list() {
        let home = tempfile::tempdir().expect("temp home");
        let dir = workflows_dir_in(home.path()).expect("workflows dir");
        save_workflow_in(&sample("app.alpha"), &dir).expect("save alpha");
        save_workflow_in(&sample("app.beta"), &dir).expect("save beta");
        let loaded = load_workflow_in("app.alpha", &dir).expect("load alpha");
        assert_eq!(loaded.description, "Sample app.alpha");
        let listed = list_workflows_in(&dir).expect("list");
        let ids: Vec<_> = listed.iter().map(|summary| summary.id.clone()).collect();
        assert_eq!(ids, vec!["app.alpha", "app.beta"]);
        assert_eq!(listed[0].steps, 1);
    }

    #[test]
    fn verified_workflow_is_published_and_removed_from_live_catalog_on_delete() {
        let home = tempfile::tempdir().expect("temp home");
        let dir = workflows_dir_in(home.path()).expect("workflows dir");
        let mut workflow = sample("app.receipt");
        workflow.success.push(WorkflowAssertion {
            op: "assert.url".into(),
            target: None,
            value: Some("/receipts/".into()),
            url: None,
            timeout_ms: None,
        });

        save_workflow_in(&workflow, &dir).expect("save verified workflow");
        let package_path = home
            .path()
            .join("capabilities/learned/app/receipt/1.0.yaml");
        let package: agentmesh_protocol::CapabilityPackage = serde_yaml::from_str(
            &std::fs::read_to_string(&package_path).expect("generated capability package"),
        )
        .expect("valid generated package");
        assert_eq!(package.capability.id, "app.receipt");
        assert_eq!(package.contract.success_evidence.len(), 1);

        delete_workflow_in("app.receipt", &dir).expect("delete workflow");
        assert!(!package_path.exists());
        assert!(
            home.path()
                .join("capability-revisions/learned/app/receipt")
                .is_dir()
        );
    }

    #[test]
    fn load_missing_is_not_found() {
        let home = tempfile::tempdir().expect("temp home");
        let dir = workflows_dir_in(home.path()).expect("workflows dir");
        let error = load_workflow_in("app.ghost", &dir).expect_err("missing workflow");
        assert!(matches!(error, TeachError::NotFound(_)));
    }

    #[test]
    fn corrupt_store_fails_list_closed() {
        let home = tempfile::tempdir().expect("temp home");
        let dir = workflows_dir_in(home.path()).expect("workflows dir");
        save_workflow_in(&sample("app.good"), &dir).expect("save good");
        std::fs::write(dir.join("broken.yaml"), "not: [valid").expect("corrupt file");
        let error = list_workflows_in(&dir).expect_err("corrupt store fails");
        assert!(matches!(error, TeachError::Parse(_)), "{error:?}");
    }

    #[test]
    fn lenient_list_reports_corrupt_files_without_hiding_good_ones() {
        let home = tempfile::tempdir().expect("temp home");
        let dir = workflows_dir_in(home.path()).expect("workflows dir");
        save_workflow_in(&sample("app.good"), &dir).expect("save good");
        std::fs::write(dir.join("broken.yaml"), "not: [valid").expect("corrupt file");
        let (summaries, errors) = list_workflows_lenient(&dir);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "app.good");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "broken.yaml");
    }

    #[test]
    fn overwrite_snapshots_a_revision_and_delete_rename_work() {
        let home = tempfile::tempdir().expect("temp home");
        let dir = workflows_dir_in(home.path()).expect("workflows dir");
        save_workflow_in(&sample("app.alpha"), &dir).expect("save alpha");
        save_workflow_in(&sample("app.alpha"), &dir).expect("overwrite alpha");
        let revisions: Vec<_> = std::fs::read_dir(dir.join("revisions"))
            .expect("revisions dir")
            .filter_map(Result::ok)
            .collect();
        assert_eq!(revisions.len(), 1);
        rename_workflow_in("app.alpha", "app.beta", &dir).expect("rename");
        assert!(dir.join("app.beta.yaml").is_file());
        assert!(!dir.join("app.alpha.yaml").exists());
        delete_workflow_in("app.beta", &dir).expect("delete");
        assert!(!dir.join("app.beta.yaml").exists());
        assert!(matches!(
            delete_workflow_in("app.beta", &dir).expect_err("missing"),
            TeachError::NotFound(_)
        ));
    }

    #[test]
    fn import_rejects_overwrite_without_flag() {
        let home = tempfile::tempdir().expect("temp home");
        let dir = workflows_dir_in(home.path()).expect("workflows dir");
        save_workflow_in(&sample("app.alpha"), &dir).expect("save alpha");
        let document = std::fs::read_to_string(dir.join("app.alpha.yaml")).expect("read");
        let error = import_workflow_in(&document, &dir, false).expect_err("overwrite guarded");
        assert!(matches!(error, TeachError::Validation(_)), "{error:?}");
        import_workflow_in(&document, &dir, true).expect("overwrite allowed");
    }
}

/// Row shown by `agentmesh workflows list`.
#[derive(Debug, Clone)]
pub struct WorkflowSummary {
    /// Workflow id.
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// Preferred runtime.
    pub runtime: String,
    /// Step count.
    pub steps: usize,
}
