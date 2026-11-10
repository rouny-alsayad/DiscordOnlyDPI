#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]

macro_rules! println {
    () => {{
        crate::last_log::write_line(false, String::new());
    }};
    ($($arg:tt)*) => {{
        crate::last_log::write_line(false, format!($($arg)*));
    }};
}

macro_rules! eprintln {
    () => {{
        crate::last_log::write_line(true, String::new());
    }};
    ($($arg:tt)*) => {{
        crate::last_log::write_line(true, format!($($arg)*));
    }};
}

mod last_log {
    use std::{
        env,
        fs::{self, File, OpenOptions},
        io::{self, Write},
        path::PathBuf,
        sync::{Mutex, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };

    static LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();

    pub fn init() -> io::Result<PathBuf> {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("DiscordOnlyDPI");
        fs::create_dir_all(&base)?;

        let path = base.join("last-run.log");

        // DiscordOnlyDPI owns this directory's diagnostic logs. Keep exactly one.
        if let Ok(entries) = fs::read_dir(&base) {
            for entry in entries.flatten() {
                let old = entry.path();
                if old != path
                    && old.is_file()
                    && old.extension().and_then(|v| v.to_str()) == Some("log")
                {
                    let _ = fs::remove_file(old);
                }
            }
        }

        // Truncate the previous run, then reopen in append mode so child processes
        // can safely append to the same single file.
        File::create(&path)?;
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let slot = LOG_FILE.get_or_init(|| Mutex::new(None));
        if let Ok(mut guard) = slot.lock() {
            *guard = Some(file);
        }

        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        write_line(false, "=== DiscordOnlyDPI last run ===".to_string());
        write_line(false, format!("Started (unix): {started}"));
        Ok(path)
    }

    pub fn write_line(is_error: bool, message: String) {
        if is_error {
            std::eprintln!("{message}");
            write_file_line(&format!("[ERROR] {message}"));
        } else {
            std::println!("{message}");
            write_file_line(&message);
        }
    }

    /// Writes to the log file only; used for Discord's own output.
    pub fn write_file_line(message: &str) {
        if let Some(slot) = LOG_FILE.get() {
            if let Ok(mut guard) = slot.lock() {
                if let Some(file) = guard.as_mut() {
                    let _ = writeln!(file, "{message}");
                    let _ = file.flush();
                }
            }
        }
    }

    pub fn open_append_file() -> io::Result<File> {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("DiscordOnlyDPI");
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(base.join("last-run.log"))
    }
}

use reqwest::{header, Client, Proxy};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    env,
    error::Error,
    fs::{self, File},
    io::{Cursor, Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::sleep,
};
use zip::ZipArchive;

const FRONTEND_PORT: u16 = 17891;
const UPDATER_HTTP_PORT: u16 = 17893;
const BYEDPI_PORT: u16 = 17892;
const BYEDPI_URL: &str =
    "https://github.com/hufrea/byedpi/releases/download/v0.17.3/byedpi-17.3-x86_64-w64.zip";
const BYEDPI_SHA256: &str = "eb53ceeeb981cc6735ac24bb1e51e725280b86630e80fdf19ddc4ee4a5b54ef4";
const BYEDPI_ARGS: &[&str] = &[
    "--split",
    "1",
    "--disorder",
    "3+s",
    "--mod-http=h,d",
    "--auto=torst",
    "--tlsrec",
    "1+s",
];
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
// A Discord that exits this soon without ever loading its UI failed to start.
const STARTUP_GRACE: Duration = Duration::from_secs(60);
// A Discord that has run this long without a fatal error is treated as started,
// even if its log format changes and the started marker is never seen.
const STARTUP_ASSUME_OK: Duration = Duration::from_secs(120);

type AnyError = Box<dyn Error + Send + Sync>;

#[derive(Default)]
struct DnsCache {
    entries: Mutex<HashMap<String, (Instant, Ipv4Addr)>>,
}

#[derive(Clone)]
struct ProxyState {
    http: Client,
    dns: Arc<DnsCache>,
}

#[tokio::main]
async fn main() -> Result<(), AnyError> {
    #[cfg(not(target_os = "windows"))]
    {
        println!(
            "DiscordOnlyDPI v{} currently targets Windows 10/11.",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        let log_path = last_log::init()?;
        std::panic::set_hook(Box::new(|info| {
            crate::last_log::write_line(true, format!("PANIC: {info}"));
        }));
        println!("Log file: {}", log_path.display());

        match run_windows().await {
            Ok(()) => Ok(()),
            Err(error) => {
                eprintln!("Fatal error: {error}");
                Err(error)
            }
        }
    }
}

#[cfg(target_os = "windows")]
async fn run_windows() -> Result<(), AnyError> {
    println!("DiscordOnlyDPI v{}", env!("CARGO_PKG_VERSION"));
    println!("Mode: Discord-only / no system proxy / no WinDivert");
    tray::start();
    println!("Minimize-to-tray enabled: minimize the console to hide it from the taskbar");

    let engine = ensure_byedpi().await?;
    let mut byedpi = spawn_byedpi(&engine)?;
    wait_for_port(BYEDPI_PORT, Duration::from_secs(8)).await?;

    let state = ProxyState {
        http: Client::builder().timeout(Duration::from_secs(8)).build()?,
        dns: Arc::new(DnsCache::default()),
    };

    let socks_task = tokio::spawn(run_frontend(state.clone()));
    wait_for_port(FRONTEND_PORT, Duration::from_secs(3)).await?;

    let (installed_exe, process_name) = find_discord_exe()?;
    stop_existing_discord(&process_name);

    let voice_tcp = env::args().any(|a| a == "--voice-tcp");
    let mut launch = LaunchState::load();

    loop {
        let prepared = prepare_discord(&state, &installed_exe, &process_name, &launch).await?;

        println!("Auto-update complete; updater proxy stopped before Discord launch");
        println!("Strict isolation: Discord-only SOCKS remains");

        let outcome = run_discord(
            &state,
            &installed_exe,
            &process_name,
            &prepared,
            voice_tcp,
            &mut launch,
        )
        .await?;

        match outcome {
            DiscordOutcome::Exited => break,
            DiscordOutcome::RestartForUpdate => {
                println!("Restarting Discord to apply the update...");
            }
            DiscordOutcome::FailedToStart => {
                let fallback = launch
                    .last_good_host
                    .clone()
                    .filter(|good| *good != prepared.host && launch.last_good_manifest.is_some());

                let Some(fallback) = fallback else {
                    eprintln!(
                        "Discord {} failed to start and there is no earlier working version to roll back to",
                        prepared.host
                    );
                    break;
                };

                eprintln!(
                    "Discord {} failed to start; rolling back to {fallback}",
                    prepared.host
                );
                launch.bad_host = Some(prepared.host.clone());
                launch.save();
            }
        }
    }

    socks_task.abort();
    let _ = byedpi.kill();
    println!("DiscordOnlyDPI shutdown complete");

    Ok(())
}

/// Remembers the last Discord host that started correctly, so a Discord update
/// that breaks startup is rolled back instead of leaving Discord unusable.
#[derive(Default)]
struct LaunchState {
    last_good_host: Option<String>,
    last_good_manifest: Option<Value>,
    bad_host: Option<String>,
}

impl LaunchState {
    fn path() -> Option<PathBuf> {
        env::var_os("LOCALAPPDATA").map(|base| {
            PathBuf::from(base)
                .join("DiscordOnlyDPI")
                .join("state.json")
        })
    }

    fn load() -> Self {
        let Some(value) = Self::path()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        else {
            return Self::default();
        };

        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
        Self {
            last_good_host: text("last_good_host"),
            last_good_manifest: value
                .get("last_good_manifest")
                .filter(|manifest| manifest.is_object())
                .cloned(),
            bad_host: text("bad_host"),
        }
    }

    fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };

        let value = serde_json::json!({
            "last_good_host": self.last_good_host,
            "last_good_manifest": self.last_good_manifest,
            "bad_host": self.bad_host,
        });

        let result = serde_json::to_vec_pretty(&value)
            .map_err(|e| e.to_string())
            .and_then(|bytes| fs::write(&path, bytes).map_err(|e| e.to_string()));
        if let Err(error) = result {
            eprintln!("Failed to save {}: {error}", path.display());
        }
    }
}

