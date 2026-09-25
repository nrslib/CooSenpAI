use serde::Deserialize;
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct Request {
    v: u8,
    id: String,
    #[serde(default)]
    session: Option<String>,
    op: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug)]
struct Options {
    action: String,
    novelty: f64,
    relevance: f64,
    readiness: String,
    required_environment: Option<(String, String)>,
    delay_first_evaluate_ms: Option<u64>,
    delay_evaluate_number: u64,
    delay_once_file: Option<PathBuf>,
    exit_after_evaluate: bool,
    pid_file: Option<PathBuf>,
    state_file: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            action: "silence".to_owned(),
            novelty: 0.8,
            relevance: 0.4,
            readiness: "ready".to_owned(),
            required_environment: None,
            delay_first_evaluate_ms: None,
            delay_evaluate_number: 1,
            delay_once_file: None,
            exit_after_evaluate: false,
            pid_file: None,
            state_file: None,
        }
    }
}

fn main() {
    let options = match parse_options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    if let Some((key, expected)) = &options.required_environment {
        if std::env::var(key).ok().as_deref() != Some(expected.as_str()) {
            eprintln!("環境変数 {key} が期待値と一致しません");
            std::process::exit(2);
        }
    }
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    if let Err(error) = run(stdin.lock(), &mut stdout, &options) {
        eprintln!("固定応答プラグインの通信に失敗しました: {error}");
        std::process::exit(1);
    }
}

fn run<R, W>(input: R, output: &mut W, options: &Options) -> io::Result<()>
where
    R: BufRead,
    W: Write,
{
    let pid = std::process::id();
    if let Some(path) = &options.pid_file {
        append_line(path, &pid.to_string())?;
    }
    let mut evaluate_count = 0_u64;
    for line in input.lines() {
        let line = match line {
            Ok(line) if !line.trim().is_empty() => line,
            Ok(_) => continue,
            Err(error) => {
                return Err(error);
            }
        };
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                eprintln!("要求の JSON が不正です: {error}");
                continue;
            }
        };
        if request.op == "evaluate" {
            evaluate_count = evaluate_count.saturating_add(1);
            if evaluate_count == options.delay_evaluate_number
                && options
                    .delay_first_evaluate_ms
                    .is_some_and(|delay| delay > 0)
                && should_delay_once(options)
            {
                std::thread::sleep(Duration::from_millis(
                    options.delay_first_evaluate_ms.unwrap_or_default(),
                ));
            }
        }
        let response = response_for(request, options);
        if let Some(path) = &options.state_file {
            append_line(
                path,
                &serde_json::to_string(&json!({
                    "pid": pid,
                    "op": if response["result"].get("input_id").is_some() {
                        "evaluate"
                    } else {
                        "other"
                    },
                    "input_id": response["result"]["input_id"]
                }))?,
            )?;
        }
        serde_json::to_writer(&mut *output, &response)
            .map_err(io::Error::other)
            .and_then(|_| output.write_all(b"\n"))
            .and_then(|_| output.flush())?;
        if options.exit_after_evaluate && response["result"].get("input_id").is_some() {
            return Ok(());
        }
    }
    Ok(())
}

fn should_delay_once(options: &Options) -> bool {
    let Some(path) = &options.delay_once_file else {
        return true;
    };
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .is_ok()
}

fn append_line(path: &PathBuf, line: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")
}

fn response_for(request: Request, options: &Options) -> Value {
    let base = json!({
        "v": request.v,
        "id": request.id,
        "session": request.session,
    });
    match request.op.as_str() {
        "evaluate" if request.v == 1 => {
            let input_id = request.params["input_id"].as_str().unwrap_or_default();
            json!({
                "v": base["v"],
                "id": base["id"],
                "session": base["session"],
                "ok": true,
                "result": {
                    "input_id": input_id,
                    "novelty": options.novelty,
                    "relevance": options.relevance,
                    "action": options.action,
                    "readiness": options.readiness,
                    "feedable": true,
                    "feed_target": "exact_image_sha256_case_memory",
                }
            })
        }
        "feed" if request.v == 1 => json!({
            "v": base["v"],
            "id": base["id"],
            "session": base["session"],
            "ok": true,
            "result": {"applied": true}
        }),
        "commit" if request.v == 1 => json!({
            "v": base["v"],
            "id": base["id"],
            "session": base["session"],
            "ok": true,
            "result": {"committed": true}
        }),
        _ => json!({
            "v": base["v"],
            "id": base["id"],
            "session": base["session"],
            "ok": false,
            "error": {"code": "unsupported-operation", "message": "未対応の操作です"}
        }),
    }
}

fn parse_options<I>(arguments: I) -> Result<Options, String>
where
    I: IntoIterator<Item = String>,
{
    let mut options = Options::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--action" => {
                let value = next_value(&mut arguments, &argument)?;
                options.action = match value.as_str() {
                    "notify" | "hold" | "silence" => value,
                    _ => {
                        return Err(
                            "--action は notify / hold / silence で指定してください".to_owned()
                        )
                    }
                };
            }
            "--novelty" => {
                let value = next_value(&mut arguments, &argument)?;
                options.novelty = probability(&value, "--novelty")?;
            }
            "--relevance" => {
                let value = next_value(&mut arguments, &argument)?;
                options.relevance = probability(&value, "--relevance")?;
            }
            "--readiness" => {
                let value = next_value(&mut arguments, &argument)?;
                if value.trim().is_empty() {
                    return Err("--readiness は空にできません".to_owned());
                }
                options.readiness = value;
            }
            "--require-environment" => {
                let value = next_value(&mut arguments, &argument)?;
                let (key, expected) = value
                    .split_once('=')
                    .filter(|(key, _)| !key.is_empty())
                    .ok_or_else(|| {
                        "--require-environment は KEY=VALUE で指定してください".to_owned()
                    })?;
                options.required_environment = Some((key.to_owned(), expected.to_owned()));
            }
            "--delay-first-evaluate-ms" => {
                let value = next_value(&mut arguments, &argument)?;
                options.delay_first_evaluate_ms = Some(value.parse::<u64>().map_err(|_| {
                    "--delay-first-evaluate-ms は整数で指定してください".to_owned()
                })?);
            }
            "--delay-evaluate-number" => {
                let value = next_value(&mut arguments, &argument)?;
                options.delay_evaluate_number = value
                    .parse::<u64>()
                    .map_err(|_| "--delay-evaluate-number は整数で指定してください".to_owned())?;
                if options.delay_evaluate_number == 0 {
                    return Err("--delay-evaluate-number は 1 以上で指定してください".to_owned());
                }
            }
            "--delay-once-file" => {
                options.delay_once_file =
                    Some(PathBuf::from(next_value(&mut arguments, &argument)?));
            }
            "--exit-after-evaluate" => options.exit_after_evaluate = true,
            "--pid-file" => {
                options.pid_file = Some(PathBuf::from(next_value(&mut arguments, &argument)?));
            }
            "--state-file" => {
                options.state_file = Some(PathBuf::from(next_value(&mut arguments, &argument)?));
            }
            _ => return Err(format!("未知の引数です: {argument}")),
        }
    }
    Ok(options)
}

fn next_value<I>(arguments: &mut I, argument: &str) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    arguments
        .next()
        .ok_or_else(|| format!("{argument} の値がありません"))
}

fn probability(value: &str, name: &str) -> Result<f64, String> {
    let value = value
        .parse::<f64>()
        .map_err(|_| format!("{name} は数値で指定してください"))?;
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(format!("{name} は 0 以上 1 以下で指定してください"))
    }
}
