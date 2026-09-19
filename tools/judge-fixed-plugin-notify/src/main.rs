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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn fixed_notify_response_is_stable() {
        let response = response_for(Request {
            v: 1,
            id: "r1".to_owned(),
            session: Some("s1".to_owned()),
            op: "evaluate".to_owned(),
            params: json!({"input_id": "frame-1"}),
        });
        assert_eq!(response["result"]["input_id"], "frame-1");
        assert_eq!(response["result"]["action"], "notify");
    }

    #[test]
    fn resident_loop_returns_one_response_for_each_request() {
        let input = concat!(
            r#"{"v":1,"id":"r1","session":"s1","op":"evaluate","params":{"input_id":"frame-1"}}"#,
            "\n",
            r#"{"v":1,"id":"r2","session":"s1","op":"evaluate","params":{"input_id":"frame-2"}}"#,
            "\n",
        );
        let mut output = Vec::new();
        run(Cursor::new(input), &mut output).expect("run");
        let responses = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Value>(line).expect("response"))
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["id"], "r1");
        assert_eq!(responses[1]["id"], "r2");
    }
}
