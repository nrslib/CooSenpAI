use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use coosenpai_core::config::{Config, ConfigPaths};
use coosenpai_core::logging::FileLogger;
use coosenpai_core::prompts::{
    build_companion_prompt, build_observer_prompt, companion_schema, observer_schema,
    observer_system_prompt, CompanionPromptData, ObserverPromptFrame,
};
use coosenpai_core::provider::{
    resolve_login_shell_path, ProviderCall, ProviderClient, ProviderError, ProviderErrorKind,
    SessionRequest,
};
use coosenpai_core::state::{parse_visual_observation, ActivityTriggerKind, ObservationLimits};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

mod app_config;
mod chat_command;
mod config_command;
mod error_output;
mod eval_observations;
mod eval_support;
mod memory_command;
mod platform;
mod provider_support;
#[cfg(unix)]
mod shutdown;
mod watch;
mod watch_targets;
use eval_support::{
    changes_mention_reasons, contains_question, eval_case_selected, find_images,
    find_optional_images, mention_reasons, observation_forbidden_reasons,
};
use provider_support::make_provider;

#[derive(Debug, Parser)]
#[command(name = "coosenpai", version, about = "CooSenpAI ambient companion")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Watch(WatchArgs),
    Chat(ChatArgs),
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    Eval(EvalArgs),
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },
    Conversation {
        #[command(subcommand)]
        action: ConversationAction,
    },
}

#[derive(Debug, Args)]
struct WatchArgs {
    #[command(subcommand)]
    action: Option<WatchAction>,
}

#[derive(Debug, Subcommand)]
enum WatchAction {
    Targets {
        #[command(subcommand)]
        action: WatchTargetAction,
    },
}

#[derive(Debug, Subcommand)]
enum WatchTargetAction {
    Add { application: String },
    Remove { application: String },
    List,
}

#[derive(Debug, Args)]
struct ChatArgs {
    message: Option<String>,
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    Set { key: String, value: String },
}

#[derive(Debug, Subcommand)]
enum MemoryAction {
    List,
    Confirm {
        candidate_id: String,
        confirmation_id: String,
    },
    Reject {
        candidate_id: String,
    },
    ConfirmUpdate {
        update_id: String,
        confirmation_id: String,
    },
    Delete {
        fact_id: String,
        confirmation_id: String,
    },
    Consolidate {
        #[arg(long)]
        period: String,
    },
}

#[derive(Debug, Subcommand)]
enum ConversationAction {
    Reset,
}

