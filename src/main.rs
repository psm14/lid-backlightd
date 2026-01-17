use anyhow::{Context, Result};
use clap::Parser;
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;
use zbus::fdo::PropertiesProxy;
use zbus::names::InterfaceName;
use zbus::zvariant::Value;
use zbus::Connection;

const LOGIND_DEST: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const LOGIND_IFACE: &str = "org.freedesktop.login1.Manager";
const BACKLIGHT_ROOT: &str = "/sys/class/backlight";
const DEVICE_ERROR_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Parser, Debug)]
#[command(name = "lid-backlightd", about = "Dim backlight on lid close via logind")]
struct Args {
    #[arg(long)]
    device: Option<String>,
    #[arg(long, default_value_t = 1)]
    restore_min: u32,
    #[arg(long)]
    log_level: Option<String>,
}

struct Config {
    device: Option<String>,
}

struct Backlight {
    name: String,
    brightness_path: PathBuf,
    max_brightness: u32,
}

struct State {
    device: Option<Backlight>,
    saved_brightness: Option<u32>,
    restore_min: u32,
    last_device_error: Option<Instant>,
}

impl State {
    fn new(restore_min: u32) -> Self {
        Self {
            device: None,
            saved_brightness: None,
            restore_min,
            last_device_error: None,
        }
    }

    fn log_device_error(&mut self, err: &anyhow::Error) {
        let now = Instant::now();
        if let Some(last) = self.last_device_error {
            if now.duration_since(last) < DEVICE_ERROR_INTERVAL {
                return;
            }
        }
        self.last_device_error = Some(now);
        warn!(error = %err, "Backlight access failed");
    }

    fn ensure_device(&mut self, config: &Config) -> Result<()> {
        if self.device.is_some() {
            return Ok(());
        }

        let device = discover_device(config.device.as_deref())?;
        info!(device = %device.name, max_brightness = device.max_brightness, "Using backlight device");
        self.device = Some(device);
        Ok(())
    }

    fn handle_lid_change(&mut self, config: &Config, closed: bool) -> Result<()> {
        if closed {
            if self.saved_brightness.is_none() {
                self.on_lid_close(config)
            } else {
                Ok(())
            }
        } else {
            if self.saved_brightness.is_some() {
                self.on_lid_open(config)
            } else {
                Ok(())
            }
        }
    }

    fn on_lid_close(&mut self, config: &Config) -> Result<()> {
        self.ensure_device(config)?;
        let (device_name, brightness_path) = {
            let device = self.device.as_ref().context("backlight device missing")?;
            (device.name.clone(), device.brightness_path.clone())
        };
        let cur = read_u32(&brightness_path).context("read brightness")?;

        if self.saved_brightness.is_none() {
            self.saved_brightness = Some(cur);
        }

        let mut dimmed = false;
        if let Err(err) = write_u32(&brightness_path, 0) {
            self.handle_device_error(&err);
            if let Err(err) = write_u32(&brightness_path, 1) {
                self.handle_device_error(&err);
            } else {
                dimmed = true;
            }
        } else {
            dimmed = true;
        }

        if dimmed {
            info!(device = %device_name, "Lid closed, dimmed backlight");
        } else {
            warn!(device = %device_name, "Lid closed, failed to dim backlight");
        }
        Ok(())
    }

    fn on_lid_open(&mut self, config: &Config) -> Result<()> {
        let Some(saved) = self.saved_brightness else {
            debug!("Lid opened, no saved brightness to restore");
            return Ok(());
        };

        self.ensure_device(config)?;
        let (device_name, brightness_path, max_brightness) = {
            let device = self.device.as_ref().context("backlight device missing")?;
            (
                device.name.clone(),
                device.brightness_path.clone(),
                device.max_brightness,
            )
        };
        let min_restore = self.restore_min.min(max_brightness);
        let restore = saved.clamp(min_restore, max_brightness);

        if let Err(err) = write_u32(&brightness_path, restore) {
            self.handle_device_error(&err);
        } else {
            info!(device = %device_name, restore, "Lid opened, restored backlight");
            self.saved_brightness = None;
        }

        Ok(())
    }

    fn restore_on_exit(&mut self, config: &Config) -> Result<()> {
        if self.saved_brightness.is_none() {
            return Ok(());
        }
        self.on_lid_open(config)
    }

    fn handle_device_error(&mut self, err: &anyhow::Error) {
        let not_found = err.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io_err| io_err.kind() == std::io::ErrorKind::NotFound)
        });
        if not_found {
            self.device = None;
        }
        self.log_device_error(err);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let filter = match args.log_level.as_deref() {
        Some(level) => EnvFilter::new(level),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = Config { device: args.device };
    let mut state = State::new(args.restore_min);
    if let Err(err) = state.ensure_device(&config) {
        state.handle_device_error(&err);
    }

    let shutdown = shutdown_signal();
    tokio::select! {
        res = run_loop(&config, &mut state) => {
            if let Err(err) = res {
                warn!(error = %err, "DBus loop exited");
            }
        }
        _ = shutdown => {
            info!("Shutdown requested");
        }
    }

    if let Err(err) = state.restore_on_exit(&config) {
        warn!(error = %err, "Failed to restore brightness on shutdown");
    }

    Ok(())
}

