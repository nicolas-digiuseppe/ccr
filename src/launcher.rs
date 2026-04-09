use std::process::Command;

/// Resolve a launch command, expanding known shell aliases to full paths.
fn resolve_command(launch_cmd: &str) -> String {
    let parts: Vec<&str> = launch_cmd.splitn(2, ' ').collect();
    let bin = parts[0];
    let args = parts.get(1).unwrap_or(&"");

    // Check if the binary exists in PATH
    if Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return launch_cmd.to_string();
    }

    // Try to resolve from zshrc aliases
    let home = std::env::var("HOME").unwrap_or_default();
    let zshrc = std::path::Path::new(&home).join(".zshrc");
    if let Ok(content) = std::fs::read_to_string(&zshrc) {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("alias ") {
                if let Some((name, value)) = rest.split_once('=') {
                    if name.trim() == bin {
                        let resolved = value.trim().trim_matches('\'').trim_matches('"');
                        let full = if args.is_empty() {
                            resolved.to_string()
                        } else {
                            format!("{} {}", resolved, args)
                        };
                        return full;
                    }
                }
            }
        }
    }

    launch_cmd.to_string()
}

/// Information needed to exec into a session after the TUI quits.
pub struct LaunchRequest {
    pub project_path: String,
    pub args: Vec<String>,
}

/// Build a LaunchRequest from config + session info.
/// Returns the resolved binary + args to exec into.
pub fn build_launch_request(launch_cmd: &str, session_id: &str, project_path: &str) -> LaunchRequest {
    let resolved = resolve_command(launch_cmd);
    let mut args: Vec<String> = resolved.split_whitespace().map(String::from).collect();
    args.push(session_id.to_string());

    LaunchRequest {
        project_path: project_path.to_string(),
        args,
    }
}

/// Spawn the session as a child process and wait for it to exit.
/// Returns control to ccr when claude exits.
pub fn spawn_session(request: &LaunchRequest) -> Result<(), String> {
    if std::env::set_current_dir(&request.project_path).is_err() {
        return Err(format!("Project directory not found: {}", request.project_path));
    }
    let bin = &request.args[0];
    let args = &request.args[1..];

    let _status = Command::new(bin)
        .args(args)
        .status()
        .map_err(|e| format!("Failed to launch {}: {}", bin, e))?;

    // Claude may exit with non-zero (e.g. ctrl-c) — still return to ccr
    Ok(())
}

/// Exec into the session, replacing the current process.
/// Call this AFTER restoring the terminal.
pub fn exec_session(request: &LaunchRequest) -> ! {
    use std::os::unix::process::CommandExt;

    if std::env::set_current_dir(&request.project_path).is_err() {
        eprintln!("Project directory not found: {}", request.project_path);
        std::process::exit(1);
    }

    let bin = &request.args[0];
    let args = &request.args[1..];

    // exec replaces this process — does not return
    let err = Command::new(bin).args(args).exec();
    eprintln!("Failed to exec {}: {}", bin, err);
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_launch_request() {
        let req = build_launch_request("mycommand --flag", "abc-123", "/Users/test/www/bugpilot");
        assert_eq!(req.project_path, "/Users/test/www/bugpilot");
        assert_eq!(req.args, vec!["mycommand", "--flag", "abc-123"]);
    }

    #[test]
    fn test_resolve_known_binary() {
        let resolved = resolve_command("ls -la");
        assert_eq!(resolved, "ls -la");
    }
}
