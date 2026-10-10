//! Native observations only. A driver utility is NOT a llama backend load test.
use std::{
    path::Path,
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncReadExt;
use unoone_model_admission::*;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

async fn bounded_command(program: &str, args: &[&str]) -> Option<String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?.take(16_385);
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.ok()?;
        if bytes.len() > 16_384 {
            let _ = child.kill().await;
            return None;
        }
        if !child.wait().await.ok()?.success() {
            return None;
        }
        String::from_utf8(bytes).ok()
    })
    .await
    .ok()
    .flatten()
}

pub(crate) fn parse_nvidia(s: &str) -> Option<(String, u64, u64)> {
    // Multi-device capacities cannot be pooled. Display the first detected GPU only.
    let fields: Vec<_> = s.lines().next()?.split(',').map(str::trim).collect();
    if fields.len() != 3
        || fields[0].is_empty()
        || fields[0].len() > 256
        || fields[0].chars().any(char::is_control)
    {
        return None;
    }
    let total = fields[1].parse::<u64>().ok()?.checked_mul(1_048_576)?;
    let available = fields[2].parse::<u64>().ok()?.checked_mul(1_048_576)?;
    if available > total || total == 0 {
        return None;
    }
    Some((fields[0].into(), total, available))
}

/// Available bytes on the selected filesystem, never sys_info's unrelated root disk.
/// Unknown on platforms without a reviewed native volume adapter.
pub async fn usable_storage(root: &Path) -> Observation<u64> {
    #[cfg(unix)]
    {
        // First run may precede vault creation; nearest existing ancestor is the same volume.
        if let Some(path) = root.ancestors().find(|p| p.is_dir()).and_then(Path::to_str) {
            if let Some(out) = bounded_command("df", &["-Pk", path]).await {
                if let Some(bytes) = out
                    .lines()
                    .last()
                    .and_then(|l| l.split_whitespace().nth(3))
                    .and_then(|s| s.parse::<u64>().ok())
                    .and_then(|n| n.checked_mul(1024))
                {
                    return Observation::Detected(bytes);
                }
            }
        }
    }
    let _ = root;
    Observation::Unknown
}

pub async fn probe(root: &Path) -> DeviceProbe {
    let memory = sys_info::mem_info().ok();
    let total = memory.as_ref().and_then(|m| m.total.checked_mul(1024));
    let available = memory.as_ref().and_then(|m| m.avail.checked_mul(1024));
    let measured = |v: Option<u64>| v.map(Observation::Detected).unwrap_or(Observation::Unknown);
    let mut p = DeviceProbe {
        schema_version: SCHEMA_VERSION,
        probe_id: uuid::Uuid::new_v4().to_string(),
        captured_at_ms: now_ms(),
        device_class: Observation::Unknown,
        os: Observation::Detected(std::env::consts::OS.into()),
        os_version: sys_info::os_release()
            .map(Observation::Detected)
            .unwrap_or(Observation::Unknown),
        os_api_level: Observation::Unknown,
        abi: Observation::Detected(std::env::consts::ARCH.into()),
        cpu_features: Observation::Unknown,
        total_ram_bytes: measured(total.filter(|x| *x > 0)),
        available_ram_bytes: measured(available.filter(|x| total.is_some_and(|t| *x <= t))),
        gpu_name: Observation::Unknown,
        total_vram_bytes: Observation::Unknown,
        available_vram_bytes: Observation::Unknown,
        unified_memory: Observation::Unknown,
        low_memory_threshold_bytes: Observation::Unknown,
        native_budget_bytes: Observation::Unknown,
        heap_budget_bytes: Observation::Unknown,
        usable_storage_bytes: usable_storage(root).await,
        disk_bytes_per_second: Observation::Unknown,
        battery_percent: Observation::Unknown,
        thermal: Observation::Unknown,
        backends: vec![],
    };
    #[cfg(target_arch = "x86_64")]
    {
        let mut f = vec![];
        if std::is_x86_feature_detected!("avx") {
            f.push("avx".into());
        }
        if std::is_x86_feature_detected!("avx2") {
            f.push("avx2".into());
        }
        if std::is_x86_feature_detected!("fma") {
            f.push("fma".into());
        }
        p.cpu_features = Observation::Detected(f);
    }
    if let Some((name, total, free)) = bounded_command(
        "nvidia-smi",
        &[
            "--query-gpu=name,memory.total,memory.free",
            "--format=csv,noheader,nounits",
        ],
    )
    .await
    .as_deref()
    .and_then(parse_nvidia)
    {
        p.gpu_name = Observation::Detected(name);
        p.total_vram_bytes = Observation::Detected(total);
        p.available_vram_bytes = Observation::Detected(free);
        p.backends.push(BackendProbe {
            runtime: "llama.cpp".into(),
            runtime_version: "unknown".into(),
            backend: "cuda".into(),
            driver_version: "unknown".into(),
            health: Observation::Detected(BackendHealth::DetectedOnly),
        });
    }
    // CPU/Metal/Vulkan backend health remains UNKNOWN, not inferred from OS or DLL presence.
    for backend in ["cpu", "metal", "vulkan"] {
        p.backends.push(BackendProbe {
            runtime: "llama.cpp".into(),
            runtime_version: "unknown".into(),
            backend: backend.into(),
            driver_version: "unknown".into(),
            health: Observation::Unknown,
        });
    }
    p.captured_at_ms = now_ms();
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gpu_mib_conversion_and_unknown_not_zero() {
        assert_eq!(
            parse_nvidia("Fixture GPU, 8192, 4096\n"),
            Some(("Fixture GPU".into(), 8_589_934_592, 4_294_967_296))
        );
        assert!(parse_nvidia("GPU, N/A, N/A").is_none());
        assert!(parse_nvidia("GPU, 1, 2").is_none());
    }
    #[tokio::test]
    async fn actual_host_probe_never_promotes_backend() {
        let p = probe(Path::new("/tmp")).await;
        assert!(p
            .backends
            .iter()
            .all(|b| !matches!(b.health, Observation::Tested(_))));
        assert!(matches!(p.device_class, Observation::Unknown));
    }
}