struct PreparedDiscord {
    exe: PathBuf,
    host: String,
    manifest: Value,
    modules_root: PathBuf,
}

enum DiscordOutcome {
    Exited,
    RestartForUpdate,
    FailedToStart,
}

/// The HTTP CONNECT proxy is only needed while DiscordOnlyDPI updates Discord.
/// It is stopped again right after, so games and unrelated processes cannot use it.
struct UpdaterProxy(Option<JoinHandle<()>>);

impl UpdaterProxy {
    async fn start(state: &ProxyState) -> Result<Self, AnyError> {
        let proxy = Self(Some(tokio::spawn(run_http_connect_frontend(state.clone()))));
        if let Err(error) = wait_for_port(UPDATER_HTTP_PORT, Duration::from_secs(3)).await {
            proxy.stop().await;
            return Err(error);
        }
        Ok(proxy)
    }

    async fn stop(mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for UpdaterProxy {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

#[cfg(target_os = "windows")]
async fn prepare_discord(
    state: &ProxyState,
    installed_exe: &Path,
    process_name: &str,
    launch: &LaunchState,
) -> Result<PreparedDiscord, AnyError> {
    let proxy = UpdaterProxy::start(state).await?;
    let result = install_discord(installed_exe, process_name, launch).await;
    proxy.stop().await;

    let prepared = result?;
    prepare_discord_startup(&prepared.exe, process_name, &prepared.modules_root)?;
    Ok(prepared)
}

#[cfg(target_os = "windows")]
async fn install_discord(
    installed_exe: &Path,
    process_name: &str,
    launch: &LaunchState,
) -> Result<PreparedDiscord, AnyError> {
    let latest = fetch_discord_manifest(process_name).await?;
    let latest_host = manifest_host_version(&latest)?;

    let manifest = match (&launch.bad_host, &launch.last_good_manifest) {
        (Some(bad), Some(good)) if *bad == latest_host => {
            println!(
                "Discord host {latest_host} failed to start before; staying on the last working version until Discord releases a newer one"
            );
            good.clone()
        }
        _ => latest,
    };

    let exe = ensure_latest_discord_host(installed_exe, process_name, &manifest).await?;
    let modules_root = sync_discord_modules(&exe, process_name, &manifest).await?;
    let host = manifest_host_version(&manifest)?;

    Ok(PreparedDiscord {
        exe,
        host,
        manifest,
        modules_root,
    })
}

/// Startup markers read from Discord's console output.
#[derive(Default)]
struct StartupSignals {
    started: AtomicBool,
    failed: AtomicBool,
}

impl StartupSignals {
    fn observe(&self, line: &str) {
        // Discord's fatal handler prints "fatal: <error>" before showing its error dialog.
        if line.starts_with("fatal: ") || line.contains("Cannot find module 'discord_") {
            self.failed.store(true, Ordering::SeqCst);
        } else if is_discord_app_log(line) || line.starts_with("Startup timing:") {
            self.started.store(true, Ordering::SeqCst);
        }
    }
}

/// Lines like "22:54:54.181 > ..." come from Discord's web app, which only runs
/// once discord_desktop_core has loaded.
fn is_discord_app_log(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() > 15
        && bytes[..12].iter().enumerate().all(|(i, c)| match i {
            2 | 5 => *c == b':',
            8 => *c == b'.',
            _ => c.is_ascii_digit(),
        })
        && &bytes[12..15] == b" > "
}

#[cfg(target_os = "windows")]
async fn run_discord(
    state: &ProxyState,
    installed_exe: &Path,
    process_name: &str,
    prepared: &PreparedDiscord,
    voice_tcp: bool,
    launch: &mut LaunchState,
) -> Result<DiscordOutcome, AnyError> {
    println!("Launching: {}", prepared.exe.display());
    let signals = Arc::new(StartupSignals::default());
    let mut discord = launch_discord(&prepared.exe, voice_tcp, &signals)?;
    let launched_at = Instant::now();

    let mut exit = tokio::task::spawn_blocking(move || discord.wait());
    let mut exited = false;
    let mut updates: Option<JoinHandle<()>> = None;

    let outcome = loop {
        tokio::select! {
            result = &mut exit => {
                exited = true;
                match result {
                    Ok(Ok(status)) => println!("Discord exited with status: {status}"),
                    Ok(Err(error)) => eprintln!("Failed while waiting for Discord: {error}"),
                    Err(error) => eprintln!("Discord wait task failed: {error}"),
                }

                let started = signals.started.load(Ordering::SeqCst);
                let failed = signals.failed.load(Ordering::SeqCst);
                if !started && (failed || launched_at.elapsed() < STARTUP_GRACE) {
                    break DiscordOutcome::FailedToStart;
                }
                break DiscordOutcome::Exited;
            }
            _ = sleep(Duration::from_millis(500)) => {
                let failed = signals.failed.load(Ordering::SeqCst);
                let started = signals.started.load(Ordering::SeqCst)
                    || (!failed && launched_at.elapsed() >= STARTUP_ASSUME_OK);

                if failed && !started {
                    break DiscordOutcome::FailedToStart;
                }

                if started && updates.is_none() {
                    println!("Discord {} started successfully", prepared.host);
                    launch.last_good_host = Some(prepared.host.clone());
                    launch.last_good_manifest = Some(prepared.manifest.clone());
                    launch.save();

                    updates = Some(tokio::spawn(watch_for_updates(
                        state.clone(),
                        installed_exe.to_path_buf(),
                        process_name.to_string(),
                        prepared.host.clone(),
                        launch.bad_host.clone(),
                    )));
                }

                if tray::take_restart_request() {
                    break DiscordOutcome::RestartForUpdate;
                }
            }
        }
    };

    if let Some(task) = updates {
        task.abort();
        let _ = task.await;
    }

    if !exited {
        stop_existing_discord(process_name);
        let _ = exit.await;
    }

    Ok(outcome)
}

/// Checks for Discord updates while Discord is running and prepares them in the
/// background, then offers a restart from the tray.
#[cfg(target_os = "windows")]
async fn watch_for_updates(
    state: ProxyState,
    installed_exe: PathBuf,
    process_name: String,
    running_host: String,
    bad_host: Option<String>,
) {
    loop {
        sleep(UPDATE_CHECK_INTERVAL).await;

        let proxy = match UpdaterProxy::start(&state).await {
            Ok(proxy) => proxy,
            Err(error) => {
                eprintln!("Background Discord update check failed: {error}");
                continue;
            }
        };
        let result = stage_discord_update(
            &installed_exe,
            &process_name,
            &running_host,
            bad_host.as_deref(),
        )
        .await;
        proxy.stop().await;

        match result {
            Ok(Some(update)) => {
                println!(
                    "Discord update ready ({update}). Click the DiscordOnlyDPI notification or right-click the tray icon to restart Discord now; otherwise it is applied the next time DiscordOnlyDPI starts."
                );
                tray::announce_update(&update);
                return;
            }
            Ok(None) => println!("Discord is up to date"),
            Err(error) => eprintln!("Background Discord update check failed: {error}"),
        }
    }
}

#[cfg(target_os = "windows")]
async fn stage_discord_update(
    installed_exe: &Path,
    process_name: &str,
    running_host: &str,
    bad_host: Option<&str>,
) -> Result<Option<String>, AnyError> {
    let manifest = fetch_discord_manifest(process_name).await?;
    let latest = manifest_host_version(&manifest)?;

    if bad_host == Some(latest.as_str()) {
        return Ok(None);
    }

    if latest != running_host {
        // A new host gets its own app-<version> folder and its own modules folder,
        // so it can be installed while the current Discord keeps running.
        let exe = ensure_latest_discord_host(installed_exe, process_name, &manifest).await?;
        sync_discord_modules(&exe, process_name, &manifest).await?;
        return Ok(Some(format!("Discord {latest}")));
    }

    // The running Discord has its modules loaded, so they are replaced on restart.
    if modules_outdated(process_name, running_host, &manifest)? {
        return Ok(Some(format!("Discord {latest} modules")));
    }

    Ok(None)
}

#[cfg(target_os = "windows")]
fn modules_outdated(
    process_name: &str,
    host_version: &str,
    manifest: &Value,
) -> Result<bool, AnyError> {
    let installed = fs::read_to_string(
        discord_modules_root(process_name, host_version)?.join("installed.json"),
    )
    .ok()
    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    .unwrap_or(Value::Null);

    let modules = manifest
        .get("modules")
        .and_then(Value::as_object)
        .ok_or("Discord manifest is missing modules")?;

    Ok(modules.iter().any(|(name, module)| {
        let wanted = module
            .get("full")
            .and_then(|full| full.get("module_version"))
            .and_then(Value::as_u64);
        let installed_version = installed
            .get(name)
            .and_then(|v| v.get("installedVersion"))
            .and_then(Value::as_u64);
        wanted.is_some() && wanted != installed_version
    }))
}

async fn ensure_byedpi() -> Result<PathBuf, AnyError> {
    let base = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or("LOCALAPPDATA is not set")?
        .join("DiscordOnlyDPI")
        .join("bin");

    fs::create_dir_all(&base)?;
    let exe = base.join("ciadpi.exe");

    if exe.exists() && sha256_file(&exe)? == BYEDPI_SHA256 {
        return Ok(exe);
    }

    println!("Downloading official ByeDPI v0.17.3...");
    let bytes = Client::new()
        .get(BYEDPI_URL)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut found = false;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.name().ends_with("ciadpi.exe") {
            let mut out = File::create(&exe)?;
            std::io::copy(&mut entry, &mut out)?;
            out.flush()?;
            found = true;
            break;
        }
    }

    if !found {
        return Err("ciadpi.exe was not found in the official ByeDPI archive".into());
    }

    let actual = sha256_file(&exe)?;
    if actual != BYEDPI_SHA256 {
        let _ = fs::remove_file(&exe);
        return Err(format!("ByeDPI hash mismatch. Expected {BYEDPI_SHA256}, got {actual}").into());
    }

    Ok(exe)
}

fn sha256_file(path: &Path) -> Result<String, AnyError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(hex::encode(hasher.finalize()))
}

#[cfg(target_os = "windows")]
fn spawn_byedpi(exe: &Path) -> Result<Child, AnyError> {
    let log_stdout = last_log::open_append_file()?;
    let log_stderr = log_stdout.try_clone()?;

    let mut cmd = Command::new(exe);
    cmd.arg("-i")
        .arg("127.0.0.1")
        .arg("-p")
        .arg(BYEDPI_PORT.to_string())
        .args(BYEDPI_ARGS)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_stdout))
        .stderr(Stdio::from(log_stderr));

    hide_window(&mut cmd);
    Ok(cmd.spawn()?)
}

