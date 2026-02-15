use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Per-region content expectations used during OCR result scoring.
///
/// All fields are optional — unset fields impose no constraint.
/// Used by `ocr::validation_score` to penalise candidates that don't fit.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegionExpectation {
    /// Whether the region is expected to contain a parseable number.
    #[serde(default)]
    pub numeric: bool,
    /// Minimum acceptable value (inclusive).
    pub min: Option<f64>,
    /// Maximum acceptable value (inclusive).
    pub max: Option<f64>,
    /// Expected number of digits after the decimal point (0 = integer).
    pub decimal_places: Option<u32>,
    /// Expected total digit count (e.g. 4 for "37.5°" → 3 digits).
    pub total_digits: Option<u32>,
    /// Maximum allowed absolute change from the previous accepted value.
    pub max_deviation: Option<f64>,
}

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
    /// Per-region-name content expectations.  Absent from old configs → empty map.
    #[serde(default)]
    pub expectations: HashMap<String, RegionExpectation>,
}

impl RegionConfig {
    /// Sort keyframes ascending by timestamp.
    /// Call this once after construction/deserialization so `get_regions_at`
    /// can skip the per-call clone + sort.
    pub fn sort_keyframes(&mut self) {
        self.keyframes.sort_by(|a, b| {
            a.timestamp
                .partial_cmp(&b.timestamp)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Linearly interpolate region positions at the given timestamp.
    /// **Requires keyframes to be sorted** — call `sort_keyframes()` first.
    ///
    /// Regions that exist in both surrounding keyframes are interpolated.
    /// Regions that exist only in the earlier keyframe keep that position.
    /// Regions that exist only in the later keyframe appear at their position.
    pub fn get_regions_at(&self, ts: f64) -> Vec<Region> {
        let kfs = &self.keyframes;
        if kfs.is_empty() {
            return vec![];
        }

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
                return interpolate_keyframes(a, b, t);
            }
        }

        vec![]
    }
}

/// Interpolate all regions between two keyframes.
/// Regions present in both are lerped; regions only in `a` keep `a`'s position;
/// regions only in `b` keep `b`'s position.
fn interpolate_keyframes(a: &Keyframe, b: &Keyframe, t: f64) -> Vec<Region> {
    use std::collections::HashMap;

    let a_map: HashMap<&str, &Region> = a.regions.iter().map(|r| (r.name.as_str(), r)).collect();
    let b_map: HashMap<&str, &Region> = b.regions.iter().map(|r| (r.name.as_str(), r)).collect();

    let mut result: Vec<Region> = a
        .regions
        .iter()
        .map(|ra| {
            let rb = b_map.get(ra.name.as_str()).copied().unwrap_or(ra);
            lerp_region(ra, rb, t)
        })
        .collect();

    // Include regions that only exist in `b` (added at this keyframe)
    for rb in &b.regions {
        if !a_map.contains_key(rb.name.as_str()) {
            result.push(rb.clone());
        }
    }

    result
}

fn lerp_region(a: &Region, b: &Region, t: f64) -> Region {
    Region {
        name: a.name.clone(),
        x: lerp_i32(a.x, b.x, t),
        y: lerp_i32(a.y, b.y, t),
        width: lerp_i32(a.width, b.width, t),
        height: lerp_i32(a.height, b.height, t),
    }
}

fn lerp_i32(a: i32, b: i32, t: f64) -> i32 {
    (a as f64 + (b as f64 - a as f64) * t).round() as i32
}

#[tauri::command]
pub fn load_config(path: String) -> Result<RegionConfig, String> {
    let text = fs::read_to_string(&path).map_err(|e| format!("Cannot read {path}: {e}"))?;
    let mut cfg: RegionConfig =
        serde_json::from_str(&text).map_err(|e| format!("Parse error in {path}: {e}"))?;
    cfg.sort_keyframes();
    Ok(cfg)
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
