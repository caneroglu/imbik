use serde::{Deserialize, Serialize};
use std::fs;

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
    Noise,      // Input-referred noise density @ target frequency in nV/√Hz
    NoiseFig,   // Noise Figure in dB
    NoiseTotal, // Integrated RMS input noise across audio band in µV RMS
}

fn default_r_source() -> f64 {
    600.0
}

/// Test conditions applied during measurement
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TestCondition {
    pub freq: f64,
    pub vin: f64,
    pub r_load: f64,
    #[serde(default = "default_r_source")]
    pub r_source: f64,
}

impl Default for TestCondition {
    fn default() -> Self {
        Self {
            freq: 1000.0,
            vin: 0.1,
            r_load: 10000.0,
            r_source: 600.0,
        }
    }
}

fn default_weight() -> f64 {
    1.0
}

/// Individual probe target definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeTarget {
    pub name: String,
    pub probe_type: ProbeType,
    #[serde(default)]
    pub condition: TestCondition,
    pub kind: ProbeKind,
    pub want: f64,
    pub soft: f64,
    #[serde(default = "default_weight")]
    pub weight: f64,
    #[serde(default, alias = "require")]
    pub is_required: bool,
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
                if (self.probe_type == ProbeType::Zin || self.probe_type == ProbeType::Zout)
                    && self.want > 0.0
                    && self.soft > 0.0
                {
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
                // Logarithmic scaling for large dynamic ranges (e.g. impedance, noise)
                if (self.probe_type == ProbeType::Zin
                    || self.probe_type == ProbeType::Zout
                    || self.probe_type == ProbeType::Noise
                    || self.probe_type == ProbeType::NoiseTotal)
                    && self.want > 0.0
                    && self.soft > 0.0
                {
                    let log_val = val.max(1e-9).log10();
                    let log_want = self.want.log10();
                    let log_soft = self.soft.log10();
                    if (log_soft - log_want).abs() > 1e-9 {
                        return ((log_soft - log_val) / (log_soft - log_want)).clamp(0.0, 1.0);
                    }
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
    #[serde(default)]
    pub is_required: Vec<bool>,
}

impl ObjectiveVector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, name: &str, val: f64, score: f64, weight: f64, is_required: bool) {
        self.names.push(name.to_string());
        self.values.push(val);
        self.scores.push(score);
        self.weights.push(weight);
        self.is_required.push(is_required);
    }

    /// Weighted average scalarization with hard requirement enforcement
    pub fn scalarized(&self) -> f64 {
        // Hard requirement check: if any required probe scores 0.0, the candidate fails immediately
        for (i, &req) in self.is_required.iter().enumerate() {
            if req && self.scores.get(i).copied().unwrap_or(0.0) <= 0.0 {
                return 0.0;
            }
        }

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
            let prefix = if self.is_required.get(i).copied().unwrap_or(false) { "*" } else { "" };
            parts.push(format!(
                "{}{}={:.2} (sc={:.2})",
                prefix, self.names[i], self.values[i], self.scores[i]
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
    /// Generate unified ConstraintLimits representing this preset's feasibility rules
    pub fn constraint_limits(&self) -> crate::constraints::ConstraintLimits {
        crate::constraints::ConstraintLimits::from(self)
    }

    /// Load preset by name (e.g. "buffer") or file path ("presets/buffer.toml")
    pub fn load_or_builtin(name_or_path: &str) -> Result<Self, String> {
        let mut candidates = Vec::new();
        candidates.push(std::path::PathBuf::from(name_or_path));
        candidates.push(std::path::PathBuf::from("presets").join(format!("{}.toml", name_or_path)));
        candidates.push(std::path::PathBuf::from("../presets").join(format!("{}.toml", name_or_path)));
        candidates.push(std::path::PathBuf::from("../../presets").join(format!("{}.toml", name_or_path)));

        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                candidates.push(exe_dir.join(name_or_path));
                candidates.push(exe_dir.join("presets").join(format!("{}.toml", name_or_path)));
                candidates.push(exe_dir.join("../presets").join(format!("{}.toml", name_or_path)));
                candidates.push(exe_dir.join("../../presets").join(format!("{}.toml", name_or_path)));
            }
        }

        for p in &candidates {
            if p.exists() && p.is_file() {
                let content = fs::read_to_string(p)
                    .map_err(|e| format!("Failed to read {:?}: {}", p, e))?;
                return toml::from_str(&content)
                    .map_err(|e| format!("Failed to parse TOML preset {:?}: {}", p, e));
            }
        }

        match name_or_path.to_lowercase().as_str() {
            "buffer" | "highz_buffer" => Ok(Self::buffer_default()),
            "low_noise_preamp" | "low_noise" | "preamp" => Ok(Self::low_noise_preamp_default()),
            other => Err(format!("Unknown preset: '{}'. Available built-in: 'buffer', 'low_noise_preamp'", other)),
        }
    }

    /// List all available built-in and discovered local presets
    pub fn list_available() -> Vec<(String, String, usize)> {
        let mut list = vec![
            (
                "buffer".to_string(),
                "High-Z Input Buffer / Follower (Zin >= 500k, Gain ~ 1.0, Low Offset)".to_string(),
                Self::buffer_default().probes.len(),
            ),
            (
                "low_noise_preamp".to_string(),
                "Ultra Low-Noise Discrete Preamplifier (Zin >= 50k, Gain >= 10, Noise < 3.5 nV/√Hz)".to_string(),
                Self::low_noise_preamp_default().probes.len(),
            ),
        ];

        // Search presets/ directories
        let mut dirs = vec![
            std::path::PathBuf::from("presets"),
            std::path::PathBuf::from("../presets"),
        ];
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(parent) = exe_path.parent() {
                dirs.push(parent.join("presets"));
            }
        }

        for d in dirs {
            if let Ok(entries) = fs::read_dir(d) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("toml") {
                        if let Ok(content) = fs::read_to_string(&path) {
                            if let Ok(preset) = toml::from_str::<Preset>(&content) {
                                if !list.iter().any(|(n, _, _)| n == &preset.name) {
                                    list.push((preset.name, preset.description, preset.probes.len()));
                                }
                            }
                        }
                    }
                }
            }
        }

        list
    }

    /// Return documented TOML configuration template for defining custom missions & probe targets
    pub fn template_toml() -> &'static str {
        concat!(
            "# ==============================================================================\n",
            "#   İmbik Mission Preset & Fitness Probe Definition Template\n",
            "#   Save this file as 'presets/<mission_name>.toml'\n",
            "#   Run with: imbik evolve --preset presets/<mission_name>.toml\n",
            "# ==============================================================================\n\n",
            "name = \"custom_audio_preamp\"\n",
            "description = \"Ultra Low-Noise Discrete Microphone Preamplifier (Gain >= 20, Noise < 2.5 nV/√Hz)\"\n\n",
            "# ------------------------------------------------------------------------------\n",
            "# Feasibility Gate Constraints:\n",
            "# Any candidate circuit violating these basic operating rules is rejected\n",
            "# before wasting time on full AC/Noise frequency sweeps.\n",
            "# ------------------------------------------------------------------------------\n",
            "feasibility_max_dc = 2.0        # Max allowable DC offset at output (V)\n",
            "feasibility_rail_margin = 0.5   # Headroom from power rails (V). Rejects railed/saturated nodes.\n",
            "allow_opamps = false            # Allow Op-Amps? (false = purely discrete BJT/diodes/passives)\n\n",
            "# ------------------------------------------------------------------------------\n",
            "# Target Fitness Probes (Multi-Objective Optimization Vector):\n",
            "# Each probe measures a physical property under specific test conditions.\n",
            "#\n",
            "# Available probe_type:\n",
            "#   - \"Gain\"       : AC voltage gain magnitude (|Vout / Vin|)\n",
            "#   - \"Noise\"      : Input-referred noise density in nV/√Hz at target frequency\n",
            "#   - \"NoiseFig\"   : Noise figure in dB (referenced to r_source Johnson noise)\n",
            "#   - \"NoiseTotal\" : Integrated audio-band (20Hz - 20kHz) total noise in µV RMS\n",
            "#   - \"Zin\"        : Input impedance in Ohms (Ω)\n",
            "#   - \"Zout\"       : Output impedance in Ohms (Ω)\n",
            "#   - \"DcOffset\"   : Output DC offset voltage (|Vout_dc|) in Volts\n",
            "#   - \"Bom\"        : Total component count (penalizes excessive parts)\n",
            "#\n",
            "# Available kind (Scoring Functions):\n",
            "#   - \"Closeness\"  : Score = 1.0 when value == 'want'. Linearly drops to 0.0 at 'soft'.\n",
            "#   - \"Greater\"    : Score = 1.0 when value >= 'want'. Drops (logarithmic for Zin/gain) to 0.0 at 'soft'.\n",
            "#   - \"Lesser\"     : Score = 1.0 when value <= 'want'. Drops (logarithmic for noise) to 0.0 at 'soft'.\n",
            "#\n",
            "# Attributes:\n",
            "#   - want        : Ideal target value\n",
            "#   - soft        : Unacceptable / dropoff threshold value\n",
            "#   - weight      : Relative importance in multi-objective fitness (e.g. 1.0 - 5.0)\n",
            "#   - is_required : If true, candidate fails immediately (fitness = 0) if score <= 0.0\n",
            "# ------------------------------------------------------------------------------\n\n",
            "# Probe 1: Target AC Voltage Gain of 20x (+26 dB) at 1 kHz\n",
            "[[probes]]\n",
            "name = \"Gain@1kHz\"\n",
            "probe_type = \"Gain\"\n",
            "kind = \"Greater\"\n",
            "want = 20.0                     # Ideal gain: 20x\n",
            "soft = 1.0                      # Below 1x gain scores 0.0\n",
            "weight = 4.0                    # High priority\n",
            "is_required = true              # Hard requirement: must amplify!\n",
            "[probes.condition]\n",
            "freq = 1000.0                   # Test frequency: 1 kHz\n",
            "vin = 0.005                     # Input signal: 5 mV AC\n",
            "r_load = 10000.0                # Load resistance: 10 kΩ\n",
            "r_source = 200.0                # Source impedance: 200 Ω\n\n",
            "# Probe 2: Input-Referred Spot Noise Voltage at 1 kHz\n",
            "[[probes]]\n",
            "name = \"Noise@1kHz\"\n",
            "probe_type = \"Noise\"\n",
            "kind = \"Lesser\"\n",
            "want = 2.0                      # Ideal target: 2.0 nV/√Hz\n",
            "soft = 25.0                     # Above 25 nV/√Hz scores 0.0\n",
            "weight = 5.0                    # Highest optimization priority\n",
            "is_required = false\n",
            "[probes.condition]\n",
            "freq = 1000.0\n",
            "vin = 0.0                       # 0V input for pure noise spectrum measurement\n",
            "r_load = 10000.0\n",
            "r_source = 200.0\n\n",
            "# Probe 3: Input Impedance at 1 kHz\n",
            "[[probes]]\n",
            "name = \"Zin@1kHz\"\n",
            "probe_type = \"Zin\"\n",
            "kind = \"Greater\"\n",
            "want = 50000.0                  # Want Zin >= 50 kΩ (bridging impedance for 200Ω mic)\n",
            "soft = 2000.0                   # Below 2 kΩ severely loads source\n",
            "weight = 2.0\n",
            "is_required = false\n",
            "[probes.condition]\n",
            "freq = 1000.0\n",
            "vin = 0.005\n",
            "r_load = 10000.0\n",
            "r_source = 200.0\n\n",
            "# Probe 4: DC Output Offset (Zero-drop DC centering)\n",
            "[[probes]]\n",
            "name = \"DcOffset\"\n",
            "probe_type = \"DcOffset\"\n",
            "kind = \"Lesser\"\n",
            "want = 0.05                     # Centered within 50 mV\n",
            "soft = 1.50                     # Above 1.5V offset is unacceptable\n",
            "weight = 1.5\n",
            "is_required = true\n",
            "[probes.condition]\n",
            "freq = 1000.0\n",
            "vin = 0.0\n",
            "r_load = 10000.0\n",
            "r_source = 200.0\n\n",
            "# Probe 5: Parsimony / Bill of Materials (BOM) Count\n",
            "[[probes]]\n",
            "name = \"BOM_Count\"\n",
            "probe_type = \"Bom\"\n",
            "kind = \"Lesser\"\n",
            "want = 4.0                      # Reward elegant 4-component circuits\n",
            "soft = 10.0                     # Penalize bloated 10+ component circuits\n",
            "weight = 1.0\n",
            "is_required = false\n",
            "[probes.condition]\n",
            "freq = 1000.0\n",
            "vin = 0.0\n",
            "r_load = 10000.0\n",
            "r_source = 200.0\n"
        )
    }

    /// Standard Low-Noise Preamplifier preset (Discrete BJT, Zin >= 50k, Gain >= 10, Noise <= 3.5 nV/√Hz @ 1kHz)
    pub fn low_noise_preamp_default() -> Self {
        Self {
            name: "low_noise_preamp".to_string(),
            description: "Ultra Low-Noise Preamplifier with < 3.5 nV/√Hz floor (Discrete Hunting)".to_string(),
            feasibility_max_dc: 2.5,
            feasibility_rail_margin: 0.5,
            allow_opamps: false,
            probes: vec![
                ProbeTarget {
                    name: "Gain@1kHz".to_string(),
                    probe_type: ProbeType::Gain,
                    condition: TestCondition {
                        freq: 1000.0,
                        vin: 0.01,
                        r_load: 10000.0,
                        r_source: 600.0,
                    },
                    kind: ProbeKind::Greater,
                    want: 10.0,
                    soft: 0.1,
                    weight: 3.5,
                    is_required: true,
                },
                ProbeTarget {
                    name: "Noise@1kHz".to_string(),
                    probe_type: ProbeType::Noise,
                    condition: TestCondition {
                        freq: 1000.0,
                        vin: 0.0,
                        r_load: 10000.0,
                        r_source: 600.0,
                    },
                    kind: ProbeKind::Lesser,
                    want: 3.5,
                    soft: 100.0,
                    weight: 4.0,
                    is_required: false,
                },
                ProbeTarget {
                    name: "Zin@1kHz".to_string(),
                    probe_type: ProbeType::Zin,
                    condition: TestCondition {
                        freq: 1000.0,
                        vin: 0.01,
                        r_load: 10000.0,
                        r_source: 600.0,
                    },
                    kind: ProbeKind::Greater,
                    want: 50_000.0,
                    soft: 2_000.0,
                    weight: 2.0,
                    is_required: false,
                },
                ProbeTarget {
                    name: "DcOffset".to_string(),
                    probe_type: ProbeType::DcOffset,
                    condition: TestCondition {
                        freq: 1000.0,
                        vin: 0.0,
                        r_load: 10000.0,
                        r_source: 600.0,
                    },
                    kind: ProbeKind::Lesser,
                    want: 0.05,
                    soft: 1.50,
                    weight: 1.5,
                    is_required: true,
                },
                ProbeTarget {
                    name: "BOM_Count".to_string(),
                    probe_type: ProbeType::Bom,
                    condition: TestCondition {
                        freq: 1000.0,
                        vin: 0.0,
                        r_load: 10000.0,
                        r_source: 600.0,
                    },
                    kind: ProbeKind::Lesser,
                    want: 4.0,
                    soft: 12.0,
                    weight: 1.0,
                    is_required: false,
                },
            ],
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
                        r_source: 600.0,
                    },
                    kind: ProbeKind::Closeness,
                    want: 1.0,
                    soft: 0.5,
                    weight: 3.0,
                    is_required: true,
                },
                ProbeTarget {
                    name: "Zin@1kHz".to_string(),
                    probe_type: ProbeType::Zin,
                    condition: TestCondition {
                        freq: 1000.0,
                        vin: 0.1,
                        r_load: 10000.0,
                        r_source: 600.0,
                    },
                    kind: ProbeKind::Greater,
                    want: 1_000_000.0, // 1 MegOhm (Bootstrap target, well below 7.23M stray ceiling)
                    soft: 10_000.0,    // 10 kOhm
                    weight: 3.5,
                    is_required: false,
                },
                ProbeTarget {
                    name: "DcOffset".to_string(),
                    probe_type: ProbeType::DcOffset,
                    condition: TestCondition::default(),
                    kind: ProbeKind::Lesser,
                    want: 0.02, // 20 mV
                    soft: 0.80, // 800 mV
                    weight: 1.5,
                    is_required: true,
                },
                ProbeTarget {
                    name: "BOM_Count".to_string(),
                    probe_type: ProbeType::Bom,
                    condition: TestCondition::default(),
                    kind: ProbeKind::Lesser,
                    want: 4.0,  // 4-5 components minimum for discrete follower
                    soft: 10.0, // 10 components
                    weight: 1.0,
                    is_required: false,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_toml_parsing_and_required_probes() {
        let preset = Preset::load_or_builtin("presets/buffer.toml").expect("Failed to load buffer.toml");
        assert_eq!(preset.name, "buffer");
        assert_eq!(preset.probes.len(), 4);

        // Verify required flags
        let gain_probe = preset.probes.iter().find(|p| p.name == "Gain@1kHz").expect("Gain probe missing");
        assert!(gain_probe.is_required, "Gain@1kHz must be required");

        let dc_probe = preset.probes.iter().find(|p| p.name == "DcOffset").expect("DcOffset probe missing");
        assert!(dc_probe.is_required, "DcOffset must be required");

        let zin_probe = preset.probes.iter().find(|p| p.name == "Zin@1kHz").expect("Zin probe missing");
        assert!(!zin_probe.is_required, "Zin@1kHz is an optimization target, not strict requirement");
    }

    #[test]
    fn test_required_probe_zero_drop() {
        let preset = Preset::buffer_default();
        let mut obj_vec = ObjectiveVector::new();

        // Simulate: Gain=1.0 (score 1.0), Zin=100k (score 0.5), DcOffset=1.5V (score 0.0, required!), BOM=5 (score 0.8)
        let p0 = &preset.probes[0];
        obj_vec.add(&p0.name, 1.0, p0.score(1.0), p0.weight, p0.is_required);
        let p1 = &preset.probes[1];
        obj_vec.add(&p1.name, 100_000.0, p1.score(100_000.0), p1.weight, p1.is_required);
        let p2 = &preset.probes[2];
        obj_vec.add(&p2.name, 1.5, p2.score(1.5), p2.weight, p2.is_required);
        let p3 = &preset.probes[3];
        obj_vec.add(&p3.name, 5.0, p3.score(5.0), p3.weight, p3.is_required);

        println!("Objective vector with failed required probe:\n  {}", obj_vec.summary());
        assert_eq!(obj_vec.scalarized(), 0.0, "Fitness MUST drop to 0.0 if any required probe fails");
    }

    #[test]
    fn test_sallen_key_10k_toml_evaluation() {
        let preset = Preset::load_or_builtin("sallen_key_10k").expect("Failed to load sallen_key_10k.toml");
        assert_eq!(preset.name, "sallen_key_10k");
        assert!(preset.allow_opamps);

        // Build textbook 10kHz Sallen-Key Lowpass (R1=11k, R2=11k, C1=2.2nF, C2=1.0nF, TL072)
        let mut sk = crate::circuit::Circuit::new();
        sk.add_component(crate::circuit::Component::new('R', 1, vec![crate::circuit::NODE_IN, 10], "11k").unwrap());
        sk.add_component(crate::circuit::Component::new('R', 2, vec![10, 20], "11k").unwrap());
        sk.add_component(crate::circuit::Component::new('C', 1, vec![10, crate::circuit::NODE_OUT], "2.2nF").unwrap());
        sk.add_component(crate::circuit::Component::new('C', 2, vec![20, crate::circuit::NODE_GND], "1.0nF").unwrap());
        sk.add_component(
            crate::circuit::Component::new('X', 1, vec![20, crate::circuit::NODE_OUT, crate::circuit::NODE_VCC, crate::circuit::NODE_VEE, crate::circuit::NODE_OUT], "TL072").unwrap(),
        );

        let timeout = std::time::Duration::from_secs(5);
        let obj_vec = crate::fitness::evaluate_preset(&sk, &preset, 0.0, timeout)
            .expect("Textbook Sallen-Key must pass feasibility gate and evaluate successfully");

        println!("Textbook Sallen-Key 10kHz Evaluation:\n  {}", obj_vec.summary());
        let score = obj_vec.scalarized();
        println!("Scalarized Score: {:.4}", score);
        assert!(score > 0.85, "Textbook Sallen-Key should achieve > 0.85 fitness, got {:.4}", score);
    }

    #[test]
    fn test_op_seed_evaluation() {
        let preset = Preset::load_or_builtin("sallen_key_10k").unwrap();
        let mut op_seed = crate::circuit::Circuit::new();
        op_seed.add_component(
            crate::circuit::Component::new('X', 1, vec![crate::circuit::NODE_IN, crate::circuit::NODE_OUT, crate::circuit::NODE_VCC, crate::circuit::NODE_VEE, crate::circuit::NODE_OUT], "TL072").unwrap(),
        );

        let timeout = std::time::Duration::from_secs(5);
        let obj_op = crate::fitness::evaluate_preset(&op_seed, &preset, 0.0, timeout)
            .expect("Op-amp follower must evaluate successfully");
        assert!(obj_op.scalarized() > 0.40, "Op-amp follower seed should score > 0.40 baseline");

        let mut pass_seed = crate::circuit::Circuit::new();
        pass_seed.add_component(crate::circuit::Component::new('R', 1, vec![crate::circuit::NODE_IN, crate::circuit::NODE_OUT], "10k").unwrap());
        let obj_pass = crate::fitness::evaluate_preset(&pass_seed, &preset, 0.0, timeout)
            .expect("Passive seed evaluates but should fail required passband probe");
        assert_eq!(obj_pass.scalarized(), 0.0, "Passive attenuator must drop to 0.0 on required passband gain");
    }

    #[test]
    fn test_textbook_gyrator_simulation() {
        let mut gyr = crate::circuit::Circuit::new();
        // 4-Component Textbook Single Op-Amp Gyrator:
        // RL = 100 Ohm between IN (1) and OUT (2)
        gyr.add_component(crate::circuit::Component::new('R', 1, vec![crate::circuit::NODE_IN, crate::circuit::NODE_OUT], "100").unwrap());
        // C = 100nF between IN (1) and internal node 10
        gyr.add_component(crate::circuit::Component::new('C', 1, vec![crate::circuit::NODE_IN, 10], "100nF").unwrap());
        // R = 100k between internal node 10 and GND (0)
        gyr.add_component(crate::circuit::Component::new('R', 2, vec![10, crate::circuit::NODE_GND], "100k").unwrap());
        // X1 TL072 follower: non-inv=10, inv=2, vcc=3, vee=4, out=2
        gyr.add_component(crate::circuit::Component::new('X', 1, vec![10, crate::circuit::NODE_OUT, crate::circuit::NODE_VCC, crate::circuit::NODE_VEE, crate::circuit::NODE_OUT], "TL072").unwrap());

        assert!(gyr.validate().is_ok(), "Textbook gyrator must pass circuit validation");

        // Run SPICE simulation on this netlist
        let netlist = gyr.to_netlist("Textbook Gyrator Active Inductor Test");
        let timeout = std::time::Duration::from_secs(5);
        let sim_res = crate::spice::run_simulation(&netlist, timeout).expect("Gyrator simulation failed");

        println!("=== TEXTBOOK GYRATOR SPICE SIMULATION ===");
        let v_out = sim_res.dc_nodes.get("2").copied().unwrap_or(0.0);
        println!("DC Operating Point Output: {:.4} V", v_out);
        // Test with Preset loaded from presets/gyrator.toml
        let gyr_preset = Preset::load_or_builtin("gyrator").expect("Failed to load presets/gyrator.toml");
        let obj = crate::fitness::evaluate_preset(&gyr, &gyr_preset, 0.0, timeout)
            .expect("Gyrator evaluation failed");
        println!("\n=== GYRATOR PRESET OBJECTIVES ===");
        println!("{}", obj.summary());
        let score = obj.scalarized();
        println!("Scalarized Fitness: {:.4}", score);
        assert!(score > 0.90, "Textbook gyrator must score > 0.90 on inductive Zin & gain probes, got {:.4}", score);
    }

    #[test]
    fn test_textbook_cap_multiplier_simulation() {
        let mut cm = crate::circuit::Circuit::new();
        // 4-Component Textbook Single Op-Amp Capacitance Multiplier:
        // R1 = 10k between IN (1) and internal node 10
        cm.add_component(crate::circuit::Component::new('R', 1, vec![crate::circuit::NODE_IN, 10], "10k").unwrap());
        // C1 = 100nF between internal node 10 and GND (0)
        cm.add_component(crate::circuit::Component::new('C', 1, vec![10, crate::circuit::NODE_GND], "100nF").unwrap());
        // RL = 100 Ohm between IN (1) and OUT (2)
        cm.add_component(crate::circuit::Component::new('R', 2, vec![crate::circuit::NODE_IN, crate::circuit::NODE_OUT], "100").unwrap());
        // X1 TL072 follower: non-inv=10, inv=2, vcc=3, vee=4, out=2
        cm.add_component(crate::circuit::Component::new('X', 1, vec![10, crate::circuit::NODE_OUT, crate::circuit::NODE_VCC, crate::circuit::NODE_VEE, crate::circuit::NODE_OUT], "TL072").unwrap());

        assert!(cm.validate().is_ok(), "Textbook cap multiplier must pass circuit validation");

        let timeout = std::time::Duration::from_secs(5);
        let cm_preset = Preset::load_or_builtin("cap_multiplier").expect("Failed to load presets/cap_multiplier.toml");
        let obj = crate::fitness::evaluate_preset(&cm, &cm_preset, 0.0, timeout)
            .expect("Cap multiplier evaluation failed");

        println!("\n=== CAPACITANCE MULTIPLIER PRESET OBJECTIVES ===");
        println!("{}", obj.summary());
        let score = obj.scalarized();
        println!("Scalarized Fitness: {:.4}", score);
        assert!(score > 0.90, "Textbook cap multiplier must score > 0.90, got {:.4}", score);
    }

    #[test]
    fn test_sallen_key_evolution_convergence() {
        let preset = Preset::load_or_builtin("sallen_key_10k").unwrap();
        let test_dir = std::path::PathBuf::from("target/test_sallen_key_convergence");
        let _ = std::fs::remove_dir_all(&test_dir);

        let config = crate::engine::EvolutionConfig {
            population_size: 20,
            max_generations: 30,
            novelty_threshold: 0.35,
            min_novelty_dist: 0.20,
            k_neighbors: 3,
            checkpoint_interval: 30,
            checkpoint_dir: test_dir,
            stray_cap_pf: 0.0,
            sim_timeout_secs: 5,
            monte_carlo_runs: 0,
            seed: Some(42),
            preset: Some(preset),
        };

        let mut engine = crate::engine::EvolutionEngine::new(config);
        let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let checkpoint = engine.run(running).expect("Sallen-Key evolution failed");

        let best_fit = checkpoint
            .archive
            .entries
            .iter()
            .map(|e| e.fitness.unwrap_or(0.0))
            .fold(0.0f64, f64::max);

        println!("Sallen-Key 30-Gen Best Fitness: {:.4}", best_fit);
        assert!(
            best_fit > 0.85,
            "Sallen-Key evolution should achieve > 0.85 fitness, got {:.4}",
            best_fit
        );
    }
}


