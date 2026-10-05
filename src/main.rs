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
        } else {
            std::println!("{message}");
        }

        if let Some(slot) = LOG_FILE.get() {
            if let Ok(mut guard) = slot.lock() {
                if let Some(file) = guard.as_mut() {
                    if is_error {
                        let _ = writeln!(file, "[ERROR] {message}");
                    } else {
                        let _ = writeln!(file, "{message}");
                    }
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
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
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
    let updater_proxy_task = tokio::spawn(run_http_connect_frontend(state));
    wait_for_port(FRONTEND_PORT, Duration::from_secs(3)).await?;
    wait_for_port(UPDATER_HTTP_PORT, Duration::from_secs(3)).await?;

    let (installed_exe, process_name) = find_discord_exe()?;
    stop_existing_discord(&process_name);

    let manifest = fetch_discord_manifest(&process_name).await?;
    let discord_exe = ensure_latest_discord_host(&installed_exe, &process_name, &manifest).await?;
    sync_discord_modules(&discord_exe, &process_name, &manifest).await?;

    // The HTTP CONNECT proxy is only needed while DiscordOnlyDPI updates Discord.
    // Stop it before Discord starts so games and unrelated processes cannot use it.
    updater_proxy_task.abort();

    prepare_discord_startup(&discord_exe, &process_name)?;

    println!("Auto-update complete; updater proxy stopped before Discord launch");
    println!("Strict isolation: Discord-only SOCKS remains");
    println!("Launching: {}", discord_exe.display());
    let voice_tcp = env::args().any(|a| a == "--voice-tcp");
    let mut discord = launch_discord(&discord_exe, voice_tcp)?;

    match tokio::task::spawn_blocking(move || discord.wait()).await {
        Ok(Ok(status)) => println!("Discord exited with status: {status}"),
        Ok(Err(error)) => eprintln!("Failed while waiting for Discord: {error}"),
        Err(error) => eprintln!("Discord wait task failed: {error}"),
    }
    socks_task.abort();
    let _ = byedpi.kill();
    println!("DiscordOnlyDPI shutdown complete");

    Ok(())
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

    let latest_version = json_version(
        manifest
            .get("full")
            .and_then(|v| v.get("host_version"))
            .ok_or("Discord manifest is missing full.host_version")?,
    )?;

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
) -> Result<(), AnyError> {
    let config_dir = match process_name.to_ascii_lowercase().as_str() {
        "discordptb.exe" => "discordptb",
        "discordcanary.exe" => "discordcanary",
        _ => "discord",
    };

    let appdata = env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not set")?;

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

    let manifest_host = json_version(
        manifest
            .get("full")
            .and_then(|v| v.get("host_version"))
            .ok_or("Discord manifest is missing full.host_version")?,
    )?;

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

    let modules_root = appdata.join(config_dir).join(&host_version).join("modules");
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
    Ok(())
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
fn prepare_discord_startup(exe: &Path, process_name: &str) -> Result<(), AnyError> {
    let config_dir = match process_name.to_ascii_lowercase().as_str() {
        "discordptb.exe" => "discordptb",
        "discordcanary.exe" => "discordcanary",
        _ => "discord",
    };

    let appdata = env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not set")?;
    let settings = appdata.join(config_dir).join("settings.json");

    patch_json_flags(
        &settings,
        &[("SKIP_HOST_UPDATE", true), ("SKIP_MODULE_UPDATE", true)],
    )?;

    let resources = exe
        .parent()
        .ok_or("Discord executable has no parent")?
        .join("resources");
    let build_info = resources.join("build_info.json");

    if build_info.is_file() {
        patch_json_flags(
            &build_info,
            &[("newUpdater", false), ("disableUpdater", true)],
        )?;
    }

    println!("Discord internal updaters disabled; modules are managed by DiscordOnlyDPI");
    Ok(())
}

#[cfg(target_os = "windows")]
fn patch_json_flags(path: &Path, flags: &[(&str, bool)]) -> Result<(), AnyError> {
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
        object.insert((*key).to_string(), Value::Bool(*value));
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
fn launch_discord(exe: &Path, voice_tcp: bool) -> Result<Child, AnyError> {
    let parent = exe.parent().ok_or("Discord executable has no parent")?;
    let proxy = format!("socks5://127.0.0.1:{FRONTEND_PORT}");

    let log_stdout = last_log::open_append_file()?;
    let log_stderr = log_stdout.try_clone()?;

    let mut cmd = Command::new(exe);
    cmd.current_dir(parent)
        .arg(format!("--proxy-server={proxy}"))
        .arg("--disable-quic")
        .stdout(Stdio::from(log_stdout))
        .stderr(Stdio::from(log_stderr));

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

    Ok(cmd.spawn()?)
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
        sync::atomic::{AtomicIsize, Ordering},
        thread,
        time::Duration,
    };

    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{Console::GetConsoleWindow, LibraryLoader::GetModuleHandleW},
        UI::{
            Shell::{
                Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
                NOTIFYICONDATAW,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, IsIconic,
                LoadIconW, PostQuitMessage, RegisterClassW, SendMessageW, ShowWindow,
                TranslateMessage, HICON, ICON_BIG, ICON_SMALL, MSG, SW_HIDE, SW_RESTORE, WM_APP,
                WM_DESTROY, WM_LBUTTONUP, WM_SETICON, WNDCLASSW,
            },
        },
    };

    const WM_TRAY_ICON: u32 = WM_APP + 1;
    static CONSOLE_HWND: AtomicIsize = AtomicIsize::new(0);

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
        LoadIconW(instance, 1usize as *const u16)
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_TRAY_ICON && lparam as u32 == WM_LBUTTONUP {
            let raw = CONSOLE_HWND.load(Ordering::SeqCst);
            if raw != 0 {
                ShowWindow(raw as HWND, SW_RESTORE);
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
        icon.uID = 1;
        icon.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        icon.uCallbackMessage = WM_TRAY_ICON;
        icon.hIcon = load_app_icon();

        let tooltip = wide("DiscordOnlyDPI - click to restore");
        let copy_len = tooltip.len().saturating_sub(1).min(icon.szTip.len() - 1);
        icon.szTip[..copy_len].copy_from_slice(&tooltip[..copy_len]);

        if Shell_NotifyIconW(NIM_ADD, &icon) == 0 {
            return;
        }

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
}
