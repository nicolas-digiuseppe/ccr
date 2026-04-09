use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub launch_command: String,
    pub terminal: String,
    pub exclude_projects: Vec<String>,
    pub theme: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            launch_command: "claude --resume".to_string(),
            terminal: "warp".to_string(),
            exclude_projects: Vec::new(),
            theme: "dark".to_string(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let path = Path::new(&home).join(".claude").join("ccr.toml");
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(cfg) = toml::from_str(&content) {
                    return cfg;
                }
            }
        }
        Config::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = Config::default();
        assert_eq!(cfg.launch_command, "claude --resume");
        assert_eq!(cfg.terminal, "warp");
        assert!(cfg.exclude_projects.is_empty());
    }

    #[test]
    fn test_parse_toml() {
        let toml_str = r#"
launch_command = "claude --resume"
terminal = "iterm"
exclude_projects = ["tmp"]
theme = "light"
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.launch_command, "claude --resume");
        assert_eq!(cfg.terminal, "iterm");
        assert_eq!(cfg.exclude_projects, vec!["tmp"]);
        assert_eq!(cfg.theme, "light");
    }
}
