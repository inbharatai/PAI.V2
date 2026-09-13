// UnoOne Power — Desktop Accessibility Adapters
// Blind View (screen reader), OCR, camera adapters for desktop
//
// Vision and OCR use the local Gemma model with mmproj (multimodal projector)
// loaded via llama-server — no external Tesseract or separate vision model needed.
// Camera access uses Tauri WebView + getUserMedia (Phase 5).

use crate::llama::{Content, ConversationTurn, InferenceRequest, ModelManagerState};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Accessibility feature status — reads real OS accessibility settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessibilityStatus {
    pub screen_reader_detected: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,
    pub font_scale: f32,
    pub screen_reader_name: String,
}

/// OCR result from image/document scan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrResult {
    pub text: String,
    /// The current multimodal server does not return calibrated OCR confidence.
    pub confidence: Option<f32>,
    pub language: String,
    pub processing_time_ms: u64,
}

/// Blind View description result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlindViewResult {
    pub description: String,
    pub objects: Vec<DetectedObject>,
    /// No calibrated confidence is available from the generative vision model.
    pub confidence: Option<f32>,
}

/// Detected object in an image
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedObject {
    pub label: String,
    pub confidence: f32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Camera frame info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraInfo {
    pub devices: Vec<CameraDevice>,
    pub capture_backend: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraDevice {
    pub name: String,
    pub device_id: String,
    pub status: String,
}

// Tauri commands

#[tauri::command]
pub fn get_accessibility_status() -> AccessibilityStatus {
    // Detect real OS accessibility settings
    let mut screen_reader_detected = false;
    let mut screen_reader_name = "Unknown".to_string();
    let mut high_contrast = false;
    let reduced_motion = false;
    let font_scale = 1.0;

    if cfg!(target_os = "windows") {
        // Check for Windows screen readers via NVDA/JAWS process detection
        if let Ok(output) = std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq nvda.exe"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("nvda.exe") {
                screen_reader_detected = true;
                screen_reader_name = "NVDA".to_string();
            }
        }

        if !screen_reader_detected {
            if let Ok(output) = std::process::Command::new("tasklist")
                .args(["/FI", "IMAGENAME eq jaws.exe"])
                .output()
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains("jaws.exe") {
                    screen_reader_detected = true;
                    screen_reader_name = "JAWS".to_string();
                }
            }
        }

        // Check Windows high contrast mode via registry
        if let Ok(output) = std::process::Command::new("reg")
            .args([
                "query",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Themes",
                "/v",
                "CurrentTheme",
            ])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Simplified check — high contrast themes contain "High Contrast" in path
            high_contrast = stdout.to_lowercase().contains("high contrast");
        }
    } else if cfg!(target_os = "macos") {
        // macOS VoiceOver detection
        if let Ok(output) = std::process::Command::new("defaults")
            .args([
                "read",
                "com.apple.accessibility",
                "ApplicationAccessibilityEnabled",
            ])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("1") {
                screen_reader_detected = true;
                screen_reader_name = "VoiceOver".to_string();
            }
        }
    }

    AccessibilityStatus {
        screen_reader_detected,
        high_contrast,
        reduced_motion,
        font_scale,
        screen_reader_name,
    }
}

