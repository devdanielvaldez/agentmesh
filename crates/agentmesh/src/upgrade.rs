//! Self-update: latest-release discovery, cached update notices, and the
//! `upgrade` installer.
//!
//! Update checks cost nothing on most runs: [`refresh_notice`] reads the
//! on-disk cache, and only queries the network when the cache is older than
//! a day — bounded by a short timeout, throttled after failures, and always
//! silent. [`run_upgrade`] performs the foreground download, checksum
//! verification, extraction, and install.

use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// GitHub repository queried for releases; tests override it.
const DEFAULT_REPO: &str = "devdanielvaldez/agentmesh";
/// GitHub API base; tests point it at a local stub.
const DEFAULT_API_BASE: &str = "https://api.github.com";
/// Seconds between background update checks.
const CHECK_INTERVAL_SECS: u64 = 24 * 3_600;
/// Network deadline for the passive update check: one slow day-old refresh
/// must never feel like a hang.
const SHORT_TIMEOUT: Duration = Duration::from_secs(2);
/// Network deadline for release metadata and downloads.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(30);
/// Cache file holding the last observed release.
const CACHE_FILE: &str = "update-check.json";

/// Release metadata returned by the GitHub releases API.
#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

/// One release artifact with its download URL.
#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Cached update-check state.
#[derive(Debug, Serialize, Deserialize)]
struct UpdateCache {
    checked_at_secs: u64,
    latest_tag: String,
}

/// Returns an update notice, refreshing the cache first when it is missing
/// or stale. The network query is bounded by [`SHORT_TIMEOUT`]; failures
/// (including timeouts) are silent and throttle the next attempt for a day
/// so offline machines never pay the wait twice.
pub async fn refresh_notice() -> Option<String> {
    let cache = cache_path();
    if !cache_fresh(&cache, now_secs()) {
        refresh_cache(&cache).await;
    }
    pending_notice()
}

/// Returns an update notice when the fresh cache names a newer release.
#[must_use]
pub fn pending_notice() -> Option<String> {
    let cache = read_cache(&cache_path())?;
    let latest = parse_version(&cache.latest_tag)?;
    let current = parse_version(concat!("v", env!("CARGO_PKG_VERSION")))?;
    if is_newer(current, latest) {
        Some(format!(
            "Update available: {} -> {}. Run `agentmesh upgrade` to install it.",
            env!("CARGO_PKG_VERSION"),
            cache.latest_tag,
        ))
    } else {
        None
    }
}

