//! Timing harness for the DesktopLaunch asset sweep (dev diagnostic).
//! Reproduces validate_package's per-asset hash loop with per-file timing so
//! a slow launch can be attributed to the exact asset, not guessed at.
//! Run: cargo run --release --example sweep_timing -- D:\UNOONE

use std::path::Path;
use std::time::Instant;

fn sha256_file_timed(path: &Path) -> Result<(String, u64, std::time::Duration), String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut total: u64 = 0;
    let started = Instant::now();
    loop {
        let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        total += count as u64;
    }
    Ok((hex::encode_upper(hasher.finalize()), total, started.elapsed()))
}

use sha2::{Digest, Sha256};

fn main() {
    let root = std::env::args()
        .nth(1)
        .expect("usage: sweep_timing <vault-root>");
    let root = Path::new(&root);
    let manifest_content = std::fs::read_to_string(root.join("manifest.json")).expect("manifest");
    let manifest: unoone_usb_manifest::PocketManifest =
        serde_json::from_str(&manifest_content).expect("manifest parse");
    let windows = &manifest.platforms.windows;

    let mut assets: Vec<&unoone_usb_manifest::AssetSpec> = vec![&windows.desktop];
    assets.extend(windows.runtimes.iter().filter(|a| a.required));
    assets.extend(windows.models.iter().filter(|a| a.required));
    assets.extend(windows.voice.iter().filter(|a| a.required));
    if let Some(speech) = windows.speech.as_ref() {
        assets.extend(
            speech
                .models
                .iter()
                .chain(&speech.configs)
                .chain(&speech.acceptance)
                .chain(&speech.runtimes)
                .filter(|a| a.required),
        );
    }
    if let Some(starter) = windows.starter.as_ref() {
        assets.push(starter);
    }
    if let Some(dock) = windows.dock.as_ref() {
        assets.push(dock);
    }

    println!("assets to hash: {}", assets.len());
    let started = Instant::now();
    let mut records = Vec::new();
    for asset in &assets {
        let path = root.join(asset.path.replace('\\', "/"));
        match sha256_file_timed(&path) {
            Ok((digest, bytes, elapsed)) => {
                let ms = elapsed.as_millis() as u64;
                if ms > 300 {
                    println!(
                        "SLOW {ms:>6}ms {:>8.1} MB {}",
                        bytes as f64 / 1048576.0,
                        path.display()
                    );
                }
                records.push((asset.path.clone(), ms, bytes, digest));
            }
            Err(error) => println!("FAIL {} {}", asset.path, error),
        }
    }
    let total = started.elapsed();
    println!();
    println!("total sweep: {:.1}s", total.as_secs_f32());
    records.sort_by_key(|r| std::cmp::Reverse(r.1));
    println!("top 12 slowest assets:");
    for (path, ms, bytes, digest) in records.iter().take(12) {
        println!(
            "{ms:>6}ms {:>8.2}GB  {path}",
            *bytes as f64 / 1073741824.0
        );
        let _ = digest;
    }
}