async fn wait_for_port(port: u16, timeout: Duration) -> Result<(), AnyError> {
    let start = Instant::now();

    while start.elapsed() < timeout {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }

    Err(format!("Timed out waiting for local port {port}").into())
}

async fn run_frontend(state: ProxyState) {
    let listener = match TcpListener::bind(("127.0.0.1", FRONTEND_PORT)).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Failed to bind Discord-only proxy: {e}");
            return;
        }
    };

    println!("Discord-only SOCKS5 ready on 127.0.0.1:{FRONTEND_PORT}");

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, state).await {
                        if e.kind() != io::ErrorKind::UnexpectedEof {
                            eprintln!("Proxy connection error: {e}");
                        }
                    }
                });
            }
            Err(e) => {
                eprintln!("Accept error: {e}");
                sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn run_http_connect_frontend(state: ProxyState) {
    let listener = match TcpListener::bind(("127.0.0.1", UPDATER_HTTP_PORT)).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Failed to bind updater HTTP proxy: {e}");
            return;
        }
    };

    println!("Discord updater HTTP proxy ready on 127.0.0.1:{UPDATER_HTTP_PORT}");

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_http_connect(stream, state).await {
                        if e.kind() != io::ErrorKind::UnexpectedEof {
                            eprintln!("Updater proxy connection error: {e}");
                        }
                    }
                });
            }
            Err(e) => {
                eprintln!("Updater proxy accept error: {e}");
                sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle_http_connect(mut client: TcpStream, state: ProxyState) -> io::Result<()> {
    let mut request = Vec::with_capacity(1024);

    while request.len() < 16 * 1024 {
        let byte = client.read_u8().await?;
        request.push(byte);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    if !request.ends_with(b"\r\n\r\n") {
        client
            .write_all(b"HTTP/1.1 431 Request Header Fields Too Large\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }

    let request_text = std::str::from_utf8(&request)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid HTTP proxy request"))?;
    let first_line = request_text
        .lines()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty HTTP proxy request"))?;

    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let authority = parts.next().unwrap_or_default();

    if method != "CONNECT" {
        client
            .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }

    let (host, port) = parse_connect_authority(authority)?;
    let target_ip = match host.parse::<IpAddr>() {
        Ok(ip) => ip,
        Err(_) => IpAddr::V4(
            resolve_v4(&state, &host)
                .await
                .map_err(|e| io::Error::other(e.to_string()))?,
        ),
    };

    let mut upstream = match connect_via_byedpi(target_ip, port).await {
        Ok(stream) => stream,
        Err(e) => {
            let _ = client
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n")
                .await;
            return Err(e);
        }
    };

    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\nProxy-Agent: DiscordOnlyDPI\r\n\r\n")
        .await?;

    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

fn parse_connect_authority(authority: &str) -> io::Result<(String, u16)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid IPv6 CONNECT host")
        })?;
        let host = &rest[..end];
        let after = &rest[end + 1..];
        let port = after
            .strip_prefix(':')
            .unwrap_or("443")
            .parse::<u16>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid CONNECT port"))?;
        return Ok((host.to_string(), port));
    }

    if let Some((host, port)) = authority.rsplit_once(':') {
        let port = port
            .parse::<u16>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid CONNECT port"))?;
        if host.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty CONNECT host",
            ));
        }
        return Ok((host.to_string(), port));
    }

    if authority.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty CONNECT authority",
        ));
    }

    Ok((authority.to_string(), 443))
}

