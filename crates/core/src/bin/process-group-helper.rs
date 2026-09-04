#![allow(clippy::print_stdout)]

use coosenpai_core::persistence::SiblingLock;
use std::env;
use std::io::{self, Read, Write};
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("--spawn-descendant") => {
            let Some(marker) = arguments.next() else {
                std::process::exit(2);
            };
            let command = format!("sleep 2; touch '{}'", marker.replace('\'', "'\\''"));
            if Command::new("/bin/sh")
                .args(["-c", &command])
                .spawn()
                .is_err()
            {
                std::process::exit(1);
            }
            println!("descendant-started");
        }
        Some("--stdout-bytes") => {
            let Some(count) = arguments
                .next()
                .and_then(|value| value.parse::<usize>().ok())
            else {
                std::process::exit(2);
            };
            if write_bytes(io::stdout(), count).is_err() {
                std::process::exit(1);
            }
        }
        Some("--stderr-bytes") => {
            let Some(count) = arguments
                .next()
                .and_then(|value| value.parse::<usize>().ok())
            else {
                std::process::exit(2);
            };
            if write_bytes(io::stderr(), count).is_err() {
                std::process::exit(1);
            }
        }
        Some("--delayed-output") => {
            if write_bytes(io::stdout(), 5).is_err() {
                std::process::exit(1);
            }
            thread::sleep(Duration::from_millis(50));
            if write_bytes(io::stderr(), 6).is_err() {
                std::process::exit(1);
            }
        }
        Some("--read-stdin") => {
            let mut input = Vec::new();
            if io::stdin().read_to_end(&mut input).is_err() {
                std::process::exit(1);
            }
            if input != b"stdin\n" {
                std::process::exit(1);
            }
        }
        Some("--sleep") => {
            let Some(milliseconds) = arguments.next().and_then(|value| value.parse::<u64>().ok())
            else {
                std::process::exit(2);
            };
            thread::sleep(Duration::from_millis(milliseconds));
        }
        Some("--hold-lock") => {
            let (Some(lock_path), Some(ready_path), Some(milliseconds)) =
                (arguments.next(), arguments.next(), arguments.next())
            else {
                std::process::exit(2);
            };
            let duration = match milliseconds.parse::<u64>() {
                Ok(value) => value,
                Err(_) => std::process::exit(2),
            };
            let _lock = match SiblingLock::acquire(lock_path.as_ref()) {
                Ok(lock) => lock,
                Err(_) => std::process::exit(1),
            };
            if std::fs::write(ready_path, b"ready").is_err() {
                std::process::exit(1);
            }
            thread::sleep(Duration::from_millis(duration));
        }
        _ => std::process::exit(2),
    }
}

fn write_bytes(mut writer: impl Write, count: usize) -> io::Result<()> {
    let chunk = [b'x'; 8192];
    let mut remaining = count;
    while remaining > 0 {
        let length = remaining.min(chunk.len());
        writer.write_all(&chunk[..length])?;
        remaining -= length;
    }
    writer.flush()
}