/// Perform OCR on an image by sending it to llama-server with mmproj.
/// The model sees the image and transcribes all visible text.
#[tauri::command]
pub async fn perform_ocr(
    image_path: String,
    model_state: tauri::State<'_, ModelManagerState>,
) -> Result<OcrResult, String> {
    let start = std::time::Instant::now();

    // Read and base64-encode the image
    let path = PathBuf::from(&image_path);
    if !path.exists() {
        return Err(format!("Image file not found: {}", image_path));
    }

    let image_bytes = std::fs::read(&path).map_err(|e| format!("Failed to read image: {}", e))?;

    // Determine MIME type from extension
    let mime_type = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/png", // default
    };

    let image_base64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

    // Send to llama-server with an OCR prompt
    let request = InferenceRequest {
        prompt: String::new(),
        system_prompt: Some("You are an OCR assistant. Transcribe ALL text visible in the image exactly as written. Preserve layout and formatting where possible. Output ONLY the transcribed text, nothing else.".to_string()),
        conversation_history: vec![ConversationTurn {
            role: "user".to_string(),
            content: Content::with_image(
                "Transcribe all text in this image.",
                &image_base64,
                mime_type,
            ),
            tool_calls: None,
            tool_call_id: None,
        }],
        max_tokens: Some(4096),
        temperature: Some(0.1), // Low temperature for accurate transcription
        stop_sequences: None,
        tools: None,
    };

    let port = *model_state
        .server_port
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let manager = model_state.manager.lock().await;
    let manager = manager
        .as_ref()
        .ok_or("Model manager not initialized — cannot perform OCR. Start the model first.")?;

    let response = manager
        .send_completion(&request, port)
        .await
        .map_err(|e| format!("OCR inference failed: {}", e))?;

    let processing_time_ms = start.elapsed().as_millis() as u64;

    Ok(OcrResult {
        text: response.text,
        confidence: None,
        language: "auto".to_string(),
        processing_time_ms,
    })
}

/// Describe an image for a visually impaired user by sending it to llama-server.
/// Uses the Gemma multimodal model with a detailed description prompt.
#[tauri::command]
pub async fn describe_image(
    image_path: String,
    model_state: tauri::State<'_, ModelManagerState>,
) -> Result<BlindViewResult, String> {
    // Read and base64-encode the image
    let path = PathBuf::from(&image_path);
    if !path.exists() {
        return Err(format!("Image file not found: {}", image_path));
    }

    let image_bytes = std::fs::read(&path).map_err(|e| format!("Failed to read image: {}", e))?;

    // Determine MIME type from extension
    let mime_type = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/png",
    };

    let image_base64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

    // Send to llama-server with a blind-view description prompt
    let request = InferenceRequest {
        prompt: String::new(),
        system_prompt: Some("You are a visual accessibility assistant for blind and low-vision users. Describe images in detail, focusing on:\n1. Main subject and scene\n2. Text visible in the image\n3. Colors and spatial layout\n4. People, objects, and their positions\n5. Any important details a blind person would want to know\nBe concise but thorough. Avoid phrases like 'I can see' or 'the image shows'.".to_string()),
        conversation_history: vec![ConversationTurn {
            role: "user".to_string(),
            content: Content::with_image(
                "Describe this image in detail for a visually impaired person.",
                &image_base64,
                mime_type,
            ),
            tool_calls: None,
            tool_call_id: None,
        }],
        max_tokens: Some(2048),
        temperature: Some(0.7),
        stop_sequences: None,
        tools: None,
    };

    let port = *model_state
        .server_port
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let manager = model_state.manager.lock().await;
    let manager = manager
        .as_ref()
        .ok_or("Model manager not initialized — cannot describe image. Start the model first.")?;

    let response = manager
        .send_completion(&request, port)
        .await
        .map_err(|e| format!("Image description failed: {}", e))?;

    Ok(BlindViewResult {
        description: response.text,
        objects: Vec::new(), // Object detection is not available without a separate model
        confidence: None,
    })
}

