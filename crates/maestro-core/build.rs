use std::path::PathBuf;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn git_path(name: &str) -> Option<PathBuf> {
    git(&["rev-parse", "--git-path", name])
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok())
}

fn kestrel_version() -> Option<String> {
    let lock = std::fs::read_to_string("../../Cargo.lock").ok()?;
    let entry = lock.split("[[package]]").find(|p| p.contains("name = \"kestrel\""))?;
    let line = entry.lines().find(|l| l.starts_with("version = "))?;
    Some(line.trim_start_matches("version = ").trim_matches('"').to_string())
}

fn main() {
    let kestrel = kestrel_version().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=KESTREL_VERSION={kestrel}");
    println!("cargo:rerun-if-changed=../../Cargo.lock");

    let build_id = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=BUILD_ID={}", build_id);

    // Rerun only when the checked-out commit can have changed: HEAD moves on a
    // branch switch, the branch ref (loose or packed) on a new commit.
    println!("cargo:rerun-if-changed=build.rs");
    let mut watched = vec!["HEAD".to_string(), "packed-refs".to_string()];
    if let Some(branch_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watched.push(branch_ref);
    }
    for path in watched.iter().filter_map(|name| git_path(name)) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