/// Runs the `upgrade` command: `--check` reports only, otherwise the latest
/// binary is downloaded, verified, extracted, and installed.
///
/// # Errors
///
/// Returns an error when the release cannot be resolved, the platform is
/// unsupported, verification fails, or the install cannot complete.
pub async fn run_upgrade(check: bool, yes: bool, to: Option<PathBuf>) -> Result<()> {
    let release = fetch_latest().await?;
    let latest = parse_version(&release.tag_name)
        .with_context(|| format!("release {} is not a version tag", release.tag_name))?;
    let current =
        parse_version(concat!("v", env!("CARGO_PKG_VERSION"))).context("own version is invalid")?;
    if !is_newer(current, latest) {
        println!(
            "agentmesh {} is already the latest release.",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    let (target, extension) = platform_target().context(
        "no prebuilt binary for this platform; install with install.sh or cargo instead",
    )?;
    let file_name = asset_name(&release.tag_name, target, extension);
    println!("Latest release: {}", release.tag_name);
    if check {
        println!("Run `agentmesh upgrade` to download and install {file_name}.");
        return Ok(());
    }
    if !yes && !confirm(&release.tag_name)? {
        println!("Upgrade cancelled.");
        return Ok(());
    }
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == file_name)
        .with_context(|| format!("release {} has no {file_name}", release.tag_name))?;
    let workdir = stagedir()?;
    let archive = download(&asset.browser_download_url, &workdir, &file_name).await?;
    let checksum_url = format!("{}.sha256", asset.browser_download_url);
    let checksum_file = format!("{file_name}.sha256");
    let checksum_path = download(&checksum_url, &workdir, &checksum_file).await?;
    verify_checksum(&archive, &checksum_path, &file_name)?;
    let binary = extract_binary(&archive, &workdir, extension)?;
    sanity_check(&binary, &release.tag_name)?;
    install_binary(&binary, to.as_deref())?;
    let _ = std::fs::remove_dir_all(&workdir);
    println!("Upgraded to {}.", release.tag_name);
    Ok(())
}

/// Fetches the latest release for the configured repository.
async fn fetch_latest() -> Result<GithubRelease> {
    let repo = std::env::var("AGENTMESH_UPDATE_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string());
    let api =
        std::env::var("AGENTMESH_GITHUB_API").unwrap_or_else(|_| DEFAULT_API_BASE.to_string());
    let url = format!("{}/repos/{repo}/releases/latest", api.trim_end_matches('/'));
    let response = reqwest::Client::builder()
        .timeout(NETWORK_TIMEOUT)
        .user_agent(format!("agentmesh/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build update client")?
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .with_context(|| format!("failed to query {url}"))?;
    if !response.status().is_success() {
        anyhow::bail!("release query returned {}", response.status());
    }
    response
        .json::<GithubRelease>()
        .await
        .context("latest release is not valid JSON")
}

/// Asks for confirmation on an interactive terminal; scripts must pass --yes.
fn confirm(tag: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("not a terminal; rerun with --yes to upgrade to {tag} non-interactively");
    }
    eprint!(
        "Upgrade agentmesh {} -> {tag}? [y/N] ",
        env!("CARGO_PKG_VERSION")
    );
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("failed to read confirmation")?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Downloads a URL into the workdir and returns its path.
async fn download(url: &str, workdir: &Path, file_name: &str) -> Result<PathBuf> {
    let destination = workdir.join(file_name);
    let mut response = reqwest::Client::builder()
        .timeout(NETWORK_TIMEOUT)
        .user_agent(format!("agentmesh/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build download client")?
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?;
    if !response.status().is_success() {
        anyhow::bail!("download returned {}", response.status());
    }
    let mut file = std::fs::File::create(&destination).context("failed to create download file")?;
    while let Some(chunk) = response
        .chunk()
        .await
        .context("failed to stream download")?
    {
        std::io::copy(&mut chunk.as_ref(), &mut file).context("failed to write download")?;
    }
    Ok(destination)
}

/// Verifies the archive against its `.sha256` sidecar (`"<hex>  <name>"`).
fn verify_checksum(archive: &Path, checksum_path: &Path, file_name: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let sidecar = std::fs::read_to_string(checksum_path).context("failed to read checksum file")?;
    let expected = sidecar
        .split_whitespace()
        .next()
        .context("checksum file is empty")?
        .to_lowercase();
    let bytes = std::fs::read(archive).context("failed to read download for hashing")?;
    let actual = to_hex(&Sha256::digest(&bytes));
    if actual != expected {
        anyhow::bail!("checksum mismatch for {file_name}; refusing to install");
    }
    Ok(())
}

/// Lowercase hex without extra dependencies.
fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0F) as usize] as char);
    }
    out
}

/// Maps the running platform to a release target triple and archive format.
/// Returns `None` where no prebuilt binary ships.
fn platform_target() -> Option<(&'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some(("aarch64-apple-darwin", "tar.gz")),
        ("macos", "x86_64") => Some(("x86_64-apple-darwin", "tar.gz")),
        ("linux", "x86_64") => Some(("x86_64-unknown-linux-gnu", "tar.gz")),
        ("windows", "x86_64") => Some(("x86_64-pc-windows-msvc", "zip")),
        _ => None,
    }
}

/// Release asset file name for a tag, target triple, and archive format.
fn asset_name(tag: &str, target: &str, extension: &str) -> String {
    format!("agentmesh-{tag}-{target}.{extension}")
}

/// Parses `vMAJOR.MINOR.PATCH` (leading `v` optional) into a triplet.
fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let mut parts = tag.strip_prefix('v').unwrap_or(tag).split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Returns true when the latest release is strictly newer.
fn is_newer(current: (u64, u64, u64), latest: (u64, u64, u64)) -> bool {
    latest > current
}

