//! Local commands for portable capability package files.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use agentmesh_protocol::{CapabilityPackage, discover_capabilities};
use anyhow::{Context, Result, bail};

use crate::CapabilityAction;

const MAX_SEARCH_LIMIT: usize = 100;
const MAX_CATALOG_DEPTH: usize = 8;

pub(super) fn run(action: CapabilityAction) -> Result<()> {
    match action {
        CapabilityAction::Validate { file } => {
            let package = read_package(&file)?;
            validate_package(&package)?;
            println!(
                "valid: {}@{}",
                package.capability.id, package.capability.version
            );
            Ok(())
        }
        CapabilityAction::Inspect { file } => {
            let package = read_package(&file)?;
            validate_package(&package)?;
            let digest = package.content_digest()?;
            let digest_status = if package.provenance.is_some() {
                if package.verify_content_digest() {
                    "verified"
                } else {
                    "mismatch"
                }
            } else {
                "not_declared"
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "package": package,
                    "computedDigest": digest,
                    "declaredDigestStatus": digest_status
                }))?
            );
            Ok(())
        }
        CapabilityAction::Search {
            catalog,
            query,
            limit,
        } => {
            if limit == 0 || limit > MAX_SEARCH_LIMIT {
                bail!("limit must be between 1 and {MAX_SEARCH_LIMIT}");
            }
            let packages = load_catalog(&catalog)?;
            let matches = discover_capabilities(&query, &packages, limit);
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &matches
                        .iter()
                        .map(|result| {
                            serde_json::json!({
                                "id": result.package.capability.id,
                                "version": result.package.capability.version,
                                "intent": result.package.capability.intent,
                                "score": result.score,
                                "matchedTerms": result.matched_terms
                            })
                        })
                        .collect::<Vec<_>>()
                )?
            );
            Ok(())
        }
        CapabilityAction::Install { file, catalog } => {
            let package = read_package(&file)?;
            validate_package(&package)?;
            let mut destination = catalog;
            for segment in package.capability.id.split('.') {
                destination.push(segment);
            }
            destination.push(format!("{}.yaml", package.capability.version));
            let parent = destination
                .parent()
                .context("capability install destination has no parent")?;
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "could not create capability catalog at {}",
                    parent.display()
                )
            })?;
            let bytes = serde_yaml::to_string(&package)?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
                .with_context(|| {
                    format!(
                        "could not install package at {}; it may already exist",
                        destination.display()
                    )
                })?;
            output.write_all(bytes.as_bytes())?;
            output.sync_all()?;
            println!(
                "installed {}@{} at {}",
                package.capability.id,
                package.capability.version,
                destination.display()
            );
            Ok(())
        }
    }
}

fn read_package(path: &Path) -> Result<CapabilityPackage> {
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let package = match path.extension().and_then(|extension| extension.to_str()) {
        Some("json") => serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid capability JSON in {}", path.display()))?,
        _ => serde_yaml::from_slice(&bytes)
            .with_context(|| format!("invalid capability YAML in {}", path.display()))?,
    };
    Ok(package)
}

fn validate_package(package: &CapabilityPackage) -> Result<()> {
    package
        .validate()
        .context("capability package validation failed")?;
    if package.provenance.is_some() && !package.verify_content_digest() {
        bail!("declared package digest does not match package content");
    }
    Ok(())
}

pub(super) fn load_catalog(root: &Path) -> Result<Vec<CapabilityPackage>> {
    let mut files = Vec::new();
    collect_package_files(root, 0, &mut files)?;
    files.sort();
    files
        .into_iter()
        .map(|file| {
            let package = read_package(&file)?;
            validate_package(&package)?;
            Ok(package)
        })
        .collect()
}

pub(super) fn catalog_directory() -> PathBuf {
    std::env::var_os("AGENTMESH_CAPABILITY_CATALOG")
        .map(PathBuf::from)
        .or_else(|| agentmesh_teach::home_dir().map(|home| home.join("capabilities")))
        .unwrap_or_else(|| PathBuf::from(".agentmesh/capabilities"))
}

fn collect_package_files(
    root: &Path,
    depth: usize,
    files: &mut Vec<std::path::PathBuf>,
) -> Result<()> {
    if depth > MAX_CATALOG_DEPTH {
        bail!("capability catalog nesting exceeds its safety limit");
    }
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("could not read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_package_files(&path, depth + 1, files)?;
        } else if file_type.is_file()
            && matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("yaml" | "yml" | "json")
            )
        {
            files.push(path);
        }
    }
    Ok(())
}
