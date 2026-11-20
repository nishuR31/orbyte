//! Self-update: checks GitHub Releases and re-runs the platform installer.
//!
//! Deliberately explicit — orbyte never updates itself in the background.
//! `orb self-update --check` only reports the version situation; the plain
//! command downloads the latest archive and replaces the installed binary.

use std::process::Command;

const DEFAULT_REPO: &str = "nishuR31/orbyte";
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn command_self_update(args: &[String]) -> i32 {
    let check_only = args.iter().any(|arg| arg == "--check");
    for arg in args {
        if arg != "--check" {
            eprintln!("error: unknown flag for self-update: {arg}");
            return 2;
        }
    }
    let repo = std::env::var("ORB_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_owned());

    let latest = match latest_version(&repo) {
        Ok(version) => version,
        Err(error) => {
            eprintln!("error: could not check the latest release: {error}");
            eprintln!("hint: no release may be published yet; re-run scripts/install.sh (or install.ps1), or build from source");
            return 1;
        }
    };
    let available = version_newer(&latest, VERSION);

    if check_only {
        if available {
            println!("update available: {VERSION} -> {latest}");
            println!("run `orb self-update` to install it");
        } else {
            println!("orb {VERSION} is up to date (latest release: {latest}).");
        }
        return 0;
    }
    if !available {
        println!("orb {VERSION} is up to date (latest release: {latest}).");
        return 0;
    }

    println!("updating orb {VERSION} -> {latest} ...");
    let (program, mut command_args) = if cfg!(windows) {
        (
            "powershell".to_owned(),
            vec![
                "-NoProfile".to_owned(),
                "-ExecutionPolicy".to_owned(),
                "Bypass".to_owned(),
                "-Command".to_owned(),
            ],
        )
    } else {
        ("sh".to_owned(), vec!["-c".to_owned()])
    };
    let script = if cfg!(windows) {
        format!("irm https://raw.githubusercontent.com/{repo}/main/scripts/install.ps1 | iex")
    } else {
        format!("curl -fsSL https://raw.githubusercontent.com/{repo}/main/scripts/install.sh | bash")
    };
    command_args.push(script);

    match Command::new(program).args(&command_args).status() {
        Ok(status) if status.success() => {
            println!("update complete. Run `orb --version` to verify.");
            0
        }
        Ok(status) => {
            eprintln!("error: installer exited with {status}");
            1
        }
        Err(error) => {
            eprintln!("error: could not run installer: {error}");
            1
        }
    }
}

fn latest_version(repo: &str) -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let output = if cfg!(windows) {
        let script = format!("(Invoke-WebRequest -Uri '{url}' -UseBasicParsing).Content");
        Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
    } else {
        Command::new("curl").args(["-fsSL", &url]).output()
    }
    .map_err(|error| format!("failed to run downloader: {error}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let text = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let tag = json
        .get("tag_name")
        .and_then(|value| value.as_str())
        .ok_or("release JSON has no tag_name")?;
    Ok(tag.strip_prefix('v').unwrap_or(tag).to_owned())
}

/// Compares the first three numeric components, ignoring a leading `v` and
/// any suffix (`-rc.1`, `+meta`).
fn version_newer(candidate: &str, current: &str) -> bool {
    fn parts(version: &str) -> [u32; 3] {
        let mut out = [0u32; 3];
        for (slot, part) in version
            .split(|c| c == '.' || c == '-' || c == '+')
            .filter(|part| !part.is_empty())
            .take(3)
            .enumerate()
        {
            out[slot] = part.parse().unwrap_or(0);
        }
        out
    }
    let candidate = parts(candidate.strip_prefix('v').unwrap_or(candidate));
    let current = parts(current);
    candidate > current
}