async fn handle_client(mut client: TcpStream, state: ProxyState) -> io::Result<()> {
    let mut hello = [0u8; 2];
    client.read_exact(&mut hello).await?;
    if hello[0] != 5 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not SOCKS5"));
    }

    let mut methods = vec![0u8; hello[1] as usize];
    client.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        client.write_all(&[5, 0xff]).await?;
        return Ok(());
    }
    client.write_all(&[5, 0]).await?;

    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;
    if req[0] != 5 || req[1] != 1 {
        send_socks_error(&mut client, 7).await?;
        return Ok(());
    }

    let target_ip = match req[3] {
        1 => {
            let mut raw = [0u8; 4];
            client.read_exact(&mut raw).await?;
            IpAddr::V4(Ipv4Addr::from(raw))
        }
        3 => {
            let len = client.read_u8().await? as usize;
            let mut raw = vec![0u8; len];
            client.read_exact(&mut raw).await?;
            let host = String::from_utf8(raw)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid hostname"))?;
            IpAddr::V4(
                resolve_v4(&state, &host)
                    .await
                    .map_err(|e| io::Error::other(e.to_string()))?,
            )
        }
        4 => {
            let mut raw = [0u8; 16];
            client.read_exact(&mut raw).await?;
            IpAddr::V6(Ipv6Addr::from(raw))
        }
        _ => {
            send_socks_error(&mut client, 8).await?;
            return Ok(());
        }
    };

    let port = client.read_u16().await?;
    let mut upstream = match connect_via_byedpi(target_ip, port).await {
        Ok(v) => v,
        Err(e) => {
            let _ = send_socks_error(&mut client, 5).await;
            return Err(e);
        }
    };

    client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;

    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

async fn send_socks_error(stream: &mut TcpStream, code: u8) -> io::Result<()> {
    stream.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await
}

async fn resolve_v4(state: &ProxyState, host: &str) -> Result<Ipv4Addr, AnyError> {
    {
        let cache = state
            .dns
            .entries
            .lock()
            .map_err(|_| "DNS cache lock poisoned")?;

        if let Some((when, ip)) = cache.get(host) {
            if when.elapsed() < Duration::from_secs(120) {
                return Ok(*ip);
            }
        }
    }

    let response = state
        .http
        .get("https://1.1.1.1/dns-query")
        .header(header::ACCEPT, "application/dns-json")
        .query(&[("name", host), ("type", "A")])
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;

    let answers = response
        .get("Answer")
        .and_then(Value::as_array)
        .ok_or("DoH response has no Answer section")?;

    let ip = answers
        .iter()
        .find_map(|answer| {
            if answer.get("type").and_then(Value::as_u64) == Some(1) {
                answer
                    .get("data")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<Ipv4Addr>().ok())
            } else {
                None
            }
        })
        .ok_or("DoH returned no IPv4 address")?;

    state
        .dns
        .entries
        .lock()
        .map_err(|_| "DNS cache lock poisoned")?
        .insert(host.to_string(), (Instant::now(), ip));

    Ok(ip)
}

