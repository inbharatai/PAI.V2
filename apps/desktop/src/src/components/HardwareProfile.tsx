import { useState, useEffect } from 'react';
import { tauriApi, type HardwareProfile as HWProfile } from '../lib/tauri';

export function HardwareProfile() {
  const [profile, setProfile] = useState<HWProfile | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');

  useEffect(() => {
    tauriApi.getHardwareProfile()
      .then(setProfile)
      .catch(e => {
        setError(`Failed to load hardware profile: ${e instanceof Error ? e.message : String(e)}`);
      })
      .finally(() => setLoading(false));
  }, []);

  if (loading) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', padding: '48px' }}>
        <span className="spinner" />
      </div>
    );
  }

  if (!profile) {
    return (
      <div className="empty-state">
        <h3>Hardware profile unavailable</h3>
        <p>{error || 'Could not detect hardware information.'}</p>
      </div>
    );
  }

  return (
    <div>
      <div className="main-header">
        <h2>Hardware Profile</h2>
      </div>

      <div className="main-body">
        <div className="hw-profile">
          <div className="hw-profile-card">
            <h4>Operating System</h4>
            <div className="value">{profile.os_name} {profile.os_version}</div>
          </div>

          <div className="hw-profile-card">
            <h4>CPU</h4>
            <div className="value">{profile.cpu_count} cores</div>
            <div className="detail">{profile.cpu_speed_ghz > 0 ? `${profile.cpu_speed_ghz} GHz` : 'Speed not detected'}</div>
          </div>

          <div className="hw-profile-card">
            <h4>Total RAM</h4>
            <div className="value">{profile.total_ram_gb.toFixed(1)}<span style={{ fontSize: '12px', color: 'var(--text-secondary)' }}> GB</span></div>
          </div>

          <div className="hw-profile-card">
            <h4>Available RAM</h4>
            <div className="value">{profile.available_ram_gb.toFixed(1)}<span style={{ fontSize: '12px', color: 'var(--text-secondary)' }}> GB</span></div>
          </div>

          <div className="hw-profile-card">
            <h4>GPU</h4>
            <div className="value">{profile.gpu_name || 'Not detected'}</div>
            <div className="detail">
              {profile.gpu_vram_gb > 0 ? `${profile.gpu_vram_gb} GB VRAM` : 'VRAM not detected'}
            </div>
          </div>

          <div className="hw-profile-card">
            <h4>USB Speed</h4>
            <div className="value">{profile.usb_speed || 'Unknown'}</div>
          </div>
        </div>

        {/* Acceleration support */}
        <div style={{ marginTop: '24px' }}>
          <h3 style={{ fontSize: '14px', fontWeight: 600, marginBottom: '12px', color: 'var(--text-secondary)' }}>
            Model Acceleration
          </h3>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(180px, 1fr))', gap: '12px' }}>
            <div className="hw-profile-card">
              <h4>CUDA (NVIDIA)</h4>
              <span className={`hw-badge ${profile.has_cuda ? 'available' : 'unavailable'}`}>
                {profile.has_cuda ? 'Available' : 'Not Available'}
              </span>
            </div>
            <div className="hw-profile-card">
              <h4>Metal (Apple)</h4>
              <span className={`hw-badge ${profile.has_metal ? 'available' : 'unavailable'}`}>
                {profile.has_metal ? 'Available' : 'Not Available'}
              </span>
            </div>
            <div className="hw-profile-card">
              <h4>Vulkan</h4>
              <span className={`hw-badge ${profile.has_vulkan ? 'available' : 'unavailable'}`}>
                {profile.has_vulkan ? 'Available' : 'Not Available'}
              </span>
            </div>
          </div>
        </div>

        {/* Model recommendation */}
        <div style={{ marginTop: '24px', padding: '16px', background: 'var(--accent-bg)', border: '1px solid var(--accent-border)', borderRadius: 'var(--radius-md)' }}>
          <h4 style={{ fontSize: '13px', fontWeight: 600, color: 'var(--accent)', marginBottom: '8px' }}>
            📋 Model Recommendation
          </h4>
          <p style={{ fontSize: '13px', color: 'var(--text-secondary)', lineHeight: 1.6 }}>
            {profile.has_cuda ? (
              <>CUDA detected — <strong>Gemma 4 12B Q4_K_M GGUF</strong> will run with GPU acceleration via llama.cpp CUDA backend. Expected ~30 tokens/sec on your hardware.</>
            ) : profile.has_metal ? (
              <>Metal detected — <strong>Gemma 4 12B Q4_K_M GGUF</strong> will run with Metal GPU acceleration via llama.cpp Metal backend.</>
            ) : profile.has_vulkan ? (
              <>Vulkan detected — <strong>Gemma 4 12B Q4_K_M GGUF</strong> will run with Vulkan GPU acceleration. Performance may vary.</>
            ) : (
              <>No GPU acceleration detected — <strong>Gemma 4 12B Q4_K_M GGUF</strong> will run on CPU only. Consider a smaller quantization for better performance, or install CUDA/Vulkan drivers.</>
            )}
          </p>
        </div>
      </div>
    </div>
  );
}