async fn run_loop(config: &Config, state: &mut State) -> Result<()> {
    let mut backoff = Backoff::new(Duration::from_millis(250), Duration::from_secs(5));

    loop {
        let connection = match Connection::system().await {
            Ok(conn) => {
                backoff.reset();
                conn
            }
            Err(err) => {
                warn!(error = %err, "Failed to connect to system bus");
                let delay = backoff.next_delay();
                sleep(delay).await;
                continue;
            }
        };

        let proxy_builder = match PropertiesProxy::builder(&connection)
            .destination(LOGIND_DEST)
            .and_then(|builder| builder.path(LOGIND_PATH))
        {
            Ok(builder) => builder,
            Err(err) => {
                warn!(error = %err, "Failed to build logind properties proxy");
                let delay = backoff.next_delay();
                sleep(delay).await;
                continue;
            }
        };

        let proxy = match proxy_builder.build().await {
            Ok(proxy) => proxy,
            Err(err) => {
                warn!(error = %err, "Failed to build logind properties proxy");
                let delay = backoff.next_delay();
                sleep(delay).await;
                continue;
            }
        };

        if let Err(err) = process_connection(config, state, &proxy).await {
            warn!(error = %err, "DBus connection error");
        }

        let delay = backoff.next_delay();
        sleep(delay).await;
    }
}

async fn process_connection(config: &Config, state: &mut State, proxy: &PropertiesProxy<'_>) -> Result<()> {
    let iface = InterfaceName::from_static_str_unchecked(LOGIND_IFACE);
    match proxy.get(iface, "LidClosed").await {
        Ok(value) => match bool::try_from(&value) {
            Ok(closed) => {
                debug!(closed, "Initial lid state");
                if let Err(err) = state.handle_lid_change(config, closed) {
                    state.handle_device_error(&err);
                }
            }
            Err(err) => {
                warn!(error = %err, "Invalid LidClosed value");
            }
        },
        Err(err) => {
            warn!(error = %err, "Failed to read initial lid state");
        }
    }

    let mut stream = proxy.receive_properties_changed().await?;
    while let Some(signal) = stream.next().await {
        let args = match signal.args() {
            Ok(args) => args,
            Err(err) => {
                warn!(error = %err, "Failed to decode PropertiesChanged signal");
                continue;
            }
        };
        if args.interface_name() != LOGIND_IFACE {
            continue;
        }
        let changed = args.changed_properties();
        let Some(value) = changed.get("LidClosed") else {
            continue;
        };
        let closed = match <&Value as TryInto<bool>>::try_into(value) {
            Ok(value) => value,
            Err(err) => {
                warn!(error = %err, "Invalid LidClosed value");
                continue;
            }
        };

        if let Err(err) = state.handle_lid_change(config, closed) {
            state.handle_device_error(&err);
        }
    }

    Err(anyhow::anyhow!("properties stream ended"))
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm = signal(SignalKind::terminate()).expect("signal handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn discover_device(device_override: Option<&str>) -> Result<Backlight> {
    let mut devices = list_backlight_devices().context("list backlight devices")?;
    if devices.is_empty() {
        anyhow::bail!("no backlight devices found in {}", BACKLIGHT_ROOT);
    }

    let chosen = if let Some(requested) = device_override {
        if !devices.iter().any(|name| name == requested) {
            anyhow::bail!("requested device '{}' not found", requested);
        }
        requested.to_string()
    } else if devices.iter().any(|name| name == "intel_backlight") {
        "intel_backlight".to_string()
    } else {
        devices.sort();
        devices[0].clone()
    };

    Backlight::new(chosen)
}

fn list_backlight_devices() -> Result<Vec<String>> {
    let mut devices = Vec::new();
    for entry in std::fs::read_dir(BACKLIGHT_ROOT).context("read backlight directory")? {
        let entry = entry.context("read backlight entry")?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.is_empty() {
            devices.push(name.to_string());
        }
    }
    Ok(devices)
}

impl Backlight {
    fn new(name: String) -> Result<Self> {
        let base = PathBuf::from(BACKLIGHT_ROOT).join(&name);
        let brightness_path = base.join("brightness");
        let max_brightness_path = base.join("max_brightness");
        let max_brightness = read_u32(&max_brightness_path).context("read max_brightness")?;

        Ok(Self {
            name,
            brightness_path,
            max_brightness,
        })
    }
}

fn read_u32(path: &Path) -> Result<u32> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let value = contents.trim().parse::<u32>().with_context(|| {
        format!("parse u32 from {} (value: {})", path.display(), contents.trim())
    })?;
    Ok(value)
}

fn write_u32(path: &Path, value: u32) -> Result<()> {
    std::fs::write(path, value.to_string())
        .with_context(|| format!("write {} to {}", value, path.display()))?;
    Ok(())
}

struct Backoff {
    base: Duration,
    max: Duration,
    current: Duration,
}

impl Backoff {
    fn new(base: Duration, max: Duration) -> Self {
        Self {
            base,
            max,
            current: base,
        }
    }

    fn reset(&mut self) {
        self.current = self.base;
    }

    fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = (self.current * 2).min(self.max);
        delay
    }
}