async fn connect_via_byedpi(ip: IpAddr, port: u16) -> io::Result<TcpStream> {
    let mut stream = TcpStream::connect(("127.0.0.1", BYEDPI_PORT)).await?;

    stream.write_all(&[5, 1, 0]).await?;
    let mut hello = [0u8; 2];
    stream.read_exact(&mut hello).await?;
    if hello != [5, 0] {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "ByeDPI rejected SOCKS5 negotiation",
        ));
    }

    let mut request = Vec::with_capacity(22);
    request.extend_from_slice(&[5, 1, 0]);

    match ip {
        IpAddr::V4(v4) => {
            request.push(1);
            request.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            request.push(4);
            request.extend_from_slice(&v6.octets());
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await?;

    read_socks_reply(&mut stream).await?;
    Ok(stream)
}

async fn read_socks_reply(stream: &mut TcpStream) -> io::Result<()> {
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;

    if head[0] != 5 || head[1] != 0 {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("ByeDPI SOCKS5 connect failed with code {}", head[1]),
        ));
    }

    match head[3] {
        1 => {
            let mut rest = [0u8; 6];
            stream.read_exact(&mut rest).await?;
        }
        3 => {
            let len = stream.read_u8().await? as usize;
            let mut rest = vec![0u8; len + 2];
            stream.read_exact(&mut rest).await?;
        }
        4 => {
            let mut rest = [0u8; 18];
            stream.read_exact(&mut rest).await?;
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid SOCKS5 reply address type",
            ));
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn find_discord_exe() -> Result<(PathBuf, String), AnyError> {
    let local = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or("LOCALAPPDATA is not set")?;

    let variants = [
        ("Discord", "Discord.exe"),
        ("DiscordPTB", "DiscordPTB.exe"),
        ("DiscordCanary", "DiscordCanary.exe"),
    ];

    for (folder, exe_name) in variants {
        let root = local.join(folder);
        if !root.is_dir() {
            continue;
        }

        let mut matches = Vec::new();
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("app-") {
                continue;
            }

            let exe = entry.path().join(exe_name);
            if exe.is_file() {
                matches.push(exe);
            }
        }

        matches.sort();
        if let Some(exe) = matches.pop() {
            return Ok((exe, exe_name.to_string()));
        }
    }

    Err("Discord installation was not found under LOCALAPPDATA".into())
}

fn discord_channel(process_name: &str) -> &'static str {
    match process_name.to_ascii_lowercase().as_str() {
        "discordptb.exe" => "ptb",
        "discordcanary.exe" => "canary",
        _ => "stable",
    }
}

fn discord_config_dir(process_name: &str) -> &'static str {
    match process_name.to_ascii_lowercase().as_str() {
        "discordptb.exe" => "discordptb",
        "discordcanary.exe" => "discordcanary",
        _ => "discord",
    }
}

fn discord_modules_root(process_name: &str, host_version: &str) -> Result<PathBuf, AnyError> {
    let appdata = env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not set")?;
    Ok(appdata
        .join(discord_config_dir(process_name))
        .join(host_version)
        .join("modules"))
}

fn manifest_host_version(manifest: &Value) -> Result<String, AnyError> {
    json_version(
        manifest
            .get("full")
            .and_then(|v| v.get("host_version"))
            .ok_or("Discord manifest is missing full.host_version")?,
    )
}

fn discord_update_client() -> Result<Client, AnyError> {
    let proxy_url = format!("http://127.0.0.1:{UPDATER_HTTP_PORT}");
    Ok(Client::builder()
        .proxy(Proxy::all(&proxy_url)?)
        .timeout(Duration::from_secs(180))
        .build()?)
}

async fn fetch_discord_manifest(process_name: &str) -> Result<Value, AnyError> {
    let channel = discord_channel(process_name);
    let client = discord_update_client()?;

    println!("Checking latest Discord {channel} version through ByeDPI...");

    let mut last_error = None;
    for attempt in 1..=3 {
        match client
            .get("https://updates.discord.com/distributions/app/manifests/latest")
            .header(header::USER_AGENT, "Discord-Updater/1")
            .query(&[("channel", channel), ("platform", "win"), ("arch", "x64")])
            .send()
            .await
        {
            Ok(response) => match response.error_for_status() {
                Ok(response) => return Ok(response.json::<Value>().await?),
                Err(e) => last_error = Some(e.to_string()),
            },
            Err(e) => last_error = Some(e.to_string()),
        }

        eprintln!("Manifest check attempt {attempt}/3 failed");
        sleep(Duration::from_millis(500 * attempt)).await;
    }

    Err(format!(
        "Unable to fetch Discord update manifest: {}",
        last_error.unwrap_or_else(|| "unknown error".to_string())
    )
    .into())
}

