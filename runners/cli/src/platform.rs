//! Host, filesystem and process facts the operator commands report.
//!
//! Everything that differs between Windows and Unix lives behind a `cfg`
//! boundary in this module so the command layer stays platform-free.

use qualia_types::{parse_stack_manifest, StackManifest};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The stack manifest compiled into the binary, used when the operator does
/// not name one.
const DEFAULT_MANIFEST: &str = include_str!("../../../config/stack-manifest.default.json");

/// Milliseconds-free monotonic-enough wall clock for identifiers and wire
/// timestamps.
pub fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

pub fn host_name() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".to_string())
    }

    #[cfg(not(windows))]
    {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

/// Load a manifest from disk, or fall back to the compiled-in default.
pub fn load_stack_manifest(path: Option<&Path>) -> Result<StackManifest, String> {
    let text = match path {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|err| format!("failed to read stack manifest '{}': {err}", path.display()))?,
        None => DEFAULT_MANIFEST.to_string(),
    };
    parse_stack_manifest(&text)
}

/// Locate the stack supervisor beside this binary, or in the cargo target
/// directory the binary was built into.
pub fn resolve_init_binary() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|err| format!("current_exe failed: {err}"))?;
    let bin_dir = current
        .parent()
        .ok_or_else(|| "cannot determine CLI binary directory".to_string())?;

    let exe_name = if cfg!(windows) {
        "qualia-init.exe"
    } else {
        "qualia-init"
    };

    let sibling = bin_dir.join(exe_name);
    if sibling.exists() {
        return Ok(sibling);
    }

    if let Some(dir) = find_target_dir(bin_dir) {
        let candidate = dir.join(exe_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(format!(
        "could not find {exe_name}; build qualia-init first or run from a cargo target directory"
    ))
}

/// Walk up from `start` looking for a directory that already holds the
/// supervisor binary; cargo puts every workspace binary in the same place.
fn find_target_dir(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        if ancestor.join("qualia-init").exists() || ancestor.join("qualia-init.exe").exists() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

/// The lowercase process names currently alive on this host.
pub fn running_process_names() -> Result<HashSet<String>, String> {
    #[cfg(windows)]
    {
        let output = Command::new("tasklist")
            .args(["/FO", "CSV", "/NH"])
            .output()
            .map_err(|err| format!("tasklist failed: {err}"))?;
        if !output.status.success() {
            return Err(format!("tasklist returned {}", output.status));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut names = HashSet::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('"').and_then(|rest| rest.split("\",").next()) {
                names.insert(name.to_ascii_lowercase());
            }
        }
        Ok(names)
    }

    #[cfg(not(windows))]
    {
        let output = Command::new("ps")
            .args(["-A", "-o", "comm="])
            .output()
            .map_err(|err| format!("ps failed: {err}"))?;
        if !output.status.success() {
            return Err(format!("ps returned {}", output.status));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(text
            .lines()
            .map(|line| line.trim().to_ascii_lowercase())
            .filter(|line| !line.is_empty())
            .collect())
    }
}

/// The executable name a runner's process shows up under.
pub fn process_name_for_runner(runner_name: &str) -> String {
    if cfg!(windows) {
        format!("{runner_name}.exe").to_ascii_lowercase()
    } else {
        runner_name.to_ascii_lowercase()
    }
}

/// The final `count` lines of `text`, in order.
pub fn tail_lines(text: &str, count: usize) -> Vec<&str> {
    let mut lines = text.lines().collect::<Vec<_>>();
    if lines.len() > count {
        lines.drain(0..lines.len() - count);
    }
    lines
}

fn detect_cpu_model() -> Option<String> {
    #[cfg(windows)]
    {
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return (!text.is_empty()).then_some(text);
    }

    #[cfg(not(windows))]
    {
        let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        text.lines()
            .find_map(|line| line.strip_prefix("model name\t: ").map(str::to_string))
    }
}

fn detect_memory_gb() -> Option<f64> {
    #[cfg(windows)]
    {
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "[double](Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        return String::from_utf8_lossy(&output.stdout).trim().parse().ok();
    }

    #[cfg(not(windows))]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kilobytes = text
            .lines()
            .find_map(|line| line.strip_prefix("MemTotal:"))?
            .split_whitespace()
            .next()?
            .parse::<f64>()
            .ok()?;
        Some(kilobytes / (1024.0 * 1024.0))
    }
}

fn detect_gpu_summary() -> Option<String> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,driver_version,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    (!line.is_empty()).then_some(line)
}

fn detect_nvcc_version() -> Option<String> {
    let output = Command::new("nvcc").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .find(|line| line.contains("release"))
        .map(|line| line.trim().to_string())
}

/// Report the local host and toolchain. Always succeeds: a missing tool is
/// reported as `unavailable`, never as an error.
pub fn print_host() -> std::process::ExitCode {
    println!("host: {}", host_name());
    println!("os: {}", std::env::consts::OS);
    println!("arch: {}", std::env::consts::ARCH);

    if let Some(cpu) = detect_cpu_model() {
        println!("cpu: {cpu}");
    }
    if let Some(memory_gb) = detect_memory_gb() {
        println!("memory_gb: {memory_gb:.1}");
    }
    match detect_gpu_summary() {
        Some(gpu) => println!("gpu: {gpu}"),
        None => println!("gpu: unavailable"),
    }
    match detect_nvcc_version() {
        Some(version) => println!("nvcc: {version}"),
        None => println!("nvcc: unavailable"),
    }

    std::process::ExitCode::SUCCESS
}

/// The first field of the first GPU line, for the fallback report.
pub fn gpu_device_label() -> String {
    detect_gpu_summary()
        .and_then(|line| line.split(',').next().map(|part| part.trim().to_string()))
        .unwrap_or_else(|| "unavailable".to_string())
}

/// Where the compute service listens when the operator names no socket.
pub fn default_compute_socket() -> String {
    std::env::var("QUALIA_COMPUTE_SOCKET").unwrap_or_else(|_| {
        if cfg!(windows) {
            "127.0.0.1:46321".to_string()
        } else {
            "/tmp/qualia-compute.sock".to_string()
        }
    })
}

/// The agent's local control surface. `QUALIA_AGENT_URL` is the operator's
/// explicit override and wins outright; without it the port comes from
/// `QUALIA_WEB_PORT` on loopback, which is the only address the CLI assumes.
pub fn default_agent_url() -> String {
    let configured = std::env::var("QUALIA_AGENT_URL").ok();
    let port = std::env::var("QUALIA_WEB_PORT").ok();
    agent_url_from(configured.as_deref(), port.as_deref())
}

/// The URL precedence behind [`default_agent_url`], kept free of the
/// environment so the order can be tested directly.
fn agent_url_from(configured: Option<&str>, port: Option<&str>) -> String {
    if let Some(url) = configured {
        let url = url.trim().trim_end_matches('/');
        if !url.is_empty() {
            return url.to_string();
        }
    }
    format!("https://127.0.0.1:{}", port.unwrap_or("8080"))
}

/// Fetch the compute service's capability document over its raw socket.
pub fn fetch_compute_capabilities(
    socket: &str,
) -> Result<crate::world::ComputeCapabilities, String> {
    use std::io::{BufRead, BufReader, Write};

    #[cfg(windows)]
    use std::net::TcpStream as PlatformStream;
    #[cfg(not(windows))]
    use std::os::unix::net::UnixStream as PlatformStream;

    let mut stream =
        PlatformStream::connect(socket).map_err(|err| format!("connect to {socket} failed: {err}"))?;
    let request = crate::world::ComputeRequestMeta {
        schema_version: "compute.v1".to_string(),
        request_id: "qualia-cli".to_string(),
        request_type: "capabilities".to_string(),
        timestamp_ns: now_ns(),
    };
    let body = serde_json::to_vec(&request).map_err(|err| err.to_string())?;
    stream.write_all(&body).map_err(|err| err.to_string())?;
    stream.write_all(b"\n").map_err(|err| err.to_string())?;
    stream.flush().map_err(|err| err.to_string())?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|err| err.to_string())?;
    serde_json::from_str(line.trim()).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_lines_keeps_the_last_requested_lines() {
        assert_eq!(tail_lines("a\nb\nc\nd", 2), vec!["c", "d"]);
        assert_eq!(tail_lines("a\nb", 5), vec!["a", "b"]);
        assert_eq!(tail_lines("only", 1), vec!["only"]);
        assert_eq!(tail_lines("", 3), Vec::<&str>::new());
    }

    #[test]
    fn compiled_default_manifest_is_valid() {
        let manifest = load_stack_manifest(None).expect("default manifest must parse");
        assert_eq!(manifest.schema_version, "qualia.stack.v1");
        assert_eq!(manifest.stack_name, "qualia-default");
        assert!(!manifest.runners.is_empty());
    }

    #[test]
    fn runner_process_name_is_lowercased_and_suffixed_on_windows() {
        let name = process_name_for_runner("Qualia-Agent");
        if cfg!(windows) {
            assert_eq!(name, "qualia-agent.exe");
        } else {
            assert_eq!(name, "qualia-agent");
        }
    }

    #[test]
    fn agent_url_tracks_the_web_port() {
        // The default must remain the documented loopback surface.
        let url = agent_url_from(None, Some("18081"));
        assert_eq!(url, "https://127.0.0.1:18081");
    }

    #[test]
    fn configured_agent_url_wins_over_the_port() {
        // `QUALIA_AGENT_URL` is the operator's explicit override; the loopback
        // default applies only when it is absent or blank.
        assert_eq!(
            agent_url_from(Some("http://192.0.2.10:9000/"), Some("18081")),
            "http://192.0.2.10:9000"
        );
        assert_eq!(
            agent_url_from(Some("   "), Some("18081")),
            "https://127.0.0.1:18081"
        );
        assert_eq!(agent_url_from(None, None), "https://127.0.0.1:8080");
    }

    #[test]
    fn clock_is_wall_time_nanoseconds() {
        let ns = now_ns();
        assert!(ns > 1_600_000_000_000_000_000, "unexpected clock value: {ns}");
    }
}