/// Extracts the `agentmesh` binary from the downloaded archive with platform
/// tools (`tar` on Unix, `Expand-Archive` on Windows) and returns its path.
fn extract_binary(archive: &Path, workdir: &Path, extension: &str) -> Result<PathBuf> {
    let outdir = workdir.join("unpacked");
    std::fs::create_dir_all(&outdir).context("failed to create unpack dir")?;
    if extension == "zip" {
        let status = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Expand-Archive",
                "-Force",
                "-Path",
            ])
            .arg(archive)
            .arg("-DestinationPath")
            .arg(&outdir)
            .status()
            .context("failed to run Expand-Archive")?;
        if !status.success() {
            anyhow::bail!("Expand-Archive failed to unpack the release");
        }
    } else {
        let status = std::process::Command::new("tar")
            .args(["-xzf"])
            .arg(archive)
            .args(["-C"])
            .arg(&outdir)
            .status()
            .context("failed to run tar")?;
        if !status.success() {
            anyhow::bail!("tar failed to unpack the release");
        }
    }
    let exe = if cfg!(windows) {
        "agentmesh.exe"
    } else {
        "agentmesh"
    };
    let binary = outdir.join(exe);
    if !binary.is_file() {
        anyhow::bail!("release archive does not contain {exe}");
    }
    Ok(binary)
}

/// Runs the extracted binary with `--version` and requires the new tag.
fn sanity_check(binary: &Path, tag: &str) -> Result<()> {
    let output = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .context("downloaded binary does not run")?;
    if !output.status.success() {
        anyhow::bail!("downloaded binary failed its self-check");
    }
    let version = String::from_utf8_lossy(&output.stdout);
    if !version.contains(tag.trim_start_matches('v')) {
        anyhow::bail!("downloaded binary reports {version:?}, expected {tag}");
    }
    Ok(())
}

/// Installs the verified binary: in place by default, or into `--to`.
/// On Windows the running executable is first renamed aside (deleting or
/// overwriting a running image is refused, renaming is allowed).
fn install_binary(binary: &Path, to: Option<&Path>) -> Result<PathBuf> {
    let destination = match to {
        Some(dir) => {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
            dir.join(exe_name())
        }
        None => std::env::current_exe().context("failed to locate the running binary")?,
    };
    if cfg!(windows) && destination.exists() {
        let backup = destination.with_extension("old");
        let _ = std::fs::remove_file(&backup);
        std::fs::rename(&destination, &backup)
            .with_context(|| format!("failed to move aside {}", destination.display()))?;
    }
    std::fs::copy(binary, &destination)
        .with_context(|| format!("failed to install to {}", destination.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o755))
            .context("failed to mark the binary executable")?;
    }
    Ok(destination)
}

/// Binary file name for the running platform.
fn exe_name() -> &'static str {
    if cfg!(windows) {
        "agentmesh.exe"
    } else {
        "agentmesh"
    }
}

/// Fresh staging directory for one upgrade run.
fn stagedir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("agentmesh-upgrade-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).context("failed to clear stale staging dir")?;
    }
    std::fs::create_dir_all(&dir).context("failed to create staging dir")?;
    Ok(dir)
}

/// Seconds since the Unix epoch; zero on clock failure.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Cache directory: `XDG_CACHE_HOME`, then `~/.cache` (Unix) or
/// `%LOCALAPPDATA%` (Windows). Returns `None` when no home is known.
fn cache_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME") {
        if !dir.trim().is_empty() {
            return Some(PathBuf::from(dir).join("agentmesh"));
        }
    }
    if cfg!(windows) {
        return std::env::var("LOCALAPPDATA")
            .ok()
            .map(|dir| PathBuf::from(dir).join("agentmesh"));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".cache").join("agentmesh"))
}

/// Cache file path, or an unsatisfiable path when no home is known.
fn cache_path() -> PathBuf {
    cache_dir()
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
        .join(CACHE_FILE)
}

