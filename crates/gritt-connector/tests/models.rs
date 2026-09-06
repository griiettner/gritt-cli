//! Parser fixtures and fake-process coverage for connector model discovery.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use gritt_connector::models::ConnectorModelCache;
use gritt_connector::protocols::claude::ClaudeCode;
use gritt_connector::protocols::codex::{parse_codex_models, Codex};
use gritt_connector::protocols::cursor::{parse_cursor_models, Cursor};
use gritt_connector::protocols::opencode::{parse_opencode_models, OpenCode};
use gritt_connector::protocols::ModelParseError;
use gritt_connector::{ExternalConnector, Protocol, Timeouts};
use gritt_core::config::{ConnectorSettings, ModelListPolicy};
use gritt_core::connector::{
    Connector, ConnectorId, ConnectorModelDiscovery, ConnectorModelFreshness, TaskRequest,
};
use gritt_core::session::SessionId;

fn fixture(connector: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/models")
        .join(connector)
        .join(name)
}

fn agent_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-agent/agent.sh")
}

struct Fake {
    dir: tempfile::TempDir,
    wrapper: PathBuf,
}

impl Fake {
    fn new(vars: &[(&str, String)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("agent");
        let mut script = String::from("#!/bin/sh\n");
        for (name, value) in vars {
            script.push_str(&format!(
                "{name}='{}'\nexport {name}\n",
                value.replace('\'', "'\\''")
            ));
        }
        script.push_str(&format!("exec '{}' \"$@\"\n", agent_script().display()));
        std::fs::write(&wrapper, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { dir, wrapper }
    }

    fn settings(&self, name: &str) -> ConnectorSettings {
        ConnectorSettings {
            executables: BTreeMap::from([(name.to_owned(), self.wrapper.display().to_string())]),
            ..ConnectorSettings::default()
        }
    }

    fn connector<P: Protocol>(&self, protocol: P, name: &str) -> ExternalConnector<P> {
        self.connector_with_cache(protocol, name, None)
    }

    fn connector_with_cache<P: Protocol>(
        &self,
        protocol: P,
        name: &str,
        cache: Option<PathBuf>,
    ) -> ExternalConnector<P> {
        let mut connector =
            ExternalConnector::new(protocol, &self.settings(name)).with_timeouts(Timeouts {
                health: Duration::from_secs(5),
                startup: Duration::from_secs(5),
                idle: Duration::from_secs(5),
            });
        if let Some(dir) = cache {
            connector = connector.with_model_cache(
                ConnectorModelCache::new(dir),
                ModelListPolicy {
                    refresh_interval_secs: 24 * 60 * 60,
                    stale_fallback: true,
                },
            );
        }
        connector
    }

    fn request(&self, prompt: &str, model: Option<&str>) -> TaskRequest {
        TaskRequest {
            session_id: SessionId("s-models".into()),
            prompt: prompt.into(),
            workspace: self.dir.path().to_path_buf(),
            continuation: None,
            model: model.map(str::to_owned),
        }
    }
}

#[test]
fn parsers_read_committed_catalog_fixtures() {
    let codex =
        parse_codex_models(&std::fs::read_to_string(fixture("codex", "current.json")).unwrap())
            .unwrap();
    assert_eq!(codex[0].id, "gpt-5.4");
    assert_eq!(codex[0].display_label.as_deref(), Some("GPT-5.4"));
    assert_eq!(codex[1].id, "gpt-5.4-mini");

    let cursor =
        parse_cursor_models(&std::fs::read_to_string(fixture("cursor", "list.txt")).unwrap())
            .unwrap();
    assert_eq!(
        cursor.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["gpt-5.5-medium", "composer-2.5", "gpt-5.3-codex"]
    );

    let opencode =
        parse_opencode_models(&std::fs::read_to_string(fixture("opencode", "list.txt")).unwrap())
            .unwrap();
    assert_eq!(
        opencode.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        [
            "opencode/big-pickle",
            "openai/gpt-5-nano",
            "anthropic/claude-sonnet-4-6"
        ]
    );
}

#[test]
fn parsers_reject_malformed_catalogs() {
    assert_eq!(
        parse_codex_models(&std::fs::read_to_string(fixture("codex", "malformed.json")).unwrap()),
        Err(ModelParseError::Malformed)
    );
    assert_eq!(
        parse_cursor_models(&std::fs::read_to_string(fixture("cursor", "malformed.txt")).unwrap()),
        Err(ModelParseError::Malformed)
    );
    assert_eq!(
        parse_opencode_models(
            &std::fs::read_to_string(fixture("opencode", "malformed.txt")).unwrap()
        ),
        Err(ModelParseError::Malformed)
    );
}

#[tokio::test]
async fn a_current_catalog_names_its_source_and_fetch_time() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_MODELS_FILE",
        fixture("codex", "current.json").display().to_string(),
    )]);
    let connector = fake.connector(Codex, "codex");
    let outcome = connector.discover_models(false).await;
    let ConnectorModelDiscovery::Current { catalog } = outcome else {
        panic!("expected current catalog, got {outcome:?}");
    };
    assert_eq!(catalog.connector, ConnectorId::Codex);
    assert_eq!(catalog.source, "codex debug models");
    assert_eq!(catalog.freshness, ConnectorModelFreshness::Current);
    assert!(!catalog.models.is_empty());
    assert!(catalog.fetched_at <= Utc::now());
}

