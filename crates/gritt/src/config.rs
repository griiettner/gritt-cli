//! Loads config layers in ADR-008 precedence: project file, user file, and
//! `GRITT_*` environment overrides, merged over the core defaults. Flags
//! are applied by the caller as the highest layer.

use std::path::{Path, PathBuf};

use gritt_core::config::{layer_from_value, merge, Config, ConfigLayer};
use gritt_core::{embeddings, Error, Result};

/// Project configuration lives at the workspace root so first-run setup does
/// not require creating a hidden directory.
pub const PROJECT_CONFIG: &str = "config.toml";

pub fn user_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("gritt").join("config.toml"))
}

fn load_file(path: &Path) -> Result<Option<ConfigLayer>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::config(format!("cannot read {}: {error}", path.display())))?;
    let hint = path.display().to_string();
    parse_toml(&text, &hint).map(Some)
}

/// Parses one config file. A parse error names the file, line, and column
/// with the parser's message only; the offending source line is never
/// repeated, because a malformed file may hold a key value.
pub fn parse_toml(text: &str, hint: &str) -> Result<ConfigLayer> {
    let value: toml::Value = toml::from_str(text).map_err(|error| {
        let at = error
            .span()
            .map(|span| line_column(text, span.start))
            .map(|(line, column)| format!(" at line {line}, column {column}"))
            .unwrap_or_default();
        Error::config(format!("invalid TOML in {hint}{at}: {}", error.message()))
    })?;
    let json = serde_json::to_value(value)
        .map_err(|error| Error::config(format!("cannot convert {hint}: {error}")))?;
    layer_from_value(json, hint)
}

fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    (line, column)
}

/// Environment overrides. `GRITT_DEFAULT_PROFILE` and `GRITT_DEFAULT_MODEL`
/// set defaults; the `AGENT_*` memory variables configure opt-in
/// embeddings and reranking.
pub fn env_layer(vars: impl IntoIterator<Item = (String, String)>) -> ConfigLayer {
    let env: std::collections::HashMap<String, String> = vars.into_iter().collect();
    let embedding = embeddings::embedding_config(&env);
    let rerank = embeddings::rerank_config(&env);
    ConfigLayer {
        default_profile: env.get("GRITT_DEFAULT_PROFILE").cloned(),
        default_model: env.get("GRITT_DEFAULT_MODEL").cloned(),
        embeddings: embedding.is_enabled().then_some(embedding),
        rerank: rerank.is_enabled().then_some(rerank),
        ..ConfigLayer::default()
    }
}

pub fn load(workspace: &Path, vars: impl IntoIterator<Item = (String, String)>) -> Result<Config> {
    load_with(
        workspace,
        user_config_path().as_deref(),
        vars,
        ConfigLayer::default(),
    )
}

/// Lowest precedence first: environment, user, project, flags.
pub fn load_with(
    workspace: &Path,
    user_path: Option<&Path>,
    vars: impl IntoIterator<Item = (String, String)>,
    flags: ConfigLayer,
) -> Result<Config> {
    let mut layers = vec![env_layer(vars)];
    if let Some(user) = user_path.and_then(|path| load_file(path).transpose()) {
        layers.push(user?);
    }
    if let Some(project) = load_file(&workspace.join(PROJECT_CONFIG))? {
        layers.push(project);
    }
    layers.push(flags);
    Ok(merge(layers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gritt_core::ErrorKind;

    #[test]
    fn project_beats_user_beats_env() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        std::fs::write(&user, "default_model = \"user-model\"\n").unwrap();
        std::fs::write(
            dir.path().join(PROJECT_CONFIG),
            "default_model = \"project-model\"\n",
        )
        .unwrap();
        let vars = [
            ("GRITT_DEFAULT_MODEL".to_string(), "env-model".to_string()),
            (
                "GRITT_DEFAULT_PROFILE".to_string(),
                "openrouter".to_string(),
            ),
        ];
        let config = load_with(dir.path(), Some(&user), vars, ConfigLayer::default()).unwrap();
        assert_eq!(config.default_model.as_deref(), Some("project-model"));
        assert_eq!(config.default_profile.as_deref(), Some("openrouter"));
        assert!(config.embeddings.is_none());
    }

    #[test]
    fn literal_key_in_toml_fails_loudly() {
        let text = "[profiles.openai]\nname = \"openai\"\nprotocol = \"responses\"\nbase_url = \"https://api.openai.com/v1\"\napi_key = \"sk-literal\"\n";
        let error = parse_toml(text, "test").unwrap_err();
        assert_eq!(error.kind, ErrorKind::SecretInConfig);
        assert!(!error.message.contains("sk-literal"));
    }

    #[test]
    fn malformed_toml_never_echoes_the_source_line() {
        let text = "[profiles.openai]\nname = \"openai\"\napi_key = \"sk-leak\n";
        let error = parse_toml(text, "project config").unwrap_err();
        assert_eq!(error.kind, ErrorKind::Config);
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert!(!display.contains("sk-leak"), "{display}");
        assert!(!debug.contains("sk-leak"), "{debug}");
        assert!(display.contains("line 3"), "{display}");
        assert!(display.contains("project config"), "{display}");
    }

    #[test]
    fn toml_embeddings_section_is_rejected() {
        let text = "[embeddings]\nmodel = \"text-embedding-3-small\"\n";
        let error = parse_toml(text, "project config").unwrap_err();
        assert_eq!(error.kind, ErrorKind::Config);
        assert!(error.message.contains("embeddings"));
    }

    #[test]
    fn env_layer_enables_embeddings_and_reranking() {
        let vars = [
            (
                "AGENT_EMBEDDING_PROVIDER".to_string(),
                "text-embedding-3-small".to_string(),
            ),
            (
                "AGENT_RERANK_PROVIDER".to_string(),
                "rerank-3.5".to_string(),
            ),
        ];
        let dir = tempfile::tempdir().unwrap();
        let config = load_with(dir.path(), None, vars, ConfigLayer::default()).unwrap();
        assert!(config.embeddings.as_ref().is_some_and(|e| e.is_enabled()));
        assert!(config.rerank.as_ref().is_some_and(|r| r.is_enabled()));
    }

    #[test]
    fn profile_with_key_reference_parses() {
        let text = "[profiles.openrouter]\nname = \"openrouter\"\nprotocol = \"chat_completions\"\nbase_url = \"https://openrouter.ai/api/v1\"\n[profiles.openrouter.key]\nkeychain_service_entry = \"gritt/openrouter\"\nenv_var_name = \"OPENROUTER_API_KEY\"\n";
        let layer = parse_toml(text, "test").unwrap();
        assert_eq!(
            layer.profiles["openrouter"].key.env_var_name,
            "OPENROUTER_API_KEY"
        );
    }

    #[test]
    fn partial_connectors_sections_parse() {
        let text =
            "[connectors]\npty = [\"codex\"]\n[connectors.extra_args]\ncodex = [\"--full-auto\"]\n";
        let layer = parse_toml(text, "test").unwrap();
        let connectors = layer.connectors.unwrap();
        assert_eq!(connectors.pty, vec!["codex".to_string()]);
        assert_eq!(
            connectors.extra_args["codex"],
            vec!["--full-auto".to_string()]
        );
        assert!(connectors.executables.is_empty());
        let layer = parse_toml("[connectors]\ntask_timeout_secs = 30\n", "test").unwrap();
        assert_eq!(layer.connectors.unwrap().task_timeout_secs, Some(30));
    }
}
