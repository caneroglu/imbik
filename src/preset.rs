use crate::parts::{PartCatalog, PartKind};
use serde::{Deserialize, Serialize};
use std::fs;

/// Selected component models for a mission preset
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetModels {
    #[serde(default = "default_opamp")]
    pub opamp: String,
    #[serde(default = "default_npn")]
    pub bjt_npn: String,
    #[serde(default = "default_pnp")]
    pub bjt_pnp: String,
    #[serde(default = "default_diode")]
    pub diode: String,
    #[serde(default)]
    pub has_explicit_opamp: bool,
}

pub fn default_opamp() -> String {
    "TL072".to_string()
}
pub fn default_npn() -> String {
    "2N3904".to_string()
}
pub fn default_pnp() -> String {
    "2N3906".to_string()
}
pub fn default_diode() -> String {
    "1N4148".to_string()
}

impl Default for PresetModels {
    fn default() -> Self {
        Self {
            opamp: default_opamp(),
            bjt_npn: default_npn(),
            bjt_pnp: default_pnp(),
            diode: default_diode(),
            has_explicit_opamp: false,
        }
    }
}

/// Helper schema for deserializing a preset from flexible TOML
#[derive(Debug, Clone, Deserialize)]
struct PresetRaw {
    pub name: String,
    pub description: String,
    pub feasibility_max_dc: f64,
    pub feasibility_rail_margin: f64,
    #[serde(default = "default_true")]
    pub allow_opamps: bool,
    #[serde(default)]
    pub op_amp: Option<String>,
    #[serde(default)]
    pub opamp: Option<String>,
    #[serde(default)]
    pub bjt_npn: Option<String>,
    #[serde(default)]
    pub npn: Option<String>,
    #[serde(default)]
    pub bjt_pnp: Option<String>,
    #[serde(default)]
    pub pnp: Option<String>,
    #[serde(default)]
    pub diode: Option<String>,
    #[serde(default)]
    pub models: Option<PresetModelsRaw>,
    pub probes: Vec<ProbeTarget>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PresetModelsRaw {
    pub op_amp: Option<String>,
    pub opamp: Option<String>,
    pub bjt_npn: Option<String>,
    pub npn: Option<String>,
    pub bjt_pnp: Option<String>,
    pub pnp: Option<String>,
    pub diode: Option<String>,
}

impl From<PresetRaw> for Preset {
    fn from(raw: PresetRaw) -> Self {
        let explicit_opamp = raw
            .models
            .as_ref()
            .and_then(|m| m.opamp.clone().or_else(|| m.op_amp.clone()))
            .or(raw.opamp)
            .or(raw.op_amp);

        let explicit_npn = raw
            .models
            .as_ref()
            .and_then(|m| m.npn.clone().or_else(|| m.bjt_npn.clone()))
            .or(raw.npn)
            .or(raw.bjt_npn);

        let explicit_pnp = raw
            .models
            .as_ref()
            .and_then(|m| m.pnp.clone().or_else(|| m.bjt_pnp.clone()))
            .or(raw.pnp)
            .or(raw.bjt_pnp);

        let explicit_diode = raw
            .models
            .as_ref()
            .and_then(|m| m.diode.clone())
            .or(raw.diode);

        let has_explicit_opamp = explicit_opamp.is_some();
        let models = PresetModels {
            opamp: explicit_opamp.unwrap_or_else(default_opamp),
            bjt_npn: explicit_npn.unwrap_or_else(default_npn),
            bjt_pnp: explicit_pnp.unwrap_or_else(default_pnp),
            diode: explicit_diode.unwrap_or_else(default_diode),
            has_explicit_opamp,
        };

        Preset {
            name: raw.name,
            description: raw.description,
            feasibility_max_dc: raw.feasibility_max_dc,
            feasibility_rail_margin: raw.feasibility_rail_margin,
            allow_opamps: raw.allow_opamps,
            models,
            probes: raw.probes,
        }
    }
}

/// Complete preset specification
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "PresetRaw")]
pub struct Preset {
    pub name: String,
    pub description: String,
    pub feasibility_max_dc: f64,
    pub feasibility_rail_margin: f64,
    pub allow_opamps: bool,
    pub models: PresetModels,
    pub probes: Vec<ProbeTarget>,
}

