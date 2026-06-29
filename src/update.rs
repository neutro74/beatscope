//! Self-update: fetch the latest GitHub release and replace the running binary
//! in place. Triggered with `--update`.

use std::io::Read;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

const REPO: &str = "neutro74/beatscope";
const ASSET: &str = "beatscope-x86_64-linux.tar.gz";

/// Check the latest release and, if it's newer than the running build (or
/// `force` is set), download it and replace this executable.
pub fn run(force: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!("beatscope {current} — checking for updates...");

    let tag = fetch_latest_tag().context("could not check the latest release")?;
    let latest = tag.trim_start_matches('v');

    if !force && !is_newer(latest, current) {
        println!("Already up to date ({current}).");
        return Ok(());
    }

    println!("Downloading {latest}...");
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/{ASSET}");
    let bytes = download(&url).with_context(|| format!("download failed: {url}"))?;

    let exe = std::env::current_exe().context("locating the current executable")?;
    install_binary(&exe, &bytes)
        .with_context(|| format!("installing the new binary to {}", exe.display()))?;

    println!("Updated {current} -> {latest}. Restart beatscope to use it.");
    Ok(())
}

/// Read `tag_name` from the GitHub "latest release" API.
fn fetch_latest_tag() -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = ureq::get(&url)
        .header("User-Agent", "beatscope-self-update")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| anyhow!(e.to_string()))?;
    let mut body = String::new();
    resp.into_body()
        .into_reader()
        .take(1024 * 1024)
        .read_to_string(&mut body)?;
    let v: serde_json::Value = serde_json::from_str(&body)?;
    v.get("tag_name")
        .and_then(|t| t.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("no tag_name in the release response"))
}

fn download(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .header("User-Agent", "beatscope-self-update")
        .call()
        .map_err(|e| anyhow!(e.to_string()))?;
    let mut buf = Vec::new();
    resp.into_body()
        .into_reader()
        .take(128 * 1024 * 1024)
        .read_to_end(&mut buf)?;
    if buf.len() < 1024 {
        bail!("downloaded file is too small ({} bytes)", buf.len());
    }
    Ok(buf)
}

/// Extract the `beatscope` binary from the tarball and atomically swap it in for
/// the current executable (same-directory rename, so a running process is fine).
fn install_binary(exe: &Path, tarball: &[u8]) -> Result<()> {
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow!("executable has no parent directory"))?;

    // Work in a temp dir alongside the executable so the final rename stays on
    // the same filesystem (and never crosses into /tmp on another mount).
    let work = dir.join(format!(".beatscope-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)
        .with_context(|| format!("the install directory {} is not writable", dir.display()))?;
    let guard = DirGuard(work.clone());

    let tar_path = work.join("download.tar.gz");
    std::fs::write(&tar_path, tarball)?;

    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&tar_path)
        .arg("-C")
        .arg(&work)
        .status()
        .context("running `tar` (is it installed?)")?;
    if !status.success() {
        bail!("tar failed to extract the release archive");
    }

    let new_bin = work.join("beatscope");
    if !new_bin.exists() {
        bail!("the release archive did not contain a `beatscope` binary");
    }
    set_executable(&new_bin)?;

    // Stage next to the target, then atomically rename over it.
    let staged = exe.with_extension("new");
    let _ = std::fs::remove_file(&staged);
    std::fs::rename(&new_bin, &staged)
        .or_else(|_| std::fs::copy(&new_bin, &staged).map(|_| ()))?;
    set_executable(&staged)?;
    std::fs::rename(&staged, exe).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        anyhow!("could not replace {}: {e}", exe.display())
    })?;

    drop(guard);
    Ok(())
}

fn set_executable(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(p)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(p, perms)?;
    Ok(())
}

/// Compare dotted numeric versions ("1.2.0"); returns true if `candidate` is
/// strictly greater than `current`.
fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| p.trim().chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parse(candidate), parse(current));
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

/// Cleans up the temp working directory on drop.
struct DirGuard(std::path::PathBuf);
impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn version_ordering() {
        assert!(is_newer("1.0.1", "1.0.0"));
        assert!(is_newer("1.1.0", "1.0.9"));
        assert!(is_newer("2.0.0", "1.9.9"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.0.1"));
        assert!(is_newer("1.2", "1.1.9"));
    }
}