#[cfg(target_os = "windows")]
async fn ensure_latest_discord_host(
    installed_exe: &Path,
    process_name: &str,
    manifest: &Value,
) -> Result<PathBuf, AnyError> {
    let current_dir = installed_exe
        .parent()
        .ok_or("Discord executable has no parent directory")?;
    let install_root = current_dir
        .parent()
        .ok_or("Discord app directory has no install root")?;

    let current_folder = current_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Unable to determine installed Discord version")?;
    let current_version = current_folder
        .strip_prefix("app-")
        .ok_or("Unexpected Discord app folder name")?;

    let latest_version = manifest_host_version(manifest)?;

    let target_dir = install_root.join(format!("app-{latest_version}"));
    let target_exe = target_dir.join(process_name);
    let target_asar = target_dir.join("resources").join("app.asar");
    let target_build_info = target_dir.join("resources").join("build_info.json");
    let target_complete =
        target_exe.is_file() && target_asar.is_file() && target_build_info.is_file();

    if latest_version == current_version && target_complete {
        println!("Discord host {latest_version}: already current");
        return Ok(target_exe);
    }

    if target_complete {
        println!("Discord host {latest_version}: already installed, switching to it");
        return Ok(target_exe);
    }

    let full = manifest
        .get("full")
        .ok_or("Discord manifest is missing full package")?;
    let url = full
        .get("url")
        .and_then(Value::as_str)
        .ok_or("Discord full package has no URL")?;
    let expected_hash = full
        .get("package_sha256")
        .and_then(Value::as_str)
        .ok_or("Discord full package has no package_sha256")?;

    println!("Discord host update: {current_version} -> {latest_version}");
    println!("Downloading official Discord host package...");

    let client = discord_update_client()?;
    let package = client
        .get(url)
        .header(header::USER_AGENT, "Discord-Updater/1")
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    let actual_hash = sha256_bytes(&package);
    if !actual_hash.eq_ignore_ascii_case(expected_hash) {
        return Err(format!(
            "Discord host SHA-256 mismatch: expected {expected_hash}, got {actual_hash}"
        )
        .into());
    }

    let temp_dir = install_root.join(format!(
        "app-{latest_version}.discordonlydpi-tmp-{}",
        std::process::id()
    ));

    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;

    if let Err(e) = extract_full_distro(&package, &temp_dir) {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(e);
    }

    let temp_exe = temp_dir.join(process_name);
    let temp_asar = temp_dir.join("resources").join("app.asar");
    let temp_build_info = temp_dir.join("resources").join("build_info.json");

    if !temp_exe.is_file() || !temp_asar.is_file() || !temp_build_info.is_file() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err("Downloaded Discord host package is incomplete".into());
    }

    if target_dir.exists() {
        fs::remove_dir_all(&target_dir)?;
    }
    fs::rename(&temp_dir, &target_dir)?;

    println!("Discord host {latest_version}: installed successfully");
    Ok(target_exe)
}

