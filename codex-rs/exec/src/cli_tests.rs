use super::*;
use clap::CommandFactory;
use pretty_assertions::assert_eq;

#[test]
fn resume_parses_prompt_after_global_flags() {
    const PROMPT: &str = "echo resume-with-global-flags-after-subcommand";
    let cli = Cli::parse_from([
        "codex-exec",
        "resume",
        "--last",
        "--json",
        "--model",
        "gpt-5.2-codex",
        "--dangerously-bypass-approvals-and-sandbox",
        "--skip-git-repo-check",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--no-tools",
        PROMPT,
    ]);

    assert!(cli.ephemeral);
    assert!(cli.ignore_user_config);
    assert!(cli.ignore_rules);
    assert!(cli.no_tools);
    let Some(Command::Resume(args)) = cli.command else {
        panic!("expected resume command");
    };
    let effective_prompt = args.prompt.clone().or_else(|| {
        if args.last {
            args.session_id.clone()
        } else {
            None
        }
    });
    assert_eq!(effective_prompt.as_deref(), Some(PROMPT));
}

#[test]
fn resume_accepts_output_flags_after_subcommand() {
    const PROMPT: &str = "echo resume-with-output-file";
    let cli = Cli::parse_from([
        "codex-exec",
        "resume",
        "session-123",
        "-o",
        "/tmp/resume-output.md",
        "--output-schema",
        "/tmp/schema.json",
        PROMPT,
    ]);

    assert_eq!(
        cli.last_message_file,
        Some(PathBuf::from("/tmp/resume-output.md"))
    );
    assert_eq!(cli.output_schema, Some(PathBuf::from("/tmp/schema.json")));
    let Some(Command::Resume(args)) = cli.command else {
        panic!("expected resume command");
    };
    assert_eq!(args.session_id.as_deref(), Some("session-123"));
    assert_eq!(args.prompt.as_deref(), Some(PROMPT));
}

#[test]
fn parses_config_isolation_flags() {
    let cli = Cli::parse_from([
        "codex-exec",
        "--ignore-user-config",
        "--ignore-rules",
        "summarize",
    ]);

    assert!(cli.ignore_user_config);
    assert!(cli.ignore_rules);
}

#[test]
fn parses_no_tools_for_new_session() {
    let cli = Cli::parse_from(["codex-exec", "--no-tools", "summarize"]);

    assert!(cli.no_tools);
    assert!(cli.command.is_none());
}

#[test]
fn help_documents_no_tools() {
    let help = Cli::command().render_long_help().to_string();

    assert!(help.contains("--no-tools"));
    assert!(help.contains("Disable all model-visible tools for this run"));
}

#[test]
fn removed_full_auto_flag_reports_migration_path() {
    let cli = Cli::parse_from(["codex-exec", "--full-auto", "summarize"]);

    assert_eq!(
        cli.removed_full_auto_warning(),
        Some("warning: `--full-auto` is deprecated; use `--sandbox workspace-write` instead.")
    );
}