/// Severity level of a validation finding
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationSeverity {
    Info,
    Warning,
    Error,
}

/// A specific diagnostic finding encountered during preset validation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub severity: ValidationSeverity,
    pub component: String,
    pub message: String,
    pub suggestion: Option<String>,
}

/// Summary of a physical model tested during preset validation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelBenchmarkSummary {
    pub name: String,
    pub kind: String,
    pub is_builtin: bool,
    pub input_stage: Option<String>,
    pub measured_gbw_mhz: Option<f64>,
    pub measured_noise_1k: Option<f64>,
    pub measured_zin_1k: Option<f64>,
    pub measured_rail_margin_v: Option<f64>,
    pub is_rrio: Option<bool>,
}

/// Complete diagnostic report of preset and model validation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetValidationReport {
    pub preset_name: String,
    pub is_valid: bool,
    pub simulation_ready: bool,
    pub issues: Vec<ValidationIssue>,
    pub model_benchmarks: Vec<ModelBenchmarkSummary>,
}

impl PresetValidationReport {
    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == ValidationSeverity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == ValidationSeverity::Warning)
            .count()
    }

    pub fn format_diagnostic(&self) -> String {
        let mut out = String::new();
        out.push_str("\n=================================================================================\n");
        out.push_str(&format!(
            "  🔬 PRESET & MODEL VALIDATION REPORT: {}\n",
            self.preset_name
        ));
        out.push_str("=================================================================================\n");

        if self.is_valid {
            out.push_str(&format!(
                "  Status:  READY FOR SIMULATION ({} Errors, {} Warnings)\n\n",
                self.error_count(),
                self.warning_count()
            ));
        } else {
            out.push_str(&format!(
                "  Status:  FAILED VALIDATION ({} Errors, {} Warnings) - NOT READY\n\n",
                self.error_count(),
                self.warning_count()
            ));
        }

        for bench in &self.model_benchmarks {
            out.push_str(&format!(
                "  [MODEL] {:<10} ({}) - {}\n",
                bench.name,
                bench.kind,
                if bench.is_builtin {
                    "Built-in System Library"
                } else {
                    "Custom / User Imported"
                }
            ));
            if let Some(ref stage) = bench.input_stage {
                out.push_str(&format!("          Stage: {}\n", stage));
            }
            if let (Some(gbw), Some(noise)) = (bench.measured_gbw_mhz, bench.measured_noise_1k) {
                out.push_str(&format!(
                    "          GBW: {:.2} MHz | Noise@1kHz: {:.1} nV/√Hz | RRIO: {}\n",
                    gbw,
                    noise,
                    if bench.is_rrio == Some(true) {
                        "Yes"
                    } else {
                        "No"
                    }
                ));
            }
        }

        if !self.model_benchmarks.is_empty() {
            out.push('\n');
        }

        for issue in &self.issues {
            let tag = match issue.severity {
                ValidationSeverity::Error => "[ERROR]",
                ValidationSeverity::Warning => "[WARN] ",
                ValidationSeverity::Info => "[INFO] ",
            };
            out.push_str(&format!("  {} {}: {}\n", tag, issue.component, issue.message));
            if let Some(ref sug) = issue.suggestion {
                out.push_str(&format!("         ↳ Suggestion: {}\n", sug));
            }
        }

        out.push_str("=================================================================================\n");
        out
    }
}

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

