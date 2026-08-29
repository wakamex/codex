use super::*;
use crate::history_cell::HistoryCell;
use crate::legacy_core::config::ConfigBuilder;
use crate::legacy_core::config::ConfigOverrides;
use codex_app_server_client::AppServerClient;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use color_eyre::eyre::WrapErr;
use pretty_assertions::assert_eq;
use std::future::Future;
use std::path::Path;

fn run_async_test_with_stack<F>(name: &str, future: F) -> Result<()>
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    const TEST_STACK_SIZE_BYTES: usize = 8 * 1024 * 1024;

    let handle = std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(TEST_STACK_SIZE_BYTES)
        .spawn(move || -> Result<()> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(Box::pin(future))
        })?;

    match handle.join() {
        Ok(result) => result,
        Err(_) => Err(color_eyre::eyre::eyre!("{name} thread panicked")),
    }
}

#[test]
fn app_scoped_key_path_quotes_dotted_app_ids() {
    assert_eq!(
        app_scoped_key_path("plugin.linear", "enabled"),
        "apps.\"plugin.linear\".enabled"
    );
}

#[test]
fn trusted_project_edit_targets_project_trust_level() {
    assert_eq!(
        trusted_project_edit(Path::new("/workspace/team.project")),
        ConfigEdit {
            key_path: "projects.\"/workspace/team.project\".trust_level".to_string(),
            value: serde_json::json!("trusted"),
            merge_strategy: MergeStrategy::Replace,
        }
    );
}

#[test]
fn remote_project_trust_guards_thread_start_and_preserves_repository_decisions() -> Result<()> {
    run_async_test_with_stack("remote-project-trust-guards-thread-start", async move {
        let temp_dir = tempfile::tempdir()?;
        let codex_home = temp_dir.path().join("codex-home");
        let project_root = temp_dir
            .path()
            .join("project as a trusted project in sibling");
        let project_cwd = project_root.join("nested");
        std::fs::create_dir_all(&codex_home)?;
        std::fs::create_dir_all(&project_cwd)?;
        std::fs::create_dir(project_root.join(".git"))?;
        std::fs::write(project_root.join(".git/HEAD"), "ref: refs/heads/main\n")?;
        std::fs::create_dir(project_cwd.join(".codex"))?;
        let undecided_config = format!(
            "[{}]\n",
            trusted_project_edit(&project_root)
                .key_path
                .trim_end_matches(".trust_level")
        );
        std::fs::write(codex_home.join("config.toml"), &undecided_config)?;
        std::fs::write(
            project_cwd.join(".codex/config.toml"),
            "model_reasoning_effort = \"high\"\n",
        )?;
        let config = ConfigBuilder::default()
            .codex_home(codex_home.clone())
            .harness_overrides(ConfigOverrides {
                cwd: Some(codex_home.clone()),
                ..ConfigOverrides::default()
            })
            .build()
            .await?;
        let app_server =
            AppServerClient::InProcess(crate::tests::start_test_embedded_app_server(config).await?);
        let relative_cwd = pathdiff::diff_paths(&project_cwd, std::env::current_dir()?)
            .ok_or_else(|| color_eyre::eyre::eyre!("failed to calculate relative project path"))?;

        assert_eq!(
            read_remote_project_trust(app_server.request_handle(), &relative_cwd).await?,
            Some(RemoteProjectTrust {
                cwd: project_cwd.clone(),
                trust_target: PathBuf::from(project_trust_key(&project_root)),
            })
        );
        assert_eq!(
            std::fs::read_to_string(codex_home.join("config.toml"))?,
            undecided_config
        );

        write_trusted_project(app_server.request_handle(), &project_root).await?;
        let persisted_config: toml::Value =
            toml::from_str(&std::fs::read_to_string(codex_home.join("config.toml"))?)?;
        assert_eq!(
            persisted_config["projects"][project_trust_key(&project_root)]["trust_level"].as_str(),
            Some("trusted")
        );

        let _: ThreadStartResponse = app_server
            .request_typed(ClientRequest::ThreadStart {
                request_id: RequestId::Integer(1),
                params: ThreadStartParams {
                    cwd: Some(project_cwd.to_string_lossy().into_owned()),
                    ephemeral: Some(true),
                    ..ThreadStartParams::default()
                },
            })
            .await?;
        assert_eq!(
            read_remote_project_trust(app_server.request_handle(), &project_cwd).await?,
            None
        );

        app_server.shutdown().await?;
        Ok(())
    })
}

