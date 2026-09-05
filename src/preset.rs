use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// How a probe target evaluates fitness from the measured value
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ProbeKind {
    /// Target wants value close to `want`. Drops to 0 at `soft`.
    Closeness,
    /// Target wants value >= `want`. Logarithmic or linear drop to 0 at `soft`.
    Greater,
    /// Target wants value <= `want`. Drops to 0 at `soft`.
    Lesser,
}

/// Physical measurement probe types
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ProbeType {
    Gain,
    Zin,
    Zout,
    DcOffset,
    Bom,
}

/// Test conditions applied during measurement
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TestCondition {
    pub freq: f64,
    pub vin: f64,
    pub r_load: f64,
}

impl Default for TestCondition {
    fn default() -> Self {
        Self {
            freq: 1000.0,
            vin: 0.1,
            r_load: 10000.0,
        }
    }
}

/// Individual probe target definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeTarget {
    pub name: String,
    pub probe_type: ProbeType,
    pub condition: TestCondition,
    pub kind: ProbeKind,
    pub want: f64,
    pub soft: f64,
    pub weight: f64,
}

impl ProbeTarget {
    /// Evaluate a measured physical value into a normalized [0.0, 1.0] graded score
    pub fn score(&self, val: f64) -> f64 {
        if val.is_nan() || val.is_infinite() {
            return 0.0;
        }

        match self.kind {
            ProbeKind::Closeness => {
                let diff = (val - self.want).abs();
                let max_diff = (self.soft - self.want).abs().max(1e-9);
                (1.0 - (diff / max_diff)).clamp(0.0, 1.0)
            }
            ProbeKind::Greater => {
                if val >= self.want {
                    return 1.0;
                }
                if val <= self.soft {
                    return 0.0;
                }
                // Logarithmic scaling for large dynamic ranges (e.g. impedance)
                if self.want > 0.0 && self.soft > 0.0 {
                    let log_val = val.max(1e-9).log10();
                    let log_want = self.want.log10();
                    let log_soft = self.soft.log10();
                    if (log_want - log_soft).abs() > 1e-9 {
                        return ((log_val - log_soft) / (log_want - log_soft)).clamp(0.0, 1.0);
                    }
                }
                ((val - self.soft) / (self.want - self.soft)).clamp(0.0, 1.0)
            }
            ProbeKind::Lesser => {
                if val <= self.want {
                    return 1.0;
                }
                if val >= self.soft {
                    return 0.0;
                }
                ((self.soft - val) / (self.soft - self.want)).clamp(0.0, 1.0)
            }
        }
    }
}

/// Objective vector returned by fitness evaluation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ObjectiveVector {
    pub names: Vec<String>,
    pub values: Vec<f64>,
    pub scores: Vec<f64>,
    pub weights: Vec<f64>,
}

impl ObjectiveVector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, name: &str, val: f64, score: f64, weight: f64) {
        self.names.push(name.to_string());
        self.values.push(val);
        self.scores.push(score);
        self.weights.push(weight);
    }

    /// Weighted average scalarization for current evolutionary selection
    pub fn scalarized(&self) -> f64 {
        let total_weight: f64 = self.weights.iter().sum();
        if total_weight <= 0.0 {
            return 0.0;
        }
        let weighted_sum: f64 = self
            .scores
            .iter()
            .zip(&self.weights)
            .map(|(s, w)| s * w)
            .sum();
        weighted_sum / total_weight
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        for i in 0..self.names.len() {
            parts.push(format!(
                "{}={:.2} (sc={:.2})",
                self.names[i], self.values[i], self.scores[i]
            ));
        }
        parts.join(" | ")
    }
}

fn default_true() -> bool {
    true
}

/// Complete preset specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub description: String,
    pub feasibility_max_dc: f64,
    pub feasibility_rail_margin: f64,
    #[serde(default = "default_true")]
    pub allow_opamps: bool,
    pub probes: Vec<ProbeTarget>,
}

impl Preset {
    /// Load preset by name (e.g. "buffer") or file path ("presets/buffer.toml")
    pub fn load_or_builtin(name_or_path: &str) -> Result<Self, String> {
        let path = Path::new(name_or_path);
        if path.exists() {
            let content = fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {}", name_or_path, e))?;
            return toml::from_str(&content).map_err(|e| format!("Failed to parse TOML preset: {}", e));
        }

        let default_dir_path = Path::new("presets").join(format!("{}.toml", name_or_path));
        if default_dir_path.exists() {
            let content = fs::read_to_string(&default_dir_path)
                .map_err(|e| format!("Failed to read {:?}: {}", default_dir_path, e))?;
            return toml::from_str(&content).map_err(|e| format!("Failed to parse TOML preset: {}", e));
        }

        match name_or_path.to_lowercase().as_str() {
            "buffer" | "highz_buffer" => Ok(Self::buffer_default()),
            other => Err(format!("Unknown preset: '{}'. Available built-in: 'buffer'", other)),
        }
    }

    /// Standard High-Z Unity-Gain Buffer preset (Discrete BJT / Active)
    pub fn buffer_default() -> Self {
        Self {
            name: "buffer".to_string(),
            description: "High-Z Unity-Gain Buffer with 10k Load driving (Discrete Hunting)".to_string(),
            feasibility_max_dc: 2.0,
            feasibility_rail_margin: 0.5,
            allow_opamps: false,
            probes: vec![
                ProbeTarget {
                    name: "Gain@1kHz".to_string(),
                    probe_type: ProbeType::Gain,
                    condition: TestCondition {
                        freq: 1000.0,
                        vin: 0.1,
                        r_load: 10000.0,
                    },
                    kind: ProbeKind::Closeness,
                    want: 1.0,
                    soft: 0.5,
                    weight: 3.0,
                },
                ProbeTarget {
                    name: "Zin@1kHz".to_string(),
                    probe_type: ProbeType::Zin,
                    condition: TestCondition {
                        freq: 1000.0,
                        vin: 0.1,
                        r_load: 10000.0,
                    },
                    kind: ProbeKind::Greater,
                    want: 1_000_000.0, // 1 MegOhm (Bootstrap target, well below 7.23M stray ceiling)
                    soft: 10_000.0,    // 10 kOhm
                    weight: 3.5,
                },
                ProbeTarget {
                    name: "DcOffset".to_string(),
                    probe_type: ProbeType::DcOffset,
                    condition: TestCondition::default(),
                    kind: ProbeKind::Lesser,
                    want: 0.02, // 20 mV
                    soft: 0.80, // 800 mV
                    weight: 1.5,
                },
                ProbeTarget {
                    name: "BOM_Count".to_string(),
                    probe_type: ProbeType::Bom,
                    condition: TestCondition::default(),
                    kind: ProbeKind::Lesser,
                    want: 4.0,  // 4-5 components minimum for discrete follower
                    soft: 10.0, // 10 components
                    weight: 1.0,
                },
            ],
        }
    }

    /// Compute deterministic 16-character hex checksum of preset definition
    pub fn checksum(&self) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut s = DefaultHasher::new();
        if let Ok(json) = serde_json::to_string(self) {
            json.hash(&mut s);
            format!("{:016x}", s.finish())
        } else {
            "unknown".to_string()
        }
    }
}
