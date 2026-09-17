//! A code workspace the agent works in. Every list, read, write and command runs inside
//! bubblewrap with the workspace at /work, no network and an empty environment, so the host
//! never follows a path or runs a command the model wrote.
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::LazyLock,
};

use anyhow::{Context, Result};
use regex::Regex;

pub const TIMEOUT_SECONDS: u32 = 20;
const OUTPUT_CHARS: usize = 3000;
const CAPTURE_BYTES: u64 = 64_000;
const WRITE_BYTES: usize = 32_000;

pub struct Output {
    pub exit: i32,
    pub text: String,
}

/// Runs `argv` in the sandbox with stdout and stderr merged; output is normalized for replay
/// and cut to its head and tail.
pub fn sandbox(ws: &Path, argv: &[&str], stdin: Option<&str>) -> Result<Output> {
    let ws = ws.canonicalize().context("workspace")?;
    let mut cmd = Command::new("bwrap");
    cmd.args([
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
    ])
    .args(["--ro-bind", "/usr", "/usr"])
    .args([
        "--symlink",
        "usr/bin",
        "/bin",
        "--symlink",
        "usr/lib",
        "/lib",
    ])
    .args([
        "--symlink",
        "usr/lib64",
        "/lib64",
        "--symlink",
        "usr/sbin",
        "/sbin",
    ])
    .args(["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"])
    .arg("--bind")
    .arg(&ws)
    .args(["/work", "--chdir", "/work"])
    .args(["--setenv", "PATH", "/usr/bin", "--setenv", "HOME", "/tmp"])
    .args([
        "--setenv",
        "LANG",
        "C.UTF-8",
        "--setenv",
        "PYTHONHASHSEED",
        "0",
    ])
    .args([
        "--setenv",
        "PYTHONDONTWRITEBYTECODE",
        "1",
        "--",
        "/bin/sh",
        "-c",
    ])
    .arg(format!(
        "exec 2>&1; ulimit -v 4000000; exec timeout -k 1 {TIMEOUT_SECONDS} \"$@\""
    ))
    .arg("sh")
    .args(argv)
    .stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    let mut child = cmd.spawn().context("bwrap")?;
    if let Some(input) = stdin {
        child.stdin.take().unwrap().write_all(input.as_bytes())?;
    }
    let mut buf = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .take(CAPTURE_BYTES)
        .read_to_end(&mut buf)?;
    if buf.len() as u64 >= CAPTURE_BYTES {
        let _ = child.kill();
    }
    let exit = child.wait()?.code().unwrap_or(-1);
    let mut text = normalize(&String::from_utf8_lossy(&buf));
    if exit == 124 {
        text.push_str(&format!("\n(timed out after {TIMEOUT_SECONDS} s)"));
    }
    Ok(Output {
        exit,
        text: cut(&text),
    })
}

static TIMING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"( in )\d+\.\d+s\b").unwrap());
static ADDRESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"0x[0-9a-fA-F]{6,}").unwrap());

fn normalize(text: &str) -> String {
    let text = TIMING.replace_all(text, "${1}0.000s");
    ADDRESS.replace_all(&text, "0x?").into_owned()
}

fn cut(text: &str) -> String {
    let n = text.chars().count();
    if n <= OUTPUT_CHARS {
        return text.to_string();
    }
    let half = OUTPUT_CHARS / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(n - half).collect();
    format!("{head}\n[... {} chars cut ...]\n{tail}", n - OUTPUT_CHARS)
}

pub fn list(ws: &Path) -> Result<Output> {
    sandbox(
        ws,
        &[
            "sh",
            "-c",
            "find . -name __pycache__ -prune -o -type f -print | sort",
        ],
        None,
    )
}

pub fn read(ws: &Path, path: &str) -> Result<Output> {
    sandbox(ws, &["cat", "--", path], None)
}

pub fn write(ws: &Path, path: &str, content: &str) -> Result<Output> {
    if content.len() > WRITE_BYTES {
        return Ok(Output {
            exit: 1,
            text: format!("write refused: over {WRITE_BYTES} bytes"),
        });
    }
    sandbox(
        ws,
        &[
            "sh",
            "-c",
            "mkdir -p -- \"$(dirname -- \"$1\")\" && cat > \"$1\" && echo \"wrote $(wc -c < \"$1\") bytes\"",
            "sh",
            path,
        ],
        Some(content),
    )
}

pub fn run(ws: &Path, command: &str) -> Result<Output> {
    sandbox(ws, &["sh", "-c", command], None)
}
