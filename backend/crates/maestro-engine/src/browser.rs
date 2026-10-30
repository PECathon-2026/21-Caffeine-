//! Local browser evidence and process-owned development previews.
use crate::{media, tools::ToolCtx};
use base64::{engine::general_purpose::STANDARD, Engine};
use maestro_types::{MaestroError, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
fn err(s: impl ToString) -> MaestroError {
    MaestroError::Config(s.to_string())
}
fn python() -> PathBuf {
    std::env::var_os("MAESTRO_BROWSER_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local/share/maestro/langgraph-venv/bin/python")
        })
}
pub async fn inspect(root: &Path, args: &Value) -> Result<Value> {
    let mut child = tokio::process::Command::new(python())
        .args([
            "-I",
            "-c",
            include_str!("../../../workers/browser/inspect.py"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            err(format!(
                "Browser unavailable: {e}; run scripts/install-browser.py"
            ))
        })?;
    let mut input = child.stdin.take().unwrap();
    input.write_all(format!("{args}\n").as_bytes()).await?;
    drop(input);
    let mut output = child.stdout.take().unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(60),
        (&mut output)
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes),
    )
    .await
    .map_err(|_| err("Browser inspection timed out"))??;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(err("Browser response exceeds limit"));
    }
    let mut v: Value = serde_json::from_slice(&bytes).map_err(|_| {
        err("Browser failed; run scripts/install-browser.py to verify Playwright and Chromium")
    })?;
    if let Some(e) = v["error"].as_str() {
        return Err(err(e));
    }
    let data = v
        .as_object_mut()
        .unwrap()
        .remove("screenshot")
        .and_then(|v| v.as_str().map(String::from))
        .ok_or_else(|| err("Browser did not return screenshot"))?;
    let asset = media::import(
        root,
        "browser-screenshot.jpg",
        &STANDARD.decode(data).map_err(err)?,
    )?;
    v["imageId"] = json!(asset.image_id);
    v["reference"] = json!(asset.reference);
    child.wait().await?;
    Ok(v)
}
struct Preview {
    child: tokio::process::Child,
    _input: tokio::process::ChildStdin,
    pid: i32,
    url: String,
    logs: Arc<Mutex<String>>,
    reader: tokio::task::JoinHandle<()>,
}
impl Drop for Preview {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-self.pid, libc::SIGTERM);
        }
        let _ = self.child.start_kill();
        self.reader.abort();
        unsafe {
            libc::kill(-self.pid, libc::SIGKILL);
        }
    }
}
static PREVIEWS: LazyLock<Mutex<BTreeMap<PathBuf, Preview>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
pub fn active(path: &Path) -> bool {
    PREVIEWS
        .lock()
        .unwrap()
        .get_mut(path)
        .is_some_and(|p| p.child.try_wait().ok().flatten().is_none())
}
pub fn stop_all(root: &Path) {
    PREVIEWS.lock().unwrap().retain(|p, _| !p.starts_with(root));
}
pub fn status(workspace: &Path, stop: bool) -> Value {
    let mut all = PREVIEWS.lock().unwrap();
    if stop {
        return json!({"stopped":all.remove(workspace).is_some()});
    }
    match all.get_mut(workspace) {
        Some(p) => {
            json!({"running":p.child.try_wait().ok().flatten().is_none(),"url":p.url,"logs":p.logs.lock().unwrap().clone()})
        }
        None => json!({"running":false,"logs":"No managed preview in this workspace"}),
    }
}
pub async fn start(ctx: &ToolCtx, port: u16) -> Result<Value> {
    if active(&ctx.workspace) {
        return Ok(status(&ctx.workspace, false));
    }
    if !(1024..=65535).contains(&port) {
        return Err(err("Preview port must be 1024–65535"));
    }
    if std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
        return Err(err("Preview port is already in use; choose another port"));
    }
    let pkg: Value = serde_json::from_slice(&std::fs::read(ctx.workspace.join("package.json"))?)?;
    let script = pkg["scripts"]["dev"]
        .as_str()
        .ok_or_else(|| err("No package.json dev script"))?;
    let mut argv = vec!["npm".to_string(), "run".into(), "dev".into(), "--".into()];
    if script.split_whitespace().any(|s| s == "next") {
        argv.extend([
            "--hostname".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
        ]);
    } else if script.split_whitespace().any(|s| s == "vite") {
        argv.extend([
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
            "--strictPort".into(),
        ]);
    } else {
        return Err(err(
            "Automatic preview supports Next.js and Vite dev scripts",
        ));
    }
    let mut env = BTreeMap::from([
        (
            "PATH".to_string(),
            std::env::var("PATH").unwrap_or("/usr/bin:/bin".into()),
        ),
        ("HOME".into(), std::env::var("HOME").unwrap_or_default()),
        ("NODE_ENV".into(), "development".into()),
        ("NEXT_TELEMETRY_DISABLED".into(), "1".into()),
    ]);
    let mut secrets = Vec::new();
    for name in [
        ".env",
        ".env.development",
        ".env.local",
        ".env.development.local",
    ] {
        let p = ctx.root.join(name);
        if p.is_file() {
            for pair in dotenvy::from_path_iter(p)
                .map_err(|_| err("Cannot parse local application environment file"))?
            {
                let (k, v) =
                    pair.map_err(|_| err("Cannot parse local application environment file"))?;
                if v.len() >= 4 {
                    secrets.push(v.clone());
                }
                env.insert(k, v);
            }
        }
    }
    // Provider credentials never enter the app process.
    env.retain(|k, _| {
        !k.starts_with("MAESTRO_")
            && !matches!(
                k.as_str(),
                "OPENAI_API_KEY" | "ANTHROPIC_API_KEY" | "AZURE_OPENAI_API_KEY"
            )
    });
    let mut sandbox = ctx.sandbox.clone();
    sandbox.cwd = ctx.workspace.clone();
    sandbox.allow_network = true;
    if let Some(bwrap) = crate::sandbox::resolve_bwrap(&sandbox) {
        let mut wrapped = crate::sandbox::bwrap_args(&sandbox)?;
        // The monitor supplies this exact environment. Preserve it across bwrap
        // instead of placing application secrets in command-line arguments.
        wrapped.retain(|arg| arg != "--clearenv");
        wrapped.extend(argv);
        argv = vec![bwrap.to_string_lossy().into_owned()];
        argv.extend(wrapped);
        env.insert("HOME".into(), "/tmp/maestro-home".into());
    } else if sandbox.mode == maestro_types::config::EnforcementMode::Required {
        return Err(MaestroError::Sandbox(
            "bwrap is required for the preview but was not found".into(),
        ));
    }
    let mut child = tokio::process::Command::new(python())
        .args([
            "-u",
            "-I",
            "-c",
            include_str!("../../../workers/browser/preview.py"),
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(err)?;
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(format!("{}\n", json!({"argv":argv,"cwd":ctx.workspace,"env":env})).as_bytes())
        .await?;
    input.flush().await?;
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    tokio::time::timeout(Duration::from_secs(5), output.read_line(&mut first))
        .await
        .map_err(|_| err("Preview startup timed out"))??;
    let pid = serde_json::from_str::<Value>(&first)
        .map_err(|_| err("Preview could not launch npm"))?["pid"]
        .as_i64()
        .ok_or_else(|| err("Missing preview pid"))? as i32;
    let logs = Arc::new(Mutex::new(String::new()));
    let buffer = logs.clone();
    let reader = tokio::spawn(async move {
        let mut pending = String::new();
        let mut bytes = [0u8; 4096];
        while let Ok(n) = output.read(&mut bytes).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&bytes[..n])); // retain overlap before publishing so split secrets are redacted
            while let Some(end) = pending.find('\n') {
                let mut line = pending.drain(..=end).collect::<String>();
                for secret in &secrets {
                    line = line.replace(secret, "[redacted]");
                }
                let mut b = buffer.lock().unwrap();
                b.push_str(&line);
                if b.len() > 64000 {
                    let mut cut = b.len() - 64000;
                    while !b.is_char_boundary(cut) {
                        cut += 1;
                    }
                    b.drain(..cut);
                }
            }
            if pending.len() > 64000 {
                pending.clear();
                buffer
                    .lock()
                    .unwrap()
                    .push_str("\n[overlong log line omitted]\n");
            }
        }
    });
    let url = format!("http://127.0.0.1:{port}");
    let preview = Preview {
        child,
        _input: input,
        pid,
        url,
        logs,
        reader,
    };
    {
        let mut all = PREVIEWS.lock().unwrap();
        if all.len() >= 4 {
            return Err(err(
                "Stop an existing preview before starting another (limit 4)",
            ));
        }
        all.insert(ctx.workspace.clone(), preview);
    }
    for _ in 0..40 {
        if !active(&ctx.workspace) {
            break;
        }
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let mut result = status(&ctx.workspace, false);
    result["note"]=json!("Process status only; use browser_inspect to verify HTTP status, rendering and errors. Preview runs the project dev script locally with application environment.");
    Ok(result)
}

pub fn list(root: &Path) -> Value {
    let mut all = PREVIEWS.lock().unwrap();
    json!(all.iter_mut().filter(|(path,_)|path.starts_with(root)).map(|(path,p)|json!({"workspace":path,"url":p.url,"running":p.child.try_wait().ok().flatten().is_none(),"logs":p.logs.lock().unwrap().clone()})).collect::<Vec<_>>())
}