#[tokio::test]
async fn selected_model_reaches_the_cli_as_a_typed_flag() {
    let args_dir = tempfile::tempdir().unwrap();
    let args_path = args_dir.path().join("args.txt");
    let fake = Fake::new(&[("FAKE_AGENT_ARGS_FILE", args_path.display().to_string())]);
    let connector = fake.connector(Codex, "codex");
    let mut stream = connector
        .start(fake.request("run it", Some("gpt-5.4")))
        .await
        .expect("start");
    while futures::StreamExt::next(&mut stream).await.is_some() {}
    let args = std::fs::read_to_string(&args_path).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    assert!(
        lines.windows(2).any(|pair| pair == ["--model", "gpt-5.4"]),
        "selected model missing from {lines:?}"
    );
    assert!(!lines
        .iter()
        .any(|line| line.contains("run it") && *line != "run it"));
}

#[tokio::test]
async fn default_selection_omits_the_model_flag() {
    let args_dir = tempfile::tempdir().unwrap();
    let args_path = args_dir.path().join("args.txt");
    let fake = Fake::new(&[("FAKE_AGENT_ARGS_FILE", args_path.display().to_string())]);
    let connector = fake.connector(Codex, "codex");
    let mut stream = connector
        .start(fake.request("run it", None))
        .await
        .expect("start");
    while futures::StreamExt::next(&mut stream).await.is_some() {}
    let args = std::fs::read_to_string(&args_path).unwrap();
    assert!(
        !args.lines().any(|line| line == "--model"),
        "default launch passed --model: {args}"
    );
}

#[tokio::test]
async fn missing_executable_is_unavailable() {
    let settings = ConnectorSettings {
        executables: BTreeMap::from([(
            "codex".to_owned(),
            "/definitely/not/an/installed/codex".into(),
        )]),
        ..ConnectorSettings::default()
    };
    let connector = ExternalConnector::new(Codex, &settings);
    let outcome = connector.discover_models(false).await;
    assert!(
        matches!(
            outcome,
            ConnectorModelDiscovery::Unavailable {
                connector: ConnectorId::Codex,
                ..
            }
        ),
        "{outcome:?}"
    );
    assert!(outcome.describe().contains("codex"));
}

