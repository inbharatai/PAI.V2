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

/// Camera info — enumerates available video capture devices.
/// Actual frame capture uses the frontend WebView + getUserMedia API.
#[tauri::command]
pub fn get_camera_info() -> Result<CameraInfo, String> {
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
            let value: serde_json::Value =
                serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Array(vec![]));
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