#[derive(Debug, Args)]
struct EvalArgs {
    agent: EvalAgent,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    #[arg(long, value_name = "PATH")]
    persona: Option<PathBuf>,
    #[arg(long, value_name = "N", default_value_t = 1)]
    runs: usize,
    #[arg(long, value_name = "NAME")]
    case_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum EvalAgent {
    Observer,
    Companion,
    Memory,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("{}", error_output::format(&error));
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    let (home, paths, config) = app_config::load()?;
    let logger = Arc::new(FileLogger::new(paths.log.clone())?);
    let result = match cli.command {
        Command::Config { action: None } => print_json(&config),
        Command::Config {
            action: Some(ConfigAction::Set { key, value }),
        } => config_command::run(&paths, &key, &value),
        Command::Chat(args) => chat_command::run(&paths, config, args, logger.clone()).await,
        Command::Watch(WatchArgs { action: None }) => {
            watch::run(&home, &paths, config, logger).await
        }
        Command::Watch(WatchArgs {
            action: Some(WatchAction::Targets { action }),
        }) => watch_targets::run(&paths, action),
        Command::Eval(args) => eval(&paths, config, args).await,
        Command::Memory { action } => match action {
            MemoryAction::List => memory_command::list(&paths),
            MemoryAction::Confirm {
                candidate_id,
                confirmation_id,
            } => memory_command::confirm(&paths, &config, &candidate_id, &confirmation_id),
            MemoryAction::Reject { candidate_id } => memory_command::reject(&paths, &candidate_id),
            MemoryAction::ConfirmUpdate {
                update_id,
                confirmation_id,
            } => memory_command::confirm_update(&paths, &config, &update_id, &confirmation_id),
            MemoryAction::Delete {
                fact_id,
                confirmation_id,
            } => memory_command::delete(&paths, &fact_id, &confirmation_id),
            MemoryAction::Consolidate { period } => {
                let path = resolve_login_shell_path(CancellationToken::new()).await;
                let provider = make_provider(
                    &config.companion.provider,
                    config.companion.executable.as_deref(),
                    &path,
                )?;
                memory_command::consolidate(&paths, &config, &period, provider).await
            }
        },
        Command::Conversation {
            action: ConversationAction::Reset,
        } => {
            let _watch_guard =
                coosenpai_core::persistence::WatchLock::acquire(&paths.watch_lock)
                    .map_err(|_| anyhow::anyhow!("watch 実行中は会話をリセットできません"))?;
            let generation = coosenpai_core::conversation_archive::reset_conversation(
                &paths,
                chrono::Utc::now(),
            )?;
            println!("会話をリセットしました（世代: {generation}）");
            Ok(())
        }
    };
    provider_support::shutdown_provider_bridge().await;
    result
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

async fn eval(paths: &ConfigPaths, config: Config, args: EvalArgs) -> Result<()> {
    if args.runs == 0 {
        anyhow::bail!("--runs は 1 以上で指定してください")
    }
    let (agent_name, provider_default, model_default, effort_default, executable) = match args.agent
    {
        EvalAgent::Observer => (
            "observer",
            config.observer.provider.clone(),
            config.observer.model.clone(),
            config.observer.effort.clone(),
            config.observer.executable.clone(),
        ),
        EvalAgent::Companion => (
            "companion",
            config.companion.provider.clone(),
            config.companion.model.clone(),
            config.companion.effort.clone(),
            config.companion.executable.clone(),
        ),
        EvalAgent::Memory => (
            "memory",
            config.companion.provider.clone(),
            config.companion.model.clone(),
            config.companion.effort.clone(),
            config.companion.executable.clone(),
        ),
    };
    let provider_name = args.provider.as_deref().unwrap_or(&provider_default);
    let effort = args.effort.clone().unwrap_or(effort_default);
    let path_value = resolve_login_shell_path(CancellationToken::new()).await;
    let executable = if provider_name == provider_default.as_str() {
        executable.as_deref()
    } else {
        None
    };
    let provider = make_provider(provider_name, executable, &path_value)?;
    let model = match args.model {
        Some(model) => model,
        None if args.provider.is_some() => provider
            .resolve_capabilities(CancellationToken::new(), Duration::from_secs(10))
            .await
            .map_err(|error| anyhow::anyhow!(error.message))?
            .map(|capabilities| capabilities.default_model)
            .unwrap_or(model_default),
        None => model_default,
    };
    let root = repository_root();
    let persona_path = args
        .persona
        .clone()
        .unwrap_or_else(|| root.join("builtins/prompts/facets/personas/coo-chan.md"));
    if matches!(args.agent, EvalAgent::Companion) && !persona_path.is_file() {
        anyhow::bail!("persona を読み込めません: {}", persona_path.display());
    }
    let cases_directory = root.join("eval").join(agent_name).join("cases");
    let mut case_names = std::fs::read_dir(&cases_directory)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| eval_case_selected(name, args.case_name.as_deref()))
        .collect::<Vec<_>>();
    case_names.sort();
    if case_names.is_empty() {
        anyhow::bail!("評価ケースがありません: {}", cases_directory.display())
    }
    println!(
        "agent={agent_name} provider={provider_name} model={model} effort={effort} runs={}",
        args.runs
    );
    let total_cases = case_names.len();
    let mut failures = 0usize;
    for case_name in case_names {
        let case_directory = cases_directory.join(&case_name);
        let input = if matches!(args.agent, EvalAgent::Observer) {
            read_optional_json(&case_directory.join("input.json"))?
        } else {
            read_json(&case_directory.join("input.json"))?
        };
        let expected = if matches!(args.agent, EvalAgent::Observer) {
            read_json(&case_directory.join("expect.json"))?
        } else {
            input.get("expect").cloned().unwrap_or_else(|| json!({}))
        };
        let mut results = Vec::new();
        for _ in 0..args.runs {
            results.push(
                run_eval_case(
                    EvalCase {
                        agent: args.agent,
                        provider: provider.clone(),
                        model: &model,
                        effort: &effort,
                        persona_path: matches!(args.agent, EvalAgent::Companion)
                            .then_some(persona_path.as_path()),
                        config: &config,
                    },
                    &case_directory,
                    &input,
                )
                .await,
            );
        }
        let verdicts = results
            .iter()
            .map(|result| judge_eval(args.agent, result, &expected))
            .collect::<Vec<_>>();
        let passed = verdicts.iter().all(|verdict| verdict.ok);
        if !passed {
            failures += 1;
        }
        println!("\n{} {}", if passed { "PASS" } else { "FAIL" }, case_name);
        for (index, (result, verdict)) in results.iter().zip(&verdicts).enumerate() {
            println!(
                "  #{} {} {}ms  {}",
                index + 1,
                if verdict.ok { "ok" } else { "NG" },
                result.elapsed_ms,
                result.summary
            );
            for reason in &verdict.reasons {
                println!("      - {reason}");
            }
        }
    }
    println!(
        "\n{}/{} ケース合格",
        total_cases.saturating_sub(failures),
        total_cases
    );
    let _ = paths;
    if failures > 0 {
        anyhow::bail!("評価に失敗しました")
    }
    Ok(())
}

#[derive(Debug)]
struct EvalResult {
    elapsed_ms: u128,
    value: Option<Value>,
    text: String,
    summary: String,
}

#[derive(Debug)]
struct Verdict {
    ok: bool,
    reasons: Vec<String>,
}

struct EvalCase<'a> {
    agent: EvalAgent,
    provider: Arc<dyn ProviderClient>,
    model: &'a str,
    effort: &'a str,
    persona_path: Option<&'a Path>,
    config: &'a Config,
}

