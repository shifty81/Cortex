use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitState {
    pub root: PathBuf,
    pub branch: Option<String>,
    pub head: String,
    pub clean: bool,
    pub porcelain: Vec<String>,
    pub origin: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub observed_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InternalGitBinding {
    pub bare_repository: PathBuf,
    pub remote_name: String,
    pub source_root: PathBuf,
    pub branch: String,
    pub head: String,
    pub pushed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeRecord {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitMutationReceipt {
    pub action: String,
    pub root: PathBuf,
    pub before_head: String,
    pub after_head: String,
    pub branch: Option<String>,
    pub detail: String,
    pub unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHistoryRow {
    pub commit: String,
    pub parents: Vec<String>,
    pub decorations: String,
    pub subject: String,
}

pub fn inspect(root: &Path) -> Result<GitState, String> {
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    let root_path = PathBuf::from(top.trim());
    let head = git(root, &["rev-parse", "HEAD"])?.trim().to_owned();
    let branch = git_optional(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let porcelain = git(root, &["status", "--porcelain=v1"])?
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let origin = git_optional(root, &["remote", "get-url", "origin"]);
    let upstream = git_optional(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    );
    let (ahead, behind) = upstream
        .as_deref()
        .and_then(|upstream| {
            git_optional(
                root,
                &[
                    "rev-list",
                    "--left-right",
                    "--count",
                    &format!("HEAD...{upstream}"),
                ],
            )
        })
        .and_then(|value| parse_counts(&value))
        .map(|(ahead, behind)| (Some(ahead), Some(behind)))
        .unwrap_or((None, None));
    Ok(GitState {
        root: root_path,
        branch,
        head,
        clean: porcelain.is_empty(),
        porcelain,
        origin,
        upstream,
        ahead,
        behind,
        observed_unix_ms: unix_ms(),
    })
}

pub fn list_worktrees(root: &Path) -> Result<Vec<WorktreeRecord>, String> {
    let text = git(root, &["worktree", "list", "--porcelain"])?;
    let mut out = Vec::new();
    let mut current: Option<WorktreeRecord> = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(record) = current.take() {
                out.push(record);
            }
            current = Some(WorktreeRecord {
                path: PathBuf::from(path),
                head: None,
                branch: None,
                detached: false,
                locked: false,
            });
        } else if let Some(record) = current.as_mut() {
            if let Some(head) = line.strip_prefix("HEAD ") {
                record.head = Some(head.to_owned());
            } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                record.branch = Some(branch.to_owned());
            } else if line == "detached" {
                record.detached = true;
            } else if line.starts_with("locked") {
                record.locked = true;
            }
        }
    }
    if let Some(record) = current {
        out.push(record);
    }
    Ok(out)
}

pub fn ensure_internal_repository(
    source_root: &Path,
    internal_root: &Path,
    project_id: &str,
    remote_name: &str,
) -> Result<InternalGitBinding, String> {
    validate_id(project_id)?;
    validate_remote_name(remote_name)?;
    let state = inspect(source_root)?;
    fs::create_dir_all(internal_root).map_err(|error| error.to_string())?;
    let bare = internal_root.join(format!("{project_id}.git"));
    if !bare.is_dir() {
        run(Command::new("git").args(["init", "--bare"]).arg(&bare), None)?;
    }
    let bare_text = bare.to_string_lossy().into_owned();
    let remotes = git(source_root, &["remote"])?;
    if remotes.lines().any(|line| line.trim() == remote_name) {
        run(
            Command::new("git").args(["remote", "set-url", remote_name, &bare_text]),
            Some(source_root),
        )?;
    } else {
        run(
            Command::new("git").args(["remote", "add", remote_name, &bare_text]),
            Some(source_root),
        )?;
    }
    Ok(InternalGitBinding {
        bare_repository: bare,
        remote_name: remote_name.to_owned(),
        source_root: source_root.to_path_buf(),
        branch: state.branch.unwrap_or_else(|| "main".to_owned()),
        head: state.head,
        pushed: false,
    })
}

pub fn push_internal_snapshot(
    source_root: &Path,
    remote_name: &str,
) -> Result<InternalGitBinding, String> {
    validate_remote_name(remote_name)?;
    let state = inspect(source_root)?;
    let branch = state.branch.clone().ok_or_else(|| {
        "cannot snapshot detached HEAD into Internal Git without an explicit branch".to_owned()
    })?;
    run(
        Command::new("git").args([
            "push",
            remote_name,
            &format!("HEAD:refs/heads/{branch}"),
        ]),
        Some(source_root),
    )?;
    let remote_url = git(source_root, &["remote", "get-url", remote_name])?;
    Ok(InternalGitBinding {
        bare_repository: PathBuf::from(remote_url.trim()),
        remote_name: remote_name.to_owned(),
        source_root: source_root.to_path_buf(),
        branch,
        head: state.head,
        pushed: true,
    })
}

pub fn fetch_origin(root: &Path) -> Result<(), String> {
    run(
        Command::new("git").args(["fetch", "--prune", "origin"]),
        Some(root),
    )
    .map(|_| ())
}

pub fn push_origin(root: &Path) -> Result<(), String> {
    run(
        Command::new("git").args(["push", "origin", "HEAD"]),
        Some(root),
    )
    .map(|_| ())
}

pub fn create_worktree(root: &Path, destination: &Path, branch: &str) -> Result<(), String> {
    if destination.exists() {
        return Err(format!(
            "worktree destination already exists: {}",
            destination.display()
        ));
    }
    validate_branch_name(branch)?;
    run(
        Command::new("git")
            .args(["worktree", "add", "-b", branch])
            .arg(destination),
        Some(root),
    )
    .map(|_| ())
}

pub fn create_branch(
    root: &Path,
    branch: &str,
    start_point: Option<&str>,
) -> Result<GitMutationReceipt, String> {
    validate_branch_name(branch)?;
    let before = inspect(root)?;
    let mut command = Command::new("git");
    command.args(["branch", branch]);
    if let Some(start) = start_point.filter(|value| !value.trim().is_empty()) {
        command.arg(start);
    }
    run(&mut command, Some(root))?;
    mutation_receipt(
        "create_branch",
        root,
        &before,
        format!("created branch {branch}"),
    )
}

pub fn switch_branch(root: &Path, branch: &str) -> Result<GitMutationReceipt, String> {
    validate_branch_name(branch)?;
    let before = inspect(root)?;
    if !before.clean {
        return Err("refusing to switch branches with a dirty working tree".to_owned());
    }
    run(Command::new("git").args(["switch", branch]), Some(root))?;
    mutation_receipt(
        "switch_branch",
        root,
        &before,
        format!("switched to {branch}"),
    )
}

pub fn pull_ff_only(root: &Path) -> Result<GitMutationReceipt, String> {
    let before = inspect(root)?;
    if !before.clean {
        return Err("refusing fast-forward pull with a dirty working tree".to_owned());
    }
    let branch = before
        .branch
        .clone()
        .ok_or_else(|| "cannot pull while HEAD is detached".to_owned())?;
    run(
        Command::new("git").args(["pull", "--ff-only", "origin", &branch]),
        Some(root),
    )?;
    mutation_receipt(
        "pull_ff_only",
        root,
        &before,
        format!("fast-forwarded {branch} from origin"),
    )
}

pub fn commit_all(
    root: &Path,
    message: &str,
) -> Result<Option<GitMutationReceipt>, String> {
    if message.trim().is_empty() {
        return Err("commit message cannot be empty".to_owned());
    }
    let before = inspect(root)?;
    if before.clean {
        return Ok(None);
    }
    run(Command::new("git").args(["add", "-A"]), Some(root))?;
    run(
        Command::new("git").args(["commit", "-m", message]),
        Some(root),
    )?;
    mutation_receipt("commit_all", root, &before, message.to_owned()).map(Some)
}

pub fn tag_checkpoint(
    root: &Path,
    tag: &str,
    message: &str,
) -> Result<GitMutationReceipt, String> {
    validate_ref_name(tag)?;
    let before = inspect(root)?;
    run(
        Command::new("git").args(["tag", "-a", tag, "-m", message]),
        Some(root),
    )?;
    mutation_receipt(
        "tag_checkpoint",
        root,
        &before,
        format!("created annotated tag {tag}"),
    )
}

pub fn history(root: &Path, limit: usize) -> Result<Vec<GitHistoryRow>, String> {
    let limit = limit.clamp(1, 1_000).to_string();
    let format = "%H%x1f%P%x1f%D%x1f%s";
    let text = git(
        root,
        &[
            "log",
            "--date-order",
            "--decorate=short",
            "--format",
            format,
            "-n",
            &limit,
        ],
    )?;
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut parts = line.splitn(4, '\u{1f}');
        let Some(commit) = parts.next() else {
            continue;
        };
        let parents = parts
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let decorations = parts.next().unwrap_or_default().to_owned();
        let subject = parts.next().unwrap_or_default().to_owned();
        rows.push(GitHistoryRow {
            commit: commit.to_owned(),
            parents,
            decorations,
            subject,
        });
    }
    Ok(rows)
}

fn mutation_receipt(
    action: &str,
    root: &Path,
    before: &GitState,
    detail: String,
) -> Result<GitMutationReceipt, String> {
    let after = inspect(root)?;
    Ok(GitMutationReceipt {
        action: action.to_owned(),
        root: root.to_path_buf(),
        before_head: before.head.clone(),
        after_head: after.head,
        branch: after.branch,
        detail,
        unix_ms: unix_ms(),
    })
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_optional(root: &Path, args: &[&str]) -> Option<String> {
    git(root, args)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn run(command: &mut Command, cwd: Option<&Path>) -> Result<String, String> {
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn parse_counts(value: &str) -> Option<(u64, u64)> {
    let mut values = value.split_whitespace();
    let ahead = values.next()?.parse().ok()?;
    let behind = values.next()?.parse().ok()?;
    Some((ahead, behind))
}

fn validate_id(value: &str) -> Result<(), String> {
    if value.len() < 2
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        Err(format!("invalid project id: {value}"))
    } else {
        Ok(())
    }
}

fn validate_remote_name(value: &str) -> Result<(), String> {
    if value.is_empty()
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        Err(format!("invalid Git remote name: {value}"))
    } else {
        Ok(())
    }
}

fn validate_branch_name(value: &str) -> Result<(), String> {
    validate_ref_name(value)?;
    if value == "HEAD" || value.starts_with('-') || value.ends_with('/') || value.contains("..") {
        return Err(format!("invalid Git branch name: {value}"));
    }
    Ok(())
}

fn validate_ref_name(value: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || value.contains('~')
        || value.contains('^')
        || value.contains(':')
        || value.contains('?')
        || value.contains('*')
        || value.contains('[')
        || value.contains('\\')
    {
        Err(format!("invalid Git ref name: {value}"))
    } else {
        Ok(())
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_parser_matches_git_left_right_order() {
        assert_eq!(parse_counts("2\t5"), Some((2, 5)));
    }

    #[test]
    fn rejects_bad_remote_names() {
        assert!(validate_remote_name("forge-internal").is_ok());
        assert!(validate_remote_name("../bad").is_err());
    }

    #[test]
    fn validates_branch_names() {
        assert!(validate_branch_name("feature/native-ide").is_ok());
        assert!(validate_branch_name("bad branch").is_err());
        assert!(validate_branch_name("-danger").is_err());
    }
}