#[cfg(target_os = "windows")]
async fn sync_discord_modules(
    exe: &Path,
    process_name: &str,
    manifest: &Value,
) -> Result<PathBuf, AnyError> {
    let app_folder = exe
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or("Unable to determine Discord app version")?;

    let host_version = app_folder
        .strip_prefix("app-")
        .ok_or("Unexpected Discord app folder name")?
        .to_string();

    let client = discord_update_client()?;

    let manifest_host = manifest_host_version(manifest)?;

    if manifest_host != host_version {
        return Err(format!(
            "Internal update error: host {host_version} does not match manifest {manifest_host}"
        )
        .into());
    }

    let required = manifest
        .get("required_modules")
        .and_then(Value::as_array)
        .ok_or("Discord manifest is missing required_modules")?;

    let modules_root = discord_modules_root(process_name, &host_version)?;
    fs::create_dir_all(&modules_root)?;

    let installed_path = modules_root.join("installed.json");
    let mut installed = if installed_path.is_file() {
        match fs::read_to_string(&installed_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        {
            Some(Value::Object(map)) => Value::Object(map),
            _ => Value::Object(serde_json::Map::new()),
        }
    } else {
        Value::Object(serde_json::Map::new())
    };

    let mut desired = required
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();

    let modules = manifest
        .get("modules")
        .and_then(Value::as_object)
        .ok_or("Discord manifest is missing modules")?;

    let mut optional = modules
        .keys()
        .filter(|name| !desired.iter().any(|required| required == *name))
        .cloned()
        .collect::<Vec<_>>();
    optional.sort();
    desired.extend(optional);

    println!(
        "Discord modules to sync ({}): {}",
        desired.len(),
        desired.join(", ")
    );

    for module_name in desired {
        let full = manifest
            .get("modules")
            .and_then(|m| m.get(&module_name))
            .and_then(|m| m.get("full"))
            .ok_or_else(|| format!("Manifest has no full package for {module_name}"))?;

        let module_host = json_version(
            full.get("host_version")
                .ok_or_else(|| format!("{module_name} has no host_version"))?,
        )?;

        if module_host != host_version {
            return Err(format!(
                "Module {module_name} targets host {module_host}, expected {host_version}"
            )
            .into());
        }

        let module_version = full
            .get("module_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("{module_name} has no module_version"))?;

        let url = full
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{module_name} has no download URL"))?;

        let expected_hash = full
            .get("package_sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{module_name} has no package_sha256"))?;

        let target_dir = modules_root.join(&module_name);
        let marker = if module_name == "discord_desktop_core" {
            target_dir.join("core.asar")
        } else {
            target_dir.join("package.json")
        };

        let installed_version = installed
            .get(&module_name)
            .and_then(|v| v.get("installedVersion"))
            .and_then(Value::as_u64);

        if installed_version == Some(module_version) && marker.is_file() {
            println!("{module_name}@{module_version}: already installed");
            continue;
        }

        println!("{module_name}@{module_version}: downloading...");

        let package = client
            .get(url)
            .header(header::USER_AGENT, "Discord-Updater/1")
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        let actual_hash = sha256_bytes(&package);
        if !actual_hash.eq_ignore_ascii_case(expected_hash) {
            return Err(format!(
                "SHA-256 mismatch for {module_name}: expected {expected_hash}, got {actual_hash}"
            )
            .into());
        }

        let temp_dir = modules_root.join(format!(
            ".discordonlydpi-{}-{}",
            module_name,
            std::process::id()
        ));

        if temp_dir.exists() {
            fs::remove_dir_all(&temp_dir)?;
        }
        fs::create_dir_all(&temp_dir)?;

        if let Err(e) = extract_full_distro(&package, &temp_dir) {
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(e);
        }

        let temp_marker = if module_name == "discord_desktop_core" {
            temp_dir.join("core.asar")
        } else {
            temp_dir.join("package.json")
        };

        if !temp_marker.is_file() {
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(format!(
                "Downloaded {module_name} package did not contain the expected files"
            )
            .into());
        }

        if target_dir.exists() {
            fs::remove_dir_all(&target_dir)?;
        }
        fs::rename(&temp_dir, &target_dir)?;

        installed
            .as_object_mut()
            .ok_or("installed.json root is not an object")?
            .insert(
                module_name.clone(),
                serde_json::json!({ "installedVersion": module_version }),
            );

        fs::write(&installed_path, serde_json::to_vec_pretty(&installed)?)?;
        println!("{module_name}@{module_version}: installed");
    }

    println!("Discord required modules are ready.");
    Ok(modules_root)
}

fn json_version(value: &Value) -> Result<String, AnyError> {
    let parts = value
        .as_array()
        .ok_or("Version value is not an array")?
        .iter()
        .map(|v| {
            v.as_u64()
                .map(|n| n.to_string())
                .ok_or("Version component is not an integer")
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(parts.join("."))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn extract_full_distro(bytes: &[u8], destination: &Path) -> Result<(), AnyError> {
    let reader = Cursor::new(bytes);
    let decompressor = brotli::Decompressor::new(reader, 64 * 1024);
    let mut archive = tar::Archive::new(decompressor);
    let mut extracted = 0usize;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let archive_path = entry.path()?.into_owned();

        let relative = match archive_path.strip_prefix("files") {
            Ok(path) if !path.as_os_str().is_empty() => path.to_path_buf(),
            _ => continue,
        };

        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(
                format!("Unsafe path in Discord distro: {}", archive_path.display()).into(),
            );
        }

        let output = destination.join(&relative);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }

        entry.unpack(&output)?;
        extracted += 1;
    }

    if extracted == 0 {
        return Err("Discord distro contained no files/ payload".into());
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn prepare_discord_startup(
    exe: &Path,
    process_name: &str,
    modules_root: &Path,
) -> Result<(), AnyError> {
    let appdata = env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not set")?;
    let settings = appdata
        .join(discord_config_dir(process_name))
        .join("settings.json");

    patch_json_flags(
        &settings,
        &[
            ("SKIP_HOST_UPDATE", Value::Bool(true)),
            ("SKIP_MODULE_UPDATE", Value::Bool(true)),
        ],
    )?;

    let resources = exe
        .parent()
        .ok_or("Discord executable has no parent")?
        .join("resources");
    let build_info = resources.join("build_info.json");

    // Newer hosts dropped the legacy module updater. With the updater disabled,
    // Discord only loads modules from build_info's localModulesRoot.
    let modules_root = modules_root
        .to_str()
        .ok_or("Discord modules path is not valid UTF-8")?;

    if build_info.is_file() {
        patch_json_flags(
            &build_info,
            &[
                ("newUpdater", Value::Bool(false)),
                ("disableUpdater", Value::Bool(true)),
                ("localModulesRoot", Value::String(modules_root.to_string())),
            ],
        )?;
    }

    println!("Discord internal updaters disabled; modules are managed by DiscordOnlyDPI");
    Ok(())
}

#[cfg(target_os = "windows")]
fn patch_json_flags(path: &Path, flags: &[(&str, Value)]) -> Result<(), AnyError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut root = if path.is_file() {
        let raw = fs::read_to_string(path)?;
        serde_json::from_str::<Value>(&raw)?
    } else {
        Value::Object(serde_json::Map::new())
    };

    let object = root
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", path.display()))?;

    for (key, value) in flags {
        object.insert((*key).to_string(), value.clone());
    }

    if path.is_file() {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("Invalid JSON file name")?;
        let backup = path.with_file_name(format!("{file_name}.discordonlydpi.bak"));
        if !backup.exists() {
            fs::copy(path, backup)?;
        }
    }

    let encoded = serde_json::to_vec_pretty(&root)?;
    fs::write(path, encoded)?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn stop_existing_discord(process_name: &str) {
    let mut cmd = Command::new("taskkill");
    cmd.args(["/IM", process_name, "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    let _ = cmd.status();
}

#[cfg(target_os = "windows")]
fn launch_discord(
    exe: &Path,
    voice_tcp: bool,
    signals: &Arc<StartupSignals>,
) -> Result<Child, AnyError> {
    let parent = exe.parent().ok_or("Discord executable has no parent")?;
    let proxy = format!("socks5://127.0.0.1:{FRONTEND_PORT}");

    let mut cmd = Command::new(exe);
    cmd.current_dir(parent)
        .arg(format!("--proxy-server={proxy}"))
        .arg("--disable-quic")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Prevent Discord and its child processes from inheriting any ambient
    // proxy environment. Only Chromium's explicit --proxy-server is used.
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        cmd.env_remove(key);
    }

    cmd.env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost");

    println!("Game-safe mode: no system proxy, no proxy environment, loopback only");

    if voice_tcp {
        cmd.arg("--force-webrtc-ip-handling-policy")
            .arg("--webrtc-ip-handling-policy=disable_non_proxied_udp");
        println!("Voice test mode: forcing WebRTC away from non-proxied UDP");
    }

    let mut child = cmd.spawn()?;
    if let Some(stdout) = child.stdout.take() {
        forward_discord_output(stdout, signals.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        forward_discord_output(stderr, signals.clone());
    }
    Ok(child)
}

/// Copies Discord's output into last-run.log and watches it for startup markers.
fn forward_discord_output(output: impl Read + Send + 'static, signals: Arc<StartupSignals>) {
    use std::io::BufRead;

    thread::spawn(move || {
        let mut reader = std::io::BufReader::new(output);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }

            let text = String::from_utf8_lossy(&line);
            let text = text.trim_end_matches(['\r', '\n']);
            signals.observe(text);
            last_log::write_file_line(text);
        }
    });
}

#[cfg(target_os = "windows")]
fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(target_os = "windows")]
mod tray {
    use std::{
        mem::{size_of, zeroed},
        ptr::{null, null_mut},
        sync::atomic::{AtomicBool, AtomicIsize, Ordering},
        thread,
        time::Duration,
    };

    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{Console::GetConsoleWindow, LibraryLoader::GetModuleHandleW},
        UI::{
            Shell::{
                Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD,
                NIM_DELETE, NIM_MODIFY, NIN_BALLOONUSERCLICK, NOTIFYICONDATAW,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, IsIconic,
                LoadIconW, PostQuitMessage, RegisterClassW, SendMessageW, ShowWindow,
                TranslateMessage, HICON, ICON_BIG, ICON_SMALL, MSG, SW_HIDE, SW_RESTORE, WM_APP,
                WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP, WM_SETICON, WNDCLASSW,
            },
        },
    };

    const WM_TRAY_ICON: u32 = WM_APP + 1;
    const TRAY_ICON_ID: u32 = 1;
    const DEFAULT_TIP: &str = "DiscordOnlyDPI - click to restore";
    static CONSOLE_HWND: AtomicIsize = AtomicIsize::new(0);
    static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
    static UPDATE_READY: AtomicBool = AtomicBool::new(false);
    static RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);

    /// Shows a notification that a Discord update is ready. Clicking it, or
    /// right-clicking the tray icon, requests a Discord restart.
    pub fn announce_update(update: &str) {
        UPDATE_READY.store(true, Ordering::SeqCst);
        unsafe {
            let Some(mut icon) = tray_icon() else {
                return;
            };
            icon.uFlags = NIF_INFO | NIF_TIP;
            icon.dwInfoFlags = NIIF_INFO;
            copy_wide(&mut icon.szInfoTitle, "Discord update ready");
            copy_wide(
                &mut icon.szInfo,
                &format!("{update} is ready. Click here to restart Discord now."),
            );
            copy_wide(
                &mut icon.szTip,
                "DiscordOnlyDPI - update ready, right-click to restart Discord",
            );
            Shell_NotifyIconW(NIM_MODIFY, &icon);
        }
    }

    /// Returns true once after the user asked to restart Discord for an update.
    pub fn take_restart_request() -> bool {
        if !RESTART_REQUESTED.swap(false, Ordering::SeqCst) {
            return false;
        }

        UPDATE_READY.store(false, Ordering::SeqCst);
        unsafe {
            if let Some(mut icon) = tray_icon() {
                icon.uFlags = NIF_TIP;
                copy_wide(&mut icon.szTip, DEFAULT_TIP);
                Shell_NotifyIconW(NIM_MODIFY, &icon);
            }
        }
        true
    }

    unsafe fn tray_icon() -> Option<NOTIFYICONDATAW> {
        let raw = TRAY_HWND.load(Ordering::SeqCst);
        if raw == 0 {
            return None;
        }

        let mut icon: NOTIFYICONDATAW = zeroed();
        icon.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        icon.hWnd = raw as HWND;
        icon.uID = TRAY_ICON_ID;
        Some(icon)
    }

    pub fn start() {
        let console = unsafe { GetConsoleWindow() };
        if console.is_null() {
            return;
        }

        CONSOLE_HWND.store(console as isize, Ordering::SeqCst);

        unsafe {
            let app_icon = load_app_icon();
            if !app_icon.is_null() {
                SendMessageW(console, WM_SETICON, ICON_BIG as usize, app_icon as isize);
                SendMessageW(console, WM_SETICON, ICON_SMALL as usize, app_icon as isize);
            }
        }

        thread::spawn(|| unsafe {
            tray_message_loop();
        });

        thread::spawn(|| loop {
            let raw = CONSOLE_HWND.load(Ordering::SeqCst);
            if raw == 0 {
                break;
            }

            let hwnd = raw as HWND;
            unsafe {
                if IsIconic(hwnd) != 0 {
                    ShowWindow(hwnd, SW_HIDE);
                }
            }

            thread::sleep(Duration::from_millis(120));
        });
    }

    unsafe fn load_app_icon() -> HICON {
        let instance = GetModuleHandleW(null());
        // MAKEINTRESOURCE(1): winres embeds the app icon as resource ID 1.
        LoadIconW(instance, std::ptr::without_provenance(1))
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_TRAY_ICON {
            let event = lparam as u32;
            if event == WM_LBUTTONUP {
                let raw = CONSOLE_HWND.load(Ordering::SeqCst);
                if raw != 0 {
                    ShowWindow(raw as HWND, SW_RESTORE);
                }
            } else if (event == NIN_BALLOONUSERCLICK || event == WM_RBUTTONUP)
                && UPDATE_READY.load(Ordering::SeqCst)
            {
                RESTART_REQUESTED.store(true, Ordering::SeqCst);
            }
            return 0;
        }

        if msg == WM_DESTROY {
            PostQuitMessage(0);
            return 0;
        }

        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    unsafe fn tray_message_loop() {
        let instance = GetModuleHandleW(null());
        let class_name = wide("DiscordOnlyDPITrayWindow");

        let mut class: WNDCLASSW = zeroed();
        class.lpfnWndProc = Some(window_proc);
        class.hInstance = instance;
        class.hIcon = load_app_icon();
        class.lpszClassName = class_name.as_ptr();

        if RegisterClassW(&class) == 0 {
            return;
        }

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null_mut(),
        );

        if hwnd.is_null() {
            return;
        }

        let mut icon: NOTIFYICONDATAW = zeroed();
        icon.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        icon.hWnd = hwnd;
        icon.uID = TRAY_ICON_ID;
        icon.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        icon.uCallbackMessage = WM_TRAY_ICON;
        icon.hIcon = load_app_icon();
        copy_wide(&mut icon.szTip, DEFAULT_TIP);

        if Shell_NotifyIconW(NIM_ADD, &icon) == 0 {
            return;
        }
        TRAY_HWND.store(hwnd as isize, Ordering::SeqCst);

        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        Shell_NotifyIconW(NIM_DELETE, &icon);
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Copies `value` into a fixed-size, null-terminated UTF-16 buffer.
    fn copy_wide(target: &mut [u16], value: &str) {
        let capacity = target.len().saturating_sub(1);
        let mut len = 0;
        for (slot, unit) in target.iter_mut().take(capacity).zip(value.encode_utf16()) {
            *slot = unit;
            len += 1;
        }
        target[len] = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_startup_from_discord_output() {
        let signals = StartupSignals::default();
        signals.observe("Starting app.");
        signals.observe("splashScreen.launchMainWindow: false");
        assert!(!signals.started.load(Ordering::SeqCst));

        signals.observe("22:54:54.181 > [FAST CONNECT] wss://gateway.discord.gg/?encoding=etf&v=9");
        assert!(signals.started.load(Ordering::SeqCst));
        assert!(!signals.failed.load(Ordering::SeqCst));
    }

    #[test]
    fn detects_missing_core_module() {
        let signals = StartupSignals::default();
        signals.observe("fatal: Error: Cannot find module 'discord_desktop_core'");
        assert!(signals.failed.load(Ordering::SeqCst));
        assert!(!signals.started.load(Ordering::SeqCst));
    }

    #[test]
    fn ignores_lines_that_only_look_like_timestamps() {
        assert!(!is_discord_app_log("22:54:54 > short"));
        assert!(!is_discord_app_log(
            "blackbox: 11/10/2026, 10:54:53 PM GMT+3 1 Discord starting"
        ));
        assert!(is_discord_app_log(
            "22:55:08.604 > [libdiscore] [BlockedDomainsStore] ok"
        ));
    }
}
