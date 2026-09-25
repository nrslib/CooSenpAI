use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

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

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    if let Err(error) = run(stdin.lock(), &mut stdout) {
        eprintln!("固定通知プラグインの通信に失敗しました: {error}");
        std::process::exit(1);
    }
}

fn run<R, W>(input: R, output: &mut W) -> io::Result<()>
where
    R: BufRead,
    W: Write,
{
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
        let response = response_for(request);
        serde_json::to_writer(&mut *output, &response)
            .map_err(io::Error::other)
            .and_then(|_| output.write_all(b"\n"))
            .and_then(|_| output.flush())?;
    }
    Ok(())
}

fn response_for(request: Request) -> Value {
    let base = json!({
        "v": request.v,
        "id": request.id,
        "session": request.session,
    });
    match request.op.as_str() {
        "evaluate" if request.v == 1 => json!({
            "v": base["v"],
            "id": base["id"],
            "session": base["session"],
            "ok": true,
            "result": {
                "input_id": request.params["input_id"],
                "novelty": 0.2,
                "relevance": 0.9,
                "action": "notify",
                "readiness": "ready",
                "feedable": true,
                "feed_target": "exact_image_sha256_case_memory"
            }
        }),
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