/// Save a captured camera frame (a `data:image/jpeg;base64,...` data URL from
/// the Vision Lab's canvas snapshot) to a file the vision pipeline can read,
/// and return its path. Live-caught 2026-09-12 (defect #19): captured frames
/// used to live only as DOM thumbnails — a blind user's snapshot could never
/// reach `describe_image`/`perform_ocr`, which take a file path. Snapshots are
/// written to the host temp area (never the read-mostly vault package).
/// Async (defect #23 family): sync commands run on the main/UI thread and a
/// blocked UI thread drops the immediately-following describe/OCR IPC request.
#[tauri::command]
pub async fn save_vision_snapshot(data_url: String) -> Result<String, String> {
    let (header, payload) = data_url
        .split_once(',')
        .ok_or_else(|| "Snapshot data URL is malformed (no comma separator)".to_string())?;
    if !header.starts_with("data:image/") {
        return Err(format!("Snapshot data URL is not an image: {header}"));
    }
    let mime_subtype = header
        .strip_prefix("data:image/")
        .and_then(|rest| rest.split(';').next())
        .unwrap_or("jpeg")
        .to_owned();
    let extension = match mime_subtype.as_str() {
        "png" => "png",
        "gif" => "gif",
        "webp" => "webp",
        "bmp" => "bmp",
        _ => "jpg",
    };
    let image_bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|e| format!("Snapshot payload is not valid base64: {e}"))?;
    write_vision_artifact("snapshot", extension, &image_bytes)
}

/// Write an image into the vision pipeline's host-temp directory and return
/// its path. Never the read-mostly vault package.
fn write_vision_artifact(
    prefix: &str,
    extension: &str,
    image_bytes: &[u8],
) -> Result<String, String> {
    if image_bytes.is_empty() {
        return Err("Image payload is empty".to_string());
    }
    let dir = std::env::temp_dir().join("unoone-vision");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create snapshot dir: {e}"))?;
    let filename = format!(
        "{prefix}-{}.{extension}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3f")
    );
    let path = dir.join(filename);
    std::fs::write(&path, image_bytes).map_err(|e| format!("Cannot write snapshot: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// Capture the main window's on-screen region as a PNG into the vision
/// pipeline and return its path, ready for `describe_image`/`perform_ocr`.
/// Live-caught 2026-09-12 (defect #20): the Screen Reader Description assist
/// could only describe camera snapshots — a blind user had no way to ask
/// "what is on my screen right now?". This captures what the app is actually
/// showing and feeds the same describe-and-speak path.
///
/// Live-caught 2026-09-13 (defect #23): this was a SYNC command, and Tauri v2
/// runs non-async commands on the MAIN/UI thread — the GDI capture blocked
/// the UI thread right before the immediately-following `describe_image` IPC
/// request, which was then silently dropped (the describe promise never
/// settled, the button wedged on "Running vision model…" forever; 4-for-4
/// reproducible, while a direct invoke seconds later always worked). Async +
/// spawn_blocking moves the capture fully off the UI thread.
#[tauri::command]
pub async fn capture_screen_snapshot(app: tauri::AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let png_bytes = crate::browser::capture_window_png(&app, "main")?;
        write_vision_artifact("screen", "png", &png_bytes)
    })
    .await
    .map_err(|e| format!("Screen capture task failed: {e}"))?
}

/// Camera info — enumerates available video capture devices.
/// Actual frame capture uses the frontend WebView + getUserMedia API.
/// Async since it shells out to PowerShell (seconds-long on the main thread
/// otherwise — defect #23 family: sync commands freeze the UI thread).
#[tauri::command]
pub async fn get_camera_info() -> Result<CameraInfo, String> {
    let devices = tauri::async_runtime::spawn_blocking(|| {
        let mut devices = Vec::new();
        if cfg!(target_os = "windows") {
            let output = std::process::Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-Command",
                    "Get-PnpDevice -PresentOnly | Where-Object { $_.Class -in @('Camera','Image') } | Select-Object FriendlyName,InstanceId,Status | ConvertTo-Json -Compress",
                ])
                .output()
                .map_err(|error| format!("Camera enumeration failed: {error}"))?;
            if output.status.success() {
                let value: serde_json::Value = serde_json::from_slice(&output.stdout)
                    .unwrap_or(serde_json::Value::Array(vec![]));
                let rows = match value {
                    serde_json::Value::Array(rows) => rows,
                    serde_json::Value::Object(_) => vec![value],
                    _ => Vec::new(),
                };
                for row in rows {
                    devices.push(CameraDevice {
                        name: row
                            .get("FriendlyName")
                            .and_then(|value| value.as_str())
                            .unwrap_or("Unnamed camera")
                            .to_string(),
                        device_id: row
                            .get("InstanceId")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string(),
                        status: row
                            .get("Status")
                            .and_then(|value| value.as_str())
                            .unwrap_or("Unknown")
                            .to_string(),
                    });
                }
            }
        }
        Ok::<_, String>(devices)
    })
    .await
    .map_err(|e| format!("Camera enumeration task failed: {e}"))??;
    Ok(CameraInfo {
        devices,
        capture_backend: "webview-getUserMedia".to_string(),
    })
}