#[tokio::test]
async fn claude_listing_is_unsupported_and_selection_still_works() {
    let args_dir = tempfile::tempdir().unwrap();
    let args_path = args_dir.path().join("args.txt");
    let fake = Fake::new(&[("FAKE_AGENT_ARGS_FILE", args_path.display().to_string())]);
    let connector = fake.connector(ClaudeCode, "claude");
    let outcome = connector.discover_models(false).await;
    assert!(
        matches!(
            outcome,
            ConnectorModelDiscovery::Unsupported {
                connector: ConnectorId::ClaudeCode,
                ..
            }
        ),
        "{outcome:?}"
    );
    let mut stream = connector
        .start(fake.request("hi", Some("sonnet")))
        .await
        .expect("start");
    while futures::StreamExt::next(&mut stream).await.is_some() {}
    let args = std::fs::read_to_string(&args_path).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    assert!(lines.windows(2).any(|pair| pair == ["--model", "sonnet"]));
}

#[tokio::test]
async fn command_failure_is_typed() {
    let fake = Fake::new(&[("FAKE_AGENT_MODELS_EXIT", "1".into())]);
    let connector = fake.connector(Codex, "codex");
    let outcome = connector.discover_models(true).await;
    assert!(
        matches!(
            outcome,
            ConnectorModelDiscovery::CommandFailure {
                connector: ConnectorId::Codex,
                ..
            }
        ),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn malformed_output_is_typed() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_MODELS_FILE",
        fixture("codex", "malformed.json").display().to_string(),
    )]);
    let connector = fake.connector(Codex, "codex");
    let outcome = connector.discover_models(true).await;
    assert!(
        matches!(
            outcome,
            ConnectorModelDiscovery::MalformedOutput {
                connector: ConnectorId::Codex,
                ..
            }
        ),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn refresh_failure_falls_back_to_a_stale_cache() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_MODELS_FILE",
        fixture("codex", "current.json").display().to_string(),
    )]);
    let cache_dir = fake.dir.path().join("cache");
    let cache = ConnectorModelCache::new(&cache_dir);
    cache
        .write(
            ConnectorId::Codex,
            &gritt_connector::models::CachedConnectorModels {
                fetched_at: Some(Utc::now() - ChronoDuration::hours(48)),
                last_attempt_at: Some(Utc::now() - ChronoDuration::hours(48)),
                source: "codex debug models".into(),
                models: parse_codex_models(
                    &std::fs::read_to_string(fixture("codex", "current.json")).unwrap(),
                )
                .unwrap(),
            },
        )
        .unwrap();
    let failing = Fake::new(&[
        (
            "FAKE_AGENT_MODELS_FILE",
            fixture("codex", "current.json").display().to_string(),
        ),
        ("FAKE_AGENT_MODELS_EXIT", "1".into()),
    ]);
    // Reuse the same cache dir with a failing process.
    let connector = failing.connector_with_cache(Codex, "codex", Some(cache_dir));
    let outcome = connector.discover_models(true).await;
    let ConnectorModelDiscovery::CachedStale { catalog, reason } = outcome else {
        panic!("expected stale fallback, got {outcome:?}");
    };
    assert_eq!(catalog.freshness, ConnectorModelFreshness::Stale);
    assert_eq!(catalog.models[0].id, "gpt-5.4");
    assert!(!reason.contains("sk-"));
    assert_ne!(catalog.freshness, ConnectorModelFreshness::Current);
}

#[tokio::test]
async fn cursor_and_opencode_discover_from_the_fake_agent() {
    let cursor = Fake::new(&[(
        "FAKE_AGENT_MODELS_FILE",
        fixture("cursor", "list.txt").display().to_string(),
    )]);
    let outcome = cursor
        .connector(Cursor, "cursor")
        .discover_models(false)
        .await;
    let catalog = outcome.catalog().expect("cursor catalog");
    assert_eq!(catalog.source, "cursor-agent --list-models");
    assert!(catalog.models.iter().any(|m| m.id == "gpt-5.5-medium"));

    let opencode = Fake::new(&[(
        "FAKE_AGENT_MODELS_FILE",
        fixture("opencode", "list.txt").display().to_string(),
    )]);
    let outcome = opencode
        .connector(OpenCode, "opencode")
        .discover_models(false)
        .await;
    let catalog = outcome.catalog().expect("opencode catalog");
    assert_eq!(catalog.source, "opencode models");
    assert!(catalog.models.iter().any(|m| m.id == "openai/gpt-5-nano"));
}
