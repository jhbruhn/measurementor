use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Region {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyframe {
    pub timestamp: f64,
    pub regions: Vec<Region>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionConfig {
    pub video_path: String,
    pub keyframes: Vec<Keyframe>,
}

impl RegionConfig {
    /// Linearly interpolate region positions at the given timestamp.
    /// Mirrors the Python RegionConfig.get_regions_at() behaviour.
    pub fn get_regions_at(&self, ts: f64) -> Vec<Region> {
        let mut kfs = self.keyframes.clone();
        if kfs.is_empty() {
            return vec![];
        }
        kfs.sort_by(|a, b| a.timestamp.partial_cmp(&b.timestamp).unwrap());

        if ts <= kfs[0].timestamp {
            return kfs[0].regions.clone();
        }
        if ts >= kfs[kfs.len() - 1].timestamp {
            return kfs[kfs.len() - 1].regions.clone();
        }

        for i in 0..kfs.len() - 1 {
            let a = &kfs[i];
            let b = &kfs[i + 1];
            if a.timestamp <= ts && ts <= b.timestamp {
                let t = (ts - a.timestamp) / (b.timestamp - a.timestamp);
                let b_map: std::collections::HashMap<&str, &Region> =
                    b.regions.iter().map(|r| (r.name.as_str(), r)).collect();
                return a
                    .regions
                    .iter()
                    .map(|ra| {
                        let rb = b_map.get(ra.name.as_str()).copied().unwrap_or(ra);
                        Region {
                            name: ra.name.clone(),
                            x:      lerp_i32(ra.x,      rb.x,      t),
                            y:      lerp_i32(ra.y,      rb.y,      t),
                            width:  lerp_i32(ra.width,  rb.width,  t),
                            height: lerp_i32(ra.height, rb.height, t),
                        }
                    })
                    .collect();
            }
        }
        vec![]
    }
}

fn lerp_i32(a: i32, b: i32, t: f64) -> i32 {
    (a as f64 + (b as f64 - a as f64) * t).round() as i32
}

#[tauri::command]
pub fn load_config(path: String) -> Result<RegionConfig, String> {
    let text = fs::read_to_string(&path).map_err(|e| format!("Cannot read {path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Parse error in {path}: {e}"))
}

#[tauri::command]
pub fn save_config(path: String, config: RegionConfig) -> Result<(), String> {
    if let Some(parent) = Path::new(&path).parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Cannot create dirs: {e}"))?;
    }
    let text =
        serde_json::to_string_pretty(&config).map_err(|e| format!("Serialise error: {e}"))?;
    fs::write(&path, text).map_err(|e| format!("Cannot write {path}: {e}"))
}