async fn run_eval_case(case: EvalCase<'_>, case_directory: &Path, input: &Value) -> EvalResult {
    let started = Instant::now();
    let result: Result<Value, ProviderError> = match case.agent {
        EvalAgent::Observer => {
            let images = match find_images(case_directory) {
                Ok(images) => images,
                Err(error) => return eval_error(started, error.to_string()),
            };
            let frames = images
                .iter()
                .enumerate()
                .map(|(index, _)| ObserverPromptFrame {
                    index: index + 1,
                    relative_seconds: index as f64 * 15.0,
                    trigger: Some(ActivityTriggerKind::Timer),
                    front_app: None,
                    app: input.get("app").and_then(Value::as_str).map(str::to_owned),
                    target: input
                        .get("target")
                        .and_then(Value::as_str)
                        .unwrap_or("fullscreen")
                        .to_owned(),
                    ocr_text: None,
                })
                .collect::<Vec<_>>();
            let previous = input.get("previousObservation");
            let observer_limits = ObservationLimits {
                text_excerpt_max_chars: case.config.observer.text_excerpt_max_chars,
                text_excerpt_max_count: case.config.observer.text_excerpt_max_count,
                text_total_max_chars: case.config.observer.text_total_max_chars,
                changes_max_count: case.config.observer.changes_max_count,
            };
            let prompt = build_observer_prompt(
                &frames,
                previous,
                observer_limits.outline_max_bytes(),
                observer_limits.changes_max_count,
            );
            case.provider
                .call(
                    ProviderCall {
                        system_prompt: observer_system_prompt(),
                        prompt,
                        images: images.into_iter().map(Into::into).collect(),
                        tools_disabled: true,
                        output_schema: Some(observer_schema()),
                        session: SessionRequest::Ephemeral,
                        model: Some(case.model.to_owned()),
                        effort: Some(case.effort.to_owned()),
                        timeout: Duration::from_millis(case.config.observer.timeout_ms),
                        tutorial_response_key: None,
                    },
                    CancellationToken::new(),
                )
                .await
                .and_then(|response| {
                    response
                        .value
                        .ok_or_else(|| invalid_output("observer の出力がありません"))
                })
                .and_then(|value| {
                    parse_visual_observation(value.clone(), observer_limits)
                        .map(|_| value)
                        .map_err(|_| invalid_output("observer の構造化出力が不正です"))
                })
        }
        EvalAgent::Companion => {
            let mut images = match find_optional_images(case_directory) {
                Ok(images) => images,
                Err(error) => return eval_error(started, error.to_string()),
            };
            if let Some(relative) = input.get("imageFixture").and_then(Value::as_str) {
                images.push(case_directory.join(relative));
            }
            let persona = match case.persona_path {
                Some(path) => {
                    let document = match std::fs::read_to_string(path) {
                        Ok(value) => value,
                        Err(error) => return eval_error(started, error.to_string()),
                    };
                    let id = path
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or("coo-chan");
                    match coosenpai_core::persona::parse_persona(id, &document) {
                        Ok(value) => value,
                        Err(error) => return eval_error(started, error.to_string()),
                    }
                }
                None => match coosenpai_core::persona::parse_persona("coo-chan", "") {
                    Ok(value) => value,
                    Err(error) => return eval_error(started, error.to_string()),
                },
            };
            let observations = input
                .get("observations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let previous_conversation = input
                .get("previousConversation")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .map(coosenpai_core::prompts::ordered_json_string)
                        .collect::<Vec<_>>()
                        .join("\n")
                });
            let data = CompanionPromptData {
                companion_name: case.config.companion.display_name.clone(),
                observations: observations.clone(),
                observation_log_directory: None,
                omitted_observations: Some(Vec::new()),
                compact_observations: false,
                omitted_summary: None,
                omitted_ids: Vec::new(),
                last_observation: observations.last().cloned(),
                observation_frame_paths: std::collections::HashMap::new(),
                elapsed_ms: input
                    .get("elapsedSinceMeaningfulChangeMs")
                    .and_then(Value::as_u64),
                stuck_after_ms: Some(case.config.companion.stuck_after_ms),
                repeated_error_count: input
                    .get("repeatedErrorCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize,
                previous_summary: input
                    .get("previousSummary")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                recent_conversation_jsonl: previous_conversation,
                user_message: input
                    .get("userMessage")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                user_message_id: input
                    .get("userMessageId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                user_attachment: !images.is_empty(),
                attachment_ocr_text: None,
                pending_frame_context: None,
                memory_block: input
                    .get("memoryBlock")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                context_notice: input
                    .get("contextNotice")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            };
            let system_prompt = coosenpai_core::prompts::companion_system_prompt(
                &case.config.companion.assertiveness,
                &case.config.companion.display_name,
                &persona.body,
            );
            case.provider
                .call(
                    ProviderCall {
                        system_prompt,
                        prompt: build_companion_prompt(&data),
                        images: images.into_iter().map(Into::into).collect(),
                        tools_disabled: true,
                        output_schema: Some(companion_schema()),
                        session: SessionRequest::New,
                        model: Some(case.model.to_owned()),
                        effort: Some(case.effort.to_owned()),
                        timeout: Duration::from_millis(case.config.companion.timeout_ms),
                        tutorial_response_key: None,
                    },
                    CancellationToken::new(),
                )
                .await
                .and_then(|response| {
                    response
                        .value
                        .ok_or_else(|| invalid_output("companion の出力がありません"))
                })
        }
        EvalAgent::Memory => {
            let source = input
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let prompt = if input.get("kind").and_then(Value::as_str) == Some("weekly") {
                coosenpai_core::memory::weekly_summary_prompt(source.as_bytes())
            } else {
                coosenpai_core::memory::daily_summary_prompt(source.as_bytes())
            };
            match prompt {
                Ok(prompt) => case
                    .provider
                    .call(
                        ProviderCall {
                            system_prompt: "あなたは CooSenpAI の記憶を整理します。指定された JSON だけを返してください。".to_owned(),
                            prompt,
                            images: Vec::new(),
                            tools_disabled: true,
                            output_schema: Some(coosenpai_core::memory::memory_summary_schema()),
                            session: SessionRequest::Ephemeral,
                            model: Some(case.model.to_owned()),
                            effort: Some(case.effort.to_owned()),
                            timeout: Duration::from_millis(case.config.companion.timeout_ms),
                            tutorial_response_key: None,
                        },
                        CancellationToken::new(),
                    )
                    .await
                    .and_then(|response| {
                        response
                            .value
                            .ok_or_else(|| invalid_output("memory の出力がありません"))
                    }),
                Err(error) => Err(invalid_output(&error.to_string())),
            }
        }
    };
    match result {
        Ok(value) => EvalResult {
            elapsed_ms: started.elapsed().as_millis(),
            text: value.to_string(),
            summary: if case.agent == EvalAgent::Observer {
                format!(
                    "visual activity={}",
                    value.get("activity").and_then(Value::as_str).unwrap_or("")
                )
            } else if case.agent == EvalAgent::Companion {
                format!(
                    "emit={} {}",
                    value.get("emit").and_then(Value::as_bool).unwrap_or(false),
                    value.get("message").and_then(Value::as_str).unwrap_or("")
                )
            } else {
                format!(
                    "summary {}",
                    value.get("text").and_then(Value::as_str).unwrap_or("")
                )
            },
            value: Some(value),
        },
        Err(error) => EvalResult {
            elapsed_ms: started.elapsed().as_millis(),
            text: String::new(),
            summary: format!("ERROR {error}"),
            value: None,
        },
    }
}

fn eval_error(started: Instant, message: String) -> EvalResult {
    EvalResult {
        elapsed_ms: started.elapsed().as_millis(),
        value: None,
        text: String::new(),
        summary: format!("ERROR {message}"),
    }
}

fn judge_eval(agent: EvalAgent, result: &EvalResult, expected: &Value) -> Verdict {
    let mut reasons = Vec::new();
    let Some(value) = result.value.as_ref() else {
        return Verdict {
            ok: false,
            reasons: vec!["実行エラー".to_owned()],
        };
    };
    if matches!(agent, EvalAgent::Observer) {
        let minimum = expected
            .get("minOutlineChars")
            .and_then(Value::as_u64)
            .unwrap_or(1) as usize;
        if value
            .get("outline")
            .and_then(Value::as_str)
            .map_or(0, |outline| outline.chars().count())
            < minimum
        {
            reasons.push(format!("outline が {minimum} 文字以上必要"));
        }
        if let Some(events) = expected.get("events").and_then(Value::as_array) {
            for event in events.iter().filter_map(Value::as_str) {
                if !value
                    .get("events")
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        items
                            .iter()
                            .any(|item| item.get("type").and_then(Value::as_str) == Some(event))
                    })
                {
                    reasons.push(format!("event {event} がない"));
                }
            }
        }
        if let Some(wake) = expected.get("wakeCompanion").and_then(Value::as_bool) {
            if value.get("wakeCompanion").and_then(Value::as_bool) != Some(wake) {
                reasons.push("wakeCompanion が期待値と異なる".to_owned());
            }
        }
        reasons.extend(changes_mention_reasons(value, expected));
        reasons.extend(observation_forbidden_reasons(value, expected));
    } else if let Some(emit) = expected.get("emit").and_then(Value::as_bool) {
        if value.get("emit").and_then(Value::as_bool) != Some(emit) {
            reasons.push(format!("emit が {emit} であるべき"));
        }
    }
    if matches!(agent, EvalAgent::Companion) {
        let message = value.get("message").and_then(Value::as_str).unwrap_or("");
        if let Some(minimum) = expected.get("minSentences").and_then(Value::as_u64) {
            let sentences = message
                .chars()
                .filter(|character| matches!(character, '。' | '！' | '？' | '!' | '?'))
                .count();
            if sentences < minimum as usize {
                reasons.push(format!("返答が {minimum} 文以上必要"));
            }
        }
        if expected.get("mustAskQuestion").and_then(Value::as_bool) == Some(true)
            && !contains_question(message)
        {
            reasons.push("質問で会話を広げていない".to_owned());
        }
    }
    let text = if matches!(agent, EvalAgent::Companion) {
        value.get("message").and_then(Value::as_str).unwrap_or("")
    } else if matches!(agent, EvalAgent::Memory) {
        value.get("text").and_then(Value::as_str).unwrap_or("")
    } else {
        &result.text
    };
    reasons.extend(mention_reasons(text, expected));
    if let Some(words) = expected.get("outlineMustMention").and_then(Value::as_array) {
        let outline = value.get("outline").and_then(Value::as_str).unwrap_or("");
        for word in words.iter().filter_map(Value::as_str) {
            if !outline.to_lowercase().contains(&word.to_lowercase()) {
                reasons.push(format!("画面文字「{word}」を含んでいない"));
            }
        }
    }
    Verdict {
        ok: reasons.is_empty(),
        reasons,
    }
}

fn load_persona(
    paths: &ConfigPaths,
    name: &str,
) -> Result<coosenpai_core::persona::PersonaProfile> {
    coosenpai_core::persona::load_persona(paths, name).map_err(anyhow::Error::from)
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

fn read_json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn read_optional_json(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    read_json(path)
}

fn invalid_output(message: &str) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::InvalidOutput,
        message: message.to_owned(),
    }
}