#[test]
fn remote_project_trust_preserves_an_untrusted_repository_decision() -> Result<()> {
    run_async_test_with_stack("remote-project-trust-preserves-untrusted", async move {
        let temp_dir = tempfile::tempdir()?;
        let codex_home = temp_dir.path().join("codex-home");
        let project_root = temp_dir.path().join("untrusted-project");
        let project_cwd = project_root.join("nested");
        std::fs::create_dir_all(&codex_home)?;
        std::fs::create_dir_all(project_cwd.join(".codex"))?;
        std::fs::create_dir(project_root.join(".git"))?;
        std::fs::write(project_root.join(".git/HEAD"), "ref: refs/heads/main\n")?;
        std::fs::write(
            project_cwd.join(".codex/config.toml"),
            "model_reasoning_effort = \"high\"\n",
        )?;
        std::fs::write(
            codex_home.join("config.toml"),
            format!(
                "[{}]\ntrust_level = \"untrusted\"\n",
                trusted_project_edit(&project_root)
                    .key_path
                    .trim_end_matches(".trust_level")
            ),
        )?;
        let config = ConfigBuilder::default()
            .codex_home(codex_home.clone())
            .harness_overrides(ConfigOverrides {
                cwd: Some(codex_home),
                ..ConfigOverrides::default()
            })
            .build()
            .await?;
        let app_server =
            AppServerClient::InProcess(crate::tests::start_test_embedded_app_server(config).await?);

        assert_eq!(
            read_remote_project_trust(app_server.request_handle(), &project_cwd).await?,
            None
        );

        let response: JsonValue = app_server
            .request_typed(ClientRequest::ConfigRead {
                request_id: RequestId::String("untrusted-project-warning".to_string()),
                params: ConfigReadParams {
                    include_layers: true,
                    cwd: Some(project_cwd.to_string_lossy().into_owned()),
                },
            })
            .await?;
        let reason = response["layers"]
            .as_array()
            .expect("config layers")
            .iter()
            .find(|layer| layer["name"]["type"] == "project")
            .and_then(|layer| layer["disabledReason"].as_str())
            .expect("explicitly untrusted project warning");
        let warning = crate::history_cell::new_warning_event(
            reason.replace(&project_trust_key(&project_root), "<PROJECT>"),
        );
        insta::assert_snapshot!(
            warning
                .display_lines(/*width*/ 80)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
            @r"
    ⚠ <PROJECT> is marked as untrusted in the effective configuration. To load
      project-local config, hooks, and exec policies, update its trust setting. If
      that setting is managed by your organization, contact your administrator.
    "
        );

        std::fs::remove_file(project_cwd.join(".codex/config.toml"))?;
        std::fs::remove_dir(project_cwd.join(".codex"))?;
        let canonical_project_cwd = PathBuf::from(project_trust_key(&project_root)).join("nested");
        assert_eq!(
            read_remote_project_trust(app_server.request_handle(), &canonical_project_cwd).await?,
            None
        );

        app_server.shutdown().await?;
        Ok(())
    })
}

#[test]
fn remote_project_trust_normalizes_a_trusted_cwd_with_a_trailing_separator() -> Result<()> {
    run_async_test_with_stack("remote-project-trust-normalizes-cwd", async move {
        let temp_dir = tempfile::tempdir()?;
        let codex_home = temp_dir.path().join("codex-home");
        let project_root = temp_dir.path().join("trusted-project");
        std::fs::create_dir_all(&codex_home)?;
        std::fs::create_dir_all(&project_root)?;
        std::fs::create_dir(project_root.join(".git"))?;
        std::fs::write(project_root.join(".git/HEAD"), "ref: refs/heads/main\n")?;
        std::fs::write(
            codex_home.join("config.toml"),
            format!(
                "[{}]\ntrust_level = \"trusted\"\n",
                trusted_project_edit(&project_root)
                    .key_path
                    .trim_end_matches(".trust_level")
            ),
        )?;
        let config = ConfigBuilder::default()
            .codex_home(codex_home.clone())
            .harness_overrides(ConfigOverrides {
                cwd: Some(codex_home),
                ..ConfigOverrides::default()
            })
            .build()
            .await?;
        let app_server =
            AppServerClient::InProcess(crate::tests::start_test_embedded_app_server(config).await?);
        let cwd_with_trailing_separator = PathBuf::from(format!("{}/", project_root.display()));

        assert_eq!(
            read_remote_project_trust(
                app_server.request_handle(),
                cwd_with_trailing_separator.as_path(),
            )
            .await?,
            None
        );

        app_server.shutdown().await?;
        Ok(())
    })
}

#[test]
fn format_config_error_preserves_server_validation_message() {
    let err = Err::<(), _>(color_eyre::eyre::eyre!(
        "config/batchWrite failed: Invalid configuration: features.fast_mode=true violates \
         managed requirements; allowed set [fast_mode=false]"
    ))
    .wrap_err("config/batchWrite failed in TUI")
    .unwrap_err();

    assert_eq!(
        format_config_error(&err),
        "config/batchWrite failed in TUI: config/batchWrite failed: Invalid configuration: \
         features.fast_mode=true violates managed requirements; allowed set [fast_mode=false]"
    );
}