impl Preset {
    /// Comprehensive validation of preset definition and component models against PartCatalog & SPICE
    pub fn validate_and_check(
        &self,
        catalog: &PartCatalog,
        run_spice_benchmarks: bool,
    ) -> PresetValidationReport {
        let mut issues = Vec::new();
        let mut model_benchmarks = Vec::new();

        // 1. Preset Schema Validation
        if self.name.trim().is_empty() {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Error,
                component: "Preset Metadata".to_string(),
                message: "Preset name cannot be empty.".to_string(),
                suggestion: Some("Provide a unique identifier name for the preset.".to_string()),
            });
        }

        if self.feasibility_max_dc < 0.0 {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Error,
                component: "feasibility_max_dc".to_string(),
                message: format!(
                    "feasibility_max_dc ({:.2}V) must be non-negative.",
                    self.feasibility_max_dc
                ),
                suggestion: Some("Use a positive threshold, e.g. 2.0 V.".to_string()),
            });
        }

        if self.feasibility_rail_margin < 0.0 {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Error,
                component: "feasibility_rail_margin".to_string(),
                message: format!(
                    "feasibility_rail_margin ({:.2}V) must be non-negative.",
                    self.feasibility_rail_margin
                ),
                suggestion: Some("Use a positive headroom margin, e.g. 0.5 V.".to_string()),
            });
        }

        if self.probes.is_empty() {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Error,
                component: "probes".to_string(),
                message: "Preset must define at least one fitness probe target.".to_string(),
                suggestion: Some(
                    "Add [[probes]] entries to define objectives (Gain, Zin, Noise, etc.)."
                        .to_string(),
                ),
            });
        }

        for (i, p) in self.probes.iter().enumerate() {
            if p.weight <= 0.0 {
                issues.push(ValidationIssue {
                    severity: ValidationSeverity::Warning,
                    component: format!("Probe #{} ({})", i + 1, p.name),
                    message: format!(
                        "Weight is {:.2} <= 0.0; probe will have zero impact on fitness.",
                        p.weight
                    ),
                    suggestion: Some("Set weight between 1.0 and 5.0.".to_string()),
                });
            }
            if p.condition.r_load < 0.0 || p.condition.r_source < 0.0 {
                issues.push(ValidationIssue {
                    severity: ValidationSeverity::Error,
                    component: format!("Probe #{} ({})", i + 1, p.name),
                    message: "Resistive load and source impedance must be non-negative.".to_string(),
                    suggestion: None,
                });
            }
        }

        // 2. Preset Model Logic & Policy Consistency
        if !self.allow_opamps && self.models.has_explicit_opamp {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Warning,
                component: "Op-Amp Policy Conflict".to_string(),
                message: format!(
                    "Preset has 'allow_opamps = false', but custom op-amp model '{}' is configured.",
                    self.models.opamp
                ),
                suggestion: Some(
                    "Set 'allow_opamps = true' in the preset if you want evolutionary search to explore op-amp topologies with this component.".to_string(),
                ),
            });
        }

        // 3. Model Verification & SPICE Readiness
        let check_list = [
            (
                "Op-Amp",
                &self.models.opamp,
                PartKind::OpAmp,
                5,
                self.allow_opamps,
            ),
            ("NPN BJT", &self.models.bjt_npn, PartKind::BjtNpn, 3, true),
            ("PNP BJT", &self.models.bjt_pnp, PartKind::BjtPnp, 3, true),
            ("Diode", &self.models.diode, PartKind::Diode, 2, true),
        ];

        for (role, model_name, expected_kind, expected_pins, is_active) in check_list {
            if !is_active {
                continue;
            }

            match catalog.get(model_name) {
                None => {
                    let available: Vec<String> = catalog
                        .list_by_kind(expected_kind)
                        .into_iter()
                        .map(|p| p.name.clone())
                        .collect();
                    issues.push(ValidationIssue {
                        severity: ValidationSeverity::Error,
                        component: format!("{} ({})", role, model_name),
                        message: format!(
                            "Component model '{}' not found in PartCatalog.",
                            model_name
                        ),
                        suggestion: Some(format!(
                            "Check 'parts/{}.toml' or run 'imbik part add <file>'. Available {} models: {:?}",
                            model_name.to_lowercase(),
                            role,
                            available
                        )),
                    });
                }
                Some(part) => {
                    if part.kind != expected_kind {
                        issues.push(ValidationIssue {
                            severity: ValidationSeverity::Error,
                            component: format!("{} ({})", role, model_name),
                            message: format!(
                                "Component '{}' is classified as {}, expected {}.",
                                model_name,
                                part.kind.as_str(),
                                expected_kind.as_str()
                            ),
                            suggestion: Some(format!(
                                "Specify a valid {} model in the preset.",
                                expected_kind.as_str()
                            )),
                        });
                    }

                    if part.pin_order.len() != expected_pins {
                        issues.push(ValidationIssue {
                            severity: ValidationSeverity::Error,
                            component: format!("{} ({})", role, model_name),
                            message: format!(
                                "Component '{}' has {} pin mapping entries, expected {}.",
                                model_name,
                                part.pin_order.len(),
                                expected_pins
                            ),
                            suggestion: Some(
                                "Correct the pin_order in the component definition.".to_string(),
                            ),
                        });
                    }

                    // Supply voltage headroom rating check against standard ±9V (18V) rails
                    if let Some(v_max) = part.specs.v_supply_max {
                        if v_max < 18.0 {
                            issues.push(ValidationIssue {
                                severity: ValidationSeverity::Warning,
                                component: format!("Supply Rating ({})", model_name),
                                message: format!(
                                    "'{}' maximum supply rating is {:.1}V, but standard evolution rails are ±9.0V (18.0V total).",
                                    model_name, v_max
                                ),
                                suggestion: Some(
                                    "SPICE macromodel will run normally, but real physical hardware breadboarding must use lower supply rails.".to_string(),
                                ),
                            });
                        }
                    }

                    // Live SPICE Smoke Test / Benchmark
                    if run_spice_benchmarks {
                        match catalog.smoke_test_part(model_name) {
                            Ok(msg) => {
                                issues.push(ValidationIssue {
                                    severity: ValidationSeverity::Info,
                                    component: format!("SPICE ({})", model_name),
                                    message: msg,
                                    suggestion: None,
                                });

                                model_benchmarks.push(ModelBenchmarkSummary {
                                    name: part.name.clone(),
                                    kind: part.kind.as_str().to_string(),
                                    is_builtin: part.is_builtin,
                                    input_stage: part
                                        .specs
                                        .input_stage
                                        .map(|s| s.as_str().to_string()),
                                    measured_gbw_mhz: part.specs.gbw_mhz,
                                    measured_noise_1k: part.specs.noise_spot_1k,
                                    measured_zin_1k: part.specs.zin_1k,
                                    measured_rail_margin_v: part.specs.rail_margin_v,
                                    is_rrio: part.specs.is_rrio,
                                });
                            }
                            Err(e) => {
                                issues.push(ValidationIssue {
                                    severity: ValidationSeverity::Error,
                                    component: format!("SPICE ({})", model_name),
                                    message: format!(
                                        "SPICE simulation failed for '{}': {}",
                                        model_name, e
                                    ),
                                    suggestion: Some(
                                        "Check model syntax and convergence parameters in ngspice."
                                            .to_string(),
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }

        let is_valid = !issues.iter().any(|i| i.severity == ValidationSeverity::Error);
        PresetValidationReport {
            preset_name: self.name.clone(),
            is_valid,
            simulation_ready: is_valid,
            issues,
            model_benchmarks,
        }
    }

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
            "# Component Models (Optional):\n",
            "# Specify custom imported or specific models to use in this mission.\n",
            "# If omitted, defaults to built-in models: TL072, 2N3904, 2N3906, 1N4148.\n",
            "# Can be specified as flat keys or under a [models] table.\n",
            "# ------------------------------------------------------------------------------\n",
            "# op_amp = \"MCP6002\"            # Active Op-Amp (e.g. MCP6002, TL072, NE5532, OPA2134)\n",
            "# bjt_npn = \"BC547B\"            # Active NPN BJT (e.g. BC547B, 2N3904)\n",
            "# bjt_pnp = \"BC557B\"            # Active PNP BJT (e.g. BC557B, 2N3906)\n",
            "# diode = \"BAT54\"               # Active Diode (e.g. BAT54, 1N4148, 1N5819)\n",
            "#\n",
            "# Or using table syntax:\n",
            "# [models]\n",
            "# opamp = \"OPA2134\"\n",
            "# bjt_npn = \"2N3904\"\n",
            "# bjt_pnp = \"2N3906\"\n",
            "# diode = \"1N4148\"\n\n",
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
            "#   - is_required : (alias: require) If true, candidate fails immediately (fitness = 0) if score <= 0.0\n",
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
            models: PresetModels::default(),
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
            models: PresetModels::default(),
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
    fn test_sallen_key_opa2134_evaluation() {
        let preset = Preset::load_or_builtin("sallen_key_opa2134").expect("Failed to load sallen_key_opa2134.toml");
        assert_eq!(preset.name, "sallen_key_opa2134");
        assert_eq!(preset.models.opamp, "OPA2134");

        // Build textbook 10kHz Sallen-Key Lowpass (R1=11k, R2=11k, C1=2.2nF, C2=1.0nF, OPA2134)
        let mut sk = crate::circuit::Circuit::new();
        sk.add_component(crate::circuit::Component::new('R', 1, vec![crate::circuit::NODE_IN, 10], "11k").unwrap());
        sk.add_component(crate::circuit::Component::new('R', 2, vec![10, 20], "11k").unwrap());
        sk.add_component(crate::circuit::Component::new('C', 1, vec![10, crate::circuit::NODE_OUT], "2.2nF").unwrap());
        sk.add_component(crate::circuit::Component::new('C', 2, vec![20, crate::circuit::NODE_GND], "1.0nF").unwrap());
        sk.add_component(
            crate::circuit::Component::new('X', 1, vec![20, crate::circuit::NODE_OUT, crate::circuit::NODE_VCC, crate::circuit::NODE_VEE, crate::circuit::NODE_OUT], "OPA2134").unwrap(),
        );

        let timeout = std::time::Duration::from_secs(5);
        let obj_vec = crate::fitness::evaluate_preset(&sk, &preset, 0.0, timeout)
            .expect("Textbook Sallen-Key with OPA2134 must pass feasibility gate and evaluate successfully");

        println!("Textbook Sallen-Key 10kHz (OPA2134) Evaluation:\n  {}", obj_vec.summary());
        let score = obj_vec.scalarized();
        println!("OPA2134 Scalarized Score: {:.4}", score);
        assert!(score > 0.85, "Textbook Sallen-Key with OPA2134 should achieve > 0.85 fitness, got {:.4}", score);
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
            stray_cap_pf: 22.0,
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

    #[test]
    fn test_preset_toml_deserialization_with_flat_models() {
        let toml_str = r#"
name = "test_flat_models"
description = "Testing flat model definitions"
feasibility_max_dc = 1.0
feasibility_rail_margin = 0.5
allow_opamps = true
op_amp = "MCP6002"
bjt_npn = "BC547B"
bjt_pnp = "BC557B"
diode = "BAT54"

[[probes]]
name = "Gain"
probe_type = "Gain"
kind = "Greater"
want = 1.0
soft = 0.1
weight = 1.0
is_required = true
[probes.condition]
freq = 1000.0
vin = 0.1
r_load = 10000.0
r_source = 600.0
"#;
        let preset: Preset = toml::from_str(toml_str).expect("Failed to deserialize preset with flat models");
        assert_eq!(preset.name, "test_flat_models");
        assert_eq!(preset.models.opamp, "MCP6002");
        assert_eq!(preset.models.bjt_npn, "BC547B");
        assert_eq!(preset.models.bjt_pnp, "BC557B");
        assert_eq!(preset.models.diode, "BAT54");
        assert!(preset.models.has_explicit_opamp);
    }

    #[test]
    fn test_preset_toml_deserialization_with_table_models() {
        let toml_str = r#"
name = "test_table_models"
description = "Testing table model definitions"
feasibility_max_dc = 1.0
feasibility_rail_margin = 0.5
allow_opamps = true

[models]
opamp = "OPA2134"
npn = "2N3904"
pnp = "2N3906"
diode = "1N4148"

[[probes]]
name = "Gain"
probe_type = "Gain"
kind = "Greater"
want = 1.0
soft = 0.1
weight = 1.0
is_required = true
[probes.condition]
freq = 1000.0
vin = 0.1
r_load = 10000.0
r_source = 600.0
"#;
        let preset: Preset = toml::from_str(toml_str).expect("Failed to deserialize preset with table models");
        assert_eq!(preset.name, "test_table_models");
        assert_eq!(preset.models.opamp, "OPA2134");
        assert_eq!(preset.models.bjt_npn, "2N3904");
        assert_eq!(preset.models.bjt_pnp, "2N3906");
        assert_eq!(preset.models.diode, "1N4148");
        assert!(preset.models.has_explicit_opamp);
    }

    #[test]
    fn test_preset_validation_missing_model() {
        let toml_str = r#"
name = "test_missing"
description = "Missing model test"
feasibility_max_dc = 1.0
feasibility_rail_margin = 0.5
allow_opamps = true
op_amp = "NON_EXISTENT_OPAMP_999"

[[probes]]
name = "Gain"
probe_type = "Gain"
kind = "Greater"
want = 1.0
soft = 0.1
weight = 1.0
is_required = true
[probes.condition]
freq = 1000.0
vin = 0.1
r_load = 10000.0
r_source = 600.0
"#;
        let preset: Preset = toml::from_str(toml_str).unwrap();
        let catalog = PartCatalog::builtin();
        let report = preset.validate_and_check(&catalog, false);
        assert!(!report.is_valid);
        assert!(report.error_count() >= 1);
        let missing_issue = report.issues.iter().find(|i| i.component.contains("NON_EXISTENT_OPAMP_999"));
        assert!(missing_issue.is_some());
    }

    #[test]
    fn test_preset_validation_kind_mismatch() {
        let toml_str = r#"
name = "test_mismatch"
description = "Kind mismatch test"
feasibility_max_dc = 1.0
feasibility_rail_margin = 0.5
allow_opamps = true
op_amp = "1N4148"

[[probes]]
name = "Gain"
probe_type = "Gain"
kind = "Greater"
want = 1.0
soft = 0.1
weight = 1.0
is_required = true
[probes.condition]
freq = 1000.0
vin = 0.1
r_load = 10000.0
r_source = 600.0
"#;
        let preset: Preset = toml::from_str(toml_str).unwrap();
        let catalog = PartCatalog::builtin();
        let report = preset.validate_and_check(&catalog, false);
        assert!(!report.is_valid);
        let mismatch_issue = report.issues.iter().find(|i| i.message.contains("expected Op-Amp"));
        assert!(mismatch_issue.is_some());
    }

    #[test]
    fn test_preset_validation_policy_conflict_warning() {
        let toml_str = r#"
name = "test_conflict"
description = "Conflict test"
feasibility_max_dc = 1.0
feasibility_rail_margin = 0.5
allow_opamps = false
op_amp = "TL072"

[[probes]]
name = "Gain"
probe_type = "Gain"
kind = "Greater"
want = 1.0
soft = 0.1
weight = 1.0
is_required = true
[probes.condition]
freq = 1000.0
vin = 0.1
r_load = 10000.0
r_source = 600.0
"#;
        let preset: Preset = toml::from_str(toml_str).unwrap();
        let catalog = PartCatalog::builtin();
        let report = preset.validate_and_check(&catalog, false);
        assert!(report.is_valid); // Warning does not invalidate
        let warn = report.issues.iter().find(|i| i.component == "Op-Amp Policy Conflict");
        assert!(warn.is_some());
    }

    #[test]
    fn test_preset_validation_mcp6002_live_spice() {
        let toml_str = r#"
name = "test_mcp6002"
description = "MCP6002 test"
feasibility_max_dc = 1.0
feasibility_rail_margin = 0.5
allow_opamps = true
op_amp = "MCP6002"

[[probes]]
name = "Gain"
probe_type = "Gain"
kind = "Greater"
want = 1.0
soft = 0.1
weight = 1.0
is_required = true
[probes.condition]
freq = 1000.0
vin = 0.1
r_load = 10000.0
r_source = 600.0
"#;
        let preset: Preset = toml::from_str(toml_str).unwrap();
        let catalog = PartCatalog::load_with_overrides();
        assert!(catalog.get("MCP6002").is_some(), "MCP6002 must be loaded from parts/");
        let report = preset.validate_and_check(&catalog, true);
        println!("{}", report.format_diagnostic());
        assert!(report.is_valid, "MCP6002 preset must pass SPICE smoke test and validation");
        assert!(report.model_benchmarks.iter().any(|b| b.name == "MCP6002"));
    }
}