/// Returns true when the cache exists and was refreshed within the interval.
fn cache_fresh(cache: &Path, now: u64) -> bool {
    read_cache(cache)
        .is_some_and(|state| now.saturating_sub(state.checked_at_secs) < CHECK_INTERVAL_SECS)
}

/// Reads and validates the cache; corrupt files read as missing.
fn read_cache(cache: &Path) -> Option<UpdateCache> {
    let bytes = std::fs::read(cache).ok()?;
    let state: UpdateCache = serde_json::from_slice(&bytes).ok()?;
    parse_version(&state.latest_tag)?;
    Some(state)
}

/// Queries the latest release and stores it. On any failure the previous tag
/// (or the running version) is kept with a fresh timestamp, throttling the
/// next attempt for another day instead of retrying on every invocation.
async fn refresh_cache(cache: &Path) {
    let previous = read_cache(cache).map(|state| state.latest_tag);
    let latest = tokio::time::timeout(SHORT_TIMEOUT, fetch_latest())
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(|release| parse_version(&release.tag_name).map(|_| release.tag_name))
        .or(previous)
        .unwrap_or_else(|| concat!("v", env!("CARGO_PKG_VERSION")).to_string());
    if let Some(parent) = cache.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let state = UpdateCache {
        checked_at_secs: now_secs(),
        latest_tag: latest,
    };
    if let Ok(bytes) = serde_json::to_vec(&state) {
        let _ = std::fs::write(cache, bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("1.10.3"), Some((1, 10, 3)));
        assert_eq!(parse_version("v0.2"), None);
        assert_eq!(parse_version("v0.2.0.1"), None);
        assert_eq!(parse_version("latest"), None);
        assert!(is_newer((0, 1, 0), (0, 2, 0)));
        assert!(is_newer((0, 2, 0), (1, 0, 0)));
        assert!(!is_newer((0, 2, 0), (0, 2, 0)));
        assert!(!is_newer((0, 2, 0), (0, 1, 9)));
    }

    #[test]
    fn asset_names_match_the_release_layout() {
        assert_eq!(
            asset_name("v0.2.0", "aarch64-apple-darwin", "tar.gz"),
            "agentmesh-v0.2.0-aarch64-apple-darwin.tar.gz"
        );
        assert_eq!(
            asset_name("v0.2.0", "x86_64-pc-windows-msvc", "zip"),
            "agentmesh-v0.2.0-x86_64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn corrupt_or_stale_cache_reads_as_missing() {
        let dir = std::env::temp_dir().join(format!("agentmesh-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test cache dir");
        let cache = dir.join(CACHE_FILE);
        assert!(!cache_fresh(&cache, now_secs()));
        std::fs::write(&cache, "not json").expect("corrupt cache");
        assert!(read_cache(&cache).is_none());
        let fresh = UpdateCache {
            checked_at_secs: now_secs(),
            latest_tag: "v9.9.9".to_string(),
        };
        std::fs::write(
            &cache,
            serde_json::to_vec(&fresh).expect("cache serializes"),
        )
        .expect("fresh cache");
        assert!(cache_fresh(&cache, now_secs()));
        assert!(!cache_fresh(&cache, now_secs() + CHECK_INTERVAL_SECS + 1));
        assert!(read_cache(&cache).expect("fresh cache").latest_tag == "v9.9.9");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checksum_verification_accepts_and_rejects() {
        let dir = std::env::temp_dir().join(format!("agentmesh-sha-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test hash dir");
        let archive = dir.join("agentmesh-v9.9.9-test.tar.gz");
        std::fs::write(&archive, b"payload").expect("test payload");
        let good = dir.join("good.sha256");
        std::fs::write(
            &good,
            "239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5  agentmesh-v9.9.9-test.tar.gz\n",
        )
        .expect("good sidecar");
        verify_checksum(&archive, &good, "agentmesh-v9.9.9-test.tar.gz").expect("good checksum");
        let bad = dir.join("bad.sha256");
        std::fs::write(&bad, "00  agentmesh-v9.9.9-test.tar.gz\n").expect("bad sidecar");
        verify_checksum(&archive, &bad, "agentmesh-v9.9.9-test.tar.gz").expect_err("bad checksum");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