/// Capture an image from a file path and return its base64-encoded content
/// for vision/OCR processing via the local model.
/// This is used when the user selects an image from the vault or takes a photo
/// on mobile and syncs it to the desktop.
#[tauri::command]
pub fn encode_image_for_vision(image_path: String) -> Result<String, String> {
    let path = PathBuf::from(&image_path);
    if !path.exists() {
        return Err(format!("Image file not found: {}", image_path));
    }

    let image_bytes = std::fs::read(&path).map_err(|e| format!("Failed to read image: {}", e))?;

    // Determine MIME type from extension
    let mime_type = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/png",
    };

    let base64_data = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

    // Return as data URI for direct use in vision requests
    Ok(format!("data:{};base64,{}", mime_type, base64_data))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Defect #19: a captured camera frame (data URL) must land on disk as a
    /// decodable image the vision pipeline can read, with an extension that
    /// matches its MIME type.
    #[tokio::test]
    async fn save_vision_snapshot_persists_a_readable_jpeg() {
        // 1x1 red JPEG (smallest valid baseline JPEG).
        let jpeg: &[u8] = &[
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xD9,
        ];
        let mut data_url = String::from("data:image/jpeg;base64,");
        data_url.push_str(&base64::engine::general_purpose::STANDARD.encode(jpeg));

        let path = save_vision_snapshot(data_url).await.expect("snapshot save");
        let saved = PathBuf::from(&path);
        assert!(saved.exists(), "snapshot file must exist at {path}");
        assert_eq!(saved.extension().and_then(|e| e.to_str()), Some("jpg"));
        assert!(
            saved.starts_with(std::env::temp_dir().join("unoone-vision")),
            "snapshots must stay in the host temp area, not the vault"
        );
        assert_eq!(std::fs::read(&saved).expect("read back"), jpeg);
        std::fs::remove_file(&saved).ok();
    }

    #[tokio::test]
    async fn save_vision_snapshot_rejects_non_image_and_garbage() {
        assert!(save_vision_snapshot("not a data url".to_string()).await.is_err());
        assert!(
            save_vision_snapshot("data:image/png;base64,!!!not-base64!!!".to_string()).await.is_err()
        );
    }

    /// Defect #20: the screen-reader describe path must persist its capture as
    /// a PNG artifact in the same host-temp vision directory, and must never
    /// accept an empty payload.
    #[test]
    fn write_vision_artifact_persists_screen_png_and_rejects_empty() {
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        ];
        let path = write_vision_artifact("screen", "png", png).expect("artifact save");
        let saved = PathBuf::from(&path);
        assert!(saved.exists(), "screen capture must exist at {path}");
        assert_eq!(saved.extension().and_then(|e| e.to_str()), Some("png"));
        assert!(
            saved.starts_with(std::env::temp_dir().join("unoone-vision")),
            "screen captures must stay in the host temp area, not the vault"
        );
        assert!(
            saved
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("screen-"))
                .unwrap_or(false),
            "screen captures must be distinguishable from camera snapshots"
        );
        assert_eq!(std::fs::read(&saved).expect("read back"), png);
        std::fs::remove_file(&saved).ok();

        assert!(write_vision_artifact("screen", "png", &[]).is_err());
    }
}
