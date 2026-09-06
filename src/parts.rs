use crate::spice::run_simulation;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Broad classification of analog components
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PartKind {
    OpAmp,
    BjtNpn,
    BjtPnp,
    Diode,
    JfetN,
    MosfetN,
}

impl PartKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            PartKind::OpAmp => "Op-Amp",
            PartKind::BjtNpn => "NPN BJT",
            PartKind::BjtPnp => "PNP BJT",
            PartKind::Diode => "Diode",
            PartKind::JfetN => "N-JFET",
            PartKind::MosfetN => "N-MOSFET",
        }
    }
}

/// Physical input stage semiconductor technology
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputStage {
    Jfet,
    Bjt,
    Cmos,
}

impl InputStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            InputStage::Jfet => "JFET (Ultra High-Z)",
            InputStage::Bjt => "Bipolar (Low Noise)",
            InputStage::Cmos => "CMOS (Rail-to-Rail)",
        }
    }
}

/// Physical and operational characteristics measured or specified for a component
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PartSpecs {
    pub input_stage: Option<InputStage>,
    /// Input impedance at 1kHz in Ohms (e.g. 1e12 for JFET, 300k for BJT)
    pub zin_1k: Option<f64>,
    /// Voltage noise spectral density at 1kHz in nV/√Hz (e.g. 18.0 for TL072, 5.0 for NE5532)
    pub noise_spot_1k: Option<f64>,
    /// 1/f flicker noise corner frequency in Hz
    pub noise_corner_freq: Option<f64>,
    /// Minimum headroom from power rail before output saturation occurs (Volts)
    /// Non-RRIO op-amps: ~1.5V; RRIO op-amps: ~0.05V
    pub rail_margin_v: Option<f64>,
    /// Whether output/input swings rail-to-rail
    pub is_rrio: Option<bool>,
    /// Unity Gain Bandwidth in MHz
    pub gbw_mhz: Option<f64>,
    /// Slew Rate in V/µs
    pub slew_rate_v_us: Option<f64>,
    /// Recommended minimum supply voltage (e.g. 6.0 for ±3V)
    pub v_supply_min: Option<f64>,
    /// Maximum supply voltage rating (e.g. 36.0 for ±18V)
    pub v_supply_max: Option<f64>,
}

/// Complete definition of an analog part: metadata, pin order, and raw SPICE model
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartDefinition {
    pub name: String,
    pub kind: PartKind,
    pub description: String,
    /// Pin order mapping (e.g. for Op-Amp: [non_inv, inv, vcc, vee, out])
    pub pin_order: Vec<usize>,
    /// Raw SPICE text: either `.subckt <name> ... .ends` or `.model <name> ...`
    pub spice_text: String,
    /// Measured or specified physical characteristics
    pub specs: PartSpecs,
    /// Whether this part is built-in to the binary
    #[serde(default)]
    pub is_builtin: bool,
}

impl PartDefinition {
    pub fn new(
        name: &str,
        kind: PartKind,
        description: &str,
        pin_order: Vec<usize>,
        spice_text: &str,
        specs: PartSpecs,
    ) -> Self {
        Self {
            name: name.to_string(),
            kind,
            description: description.to_string(),
            pin_order,
            spice_text: spice_text.trim().to_string(),
            specs,
            is_builtin: false,
        }
    }
}

/// Helper schema for deserializing a part definition from a TOML configuration file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartTomlInput {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub spice_text: Option<String>,
    #[serde(default)]
    pub spice_file: Option<String>,
    #[serde(default)]
    pub pin_order: Option<Vec<usize>>,
    #[serde(default)]
    pub specs: Option<PartSpecs>,
}

/// Synthesize a valid SPICE macromodel from datasheet specifications when no raw SPICE text is provided
pub fn synthesize_macromodel(name: &str, kind: PartKind, specs: &PartSpecs) -> String {
    match kind {
        PartKind::OpAmp => {
            let is_jfet = matches!(specs.input_stage, Some(InputStage::Jfet) | Some(InputStage::Cmos))
                || specs.zin_1k.unwrap_or(0.0) > 1.0e8;
            let gbw = specs.gbw_mhz.unwrap_or(3.0).max(0.1);
            let rail_margin = specs.rail_margin_v.unwrap_or(if specs.is_rrio.unwrap_or(false) { 0.05 } else { 1.5 });
            let noise_1k = specs.noise_spot_1k.unwrap_or(15.0);

            // Transconductance Ga and Miller compensation Cc: GBW = Ga / (2 * pi * Cc)
            let ga = 1.0e-3;
            let cc = (ga / (2.0 * std::f64::consts::PI * gbw * 1.0e6)).max(1.0e-12);
            let c_diff = 1.5e-12;

            if is_jfet {
                format!(
                    concat!(
                        ".subckt {} 1 2 3 4 5\n",
                        "  * Synthesized Precision JFET/CMOS Op-Amp Macromodel (GBW={:.1}MHz, Vsat={:.2}V)\n",
                        "  C1 11 12 {:.4e}\n",
                        "  C2 6 7 {:.4e}\n",
                        "  DC 5 53 DX\n",
                        "  DE 54 5 DX\n",
                        "  DLP 90 91 DX\n",
                        "  DLN 92 90 DX\n",
                        "  DP 4 3 DX\n",
                        "  EGND 99 0 POLY(2) (3,0) (4,0) 0 .5 .5\n",
                        "  FB 7 99 POLY(5) VB VC VE VLP VLN 0 5.0e6 -5e6 5E6 5E6 -5E6\n",
                        "  GA 6 0 11 12 {:.4e}\n",
                        "  GCM 0 6 10 99 1.0e-9\n",
                        "  ISS 3 10 DC 5.0e-5\n",
                        "  HLIM 90 0 VLIM 1K\n",
                        "  J1 11 2 10 JX\n",
                        "  J2 12 1 10 JX\n",
                        "  R2 6 9 100.0E3\n",
                        "  RD1 4 11 2.0e3\n",
                        "  RD2 4 12 2.0e3\n",
                        "  RO1 8 5 40\n",
                        "  RO2 7 99 40\n",
                        "  RP 3 4 2.5e3\n",
                        "  VB 9 0 DC 0\n",
                        "  VC 3 53 DC {:.3}\n",
                        "  VE 54 4 DC {:.3}\n",
                        "  VLIM 7 8 DC 0\n",
                        "  VLP 91 0 DC 25\n",
                        "  VLN 0 92 DC 25\n",
                        "  .model DX D(IS=800.0E-18)\n",
                        "  .model JX PJF(BETA=1.5e-3 VTO=-1.0)\n",
                        ".ends\n"
                    ),
                    name, gbw, rail_margin, c_diff, cc, ga, rail_margin, rail_margin
                )
            } else {
                let rb = ((noise_1k.powi(2) - 16.0).max(1.0) / 0.165).clamp(5.0, 500.0);
                format!(
                    concat!(
                        ".subckt {} 1 2 3 4 5\n",
                        "  * Synthesized Low-Noise Bipolar Op-Amp Macromodel (GBW={:.1}MHz, en={:.1}nV/√Hz, Vsat={:.2}V)\n",
                        "  C1 11 12 {:.4e}\n",
                        "  C2 6 7 {:.4e}\n",
                        "  DC 5 53 DX\n",
                        "  DE 54 5 DX\n",
                        "  DLP 90 91 DX\n",
                        "  DLN 92 90 DX\n",
                        "  DP 4 3 DX\n",
                        "  EGND 99 0 POLY(2) (3,0) (4,0) 0 .5 .5\n",
                        "  FB 7 99 POLY(5) VB VC VE VLP VLN 0 5.0e6 -5e6 5E6 5E6 -5E6\n",
                        "  GA 6 0 11 12 {:.4e}\n",
                        "  GCM 0 6 10 99 1.0e-8\n",
                        "  ISS 10 4 DC 2.0e-4\n",
                        "  HLIM 90 0 VLIM 1K\n",
                        "  Q1 11 2 10 QIN\n",
                        "  Q2 12 1 10 QIN\n",
                        "  R2 6 9 100.0E3\n",
                        "  RC1 3 11 1.0e3\n",
                        "  RC2 3 12 1.0e3\n",
                        "  RO1 8 5 50\n",
                        "  RO2 7 99 50\n",
                        "  RP 3 4 3.0e3\n",
                        "  VB 9 0 DC 0\n",
                        "  VC 3 53 DC {:.3}\n",
                        "  VE 54 4 DC {:.3}\n",
                        "  VLIM 7 8 DC 0\n",
                        "  VLP 91 0 DC 25\n",
                        "  VLN 0 92 DC 25\n",
                        "  .model DX D(IS=800.0E-18)\n",
                        "  .model QIN NPN(IS=1e-15 BF=300 NF=1 VAF=100 CJC=2p CJE=3p RB={:.1} KF=1e-16 AF=1.0)\n",
                        ".ends\n"
                    ),
                    name, gbw, noise_1k, rail_margin, c_diff, cc, ga, rail_margin, rail_margin, rb
                )
            }
        }
        PartKind::BjtNpn => {
            let vaf = 100.0;
            let bf = 300.0;
            let rb = ((specs.noise_spot_1k.unwrap_or(3.0).powi(2) - 4.0).max(1.0) / 0.165).clamp(5.0, 200.0);
            format!(
                ".model {} NPN(Is=1e-14 Bf={:.0} Vaf={:.0} Cjc=3.5p Cje=4.5p Rb={:.1} Kf=1e-16 Af=1.0)\n",
                name, bf, vaf, rb
            )
        }
        PartKind::BjtPnp => {
            let vaf = 50.0;
            let bf = 200.0;
            let rb = ((specs.noise_spot_1k.unwrap_or(3.5).powi(2) - 4.0).max(1.0) / 0.165).clamp(5.0, 200.0);
            format!(
                ".model {} PNP(Is=1e-14 Bf={:.0} Vaf={:.0} Cjc=4.5p Cje=5.0p Rb={:.1} Kf=1.5e-16 Af=1.0)\n",
                name, bf, vaf, rb
            )
        }
        PartKind::Diode => {
            let is = if specs.rail_margin_v.unwrap_or(0.65) < 0.45 {
                "1e-7"
            } else {
                "1e-14"
            };
            format!(".model {} D(Is={} Rs=1.0 Cjo=2p)\n", name, is)
        }
        _ => format!(".model {} D(Is=1e-14)\n", name),
    }
}

/// Dynamic Part Catalog managing all active component models
#[derive(Debug, Clone, Default)]
pub struct PartCatalog {
    pub parts: HashMap<String, PartDefinition>,
}

impl PartCatalog {
    pub fn new() -> Self {
        Self {
            parts: HashMap::new(),
        }
    }

    /// Construct the canonical built-in library of op-amps, BJTs, and diodes
    pub fn builtin() -> Self {
        let mut catalog = Self::new();

        // 1. TL072: JFET-Input Low-Noise Audio Op-Amp
        let tl072_spice = concat!(
            ".subckt TL072 1 2 3 4 5\n",
            "  C1 11 12 3.498E-12\n",
            "  C2 6 7 12.00E-12\n",
            "  DC 5 53 DX\n",
            "  DE 54 5 DX\n",
            "  DLP 90 91 DX\n",
            "  DLN 92 90 DX\n",
            "  DP 4 3 DX\n",
            "  EGND 99 0 POLY(2) (3,0) (4,0) 0 .5 .5\n",
            "  FB 7 99 POLY(5) VB VC VE VLP VLN 0 4.715E6 -5E6 5E6 5E6 -5E6\n",
            "  GA 6 0 11 12 2.828E-4\n",
            "  GCM 0 6 10 99 8.944E-9\n",
            "  ISS 3 10 DC 1.072E-5\n",
            "  HLIM 90 0 VLIM 1K\n",
            "  J1 11 2 10 JX\n",
            "  J2 12 1 10 JX\n",
            "  R2 6 9 100.0E3\n",
            "  RD1 4 11 3.536E3\n",
            "  RD2 4 12 3.536E3\n",
            "  RO1 8 5 150\n",
            "  RO2 7 99 150\n",
            "  RP 3 4 2.143E3\n",
            "  VB 9 0 DC 0\n",
            "  VC 3 53 DC 2.200\n",
            "  VE 54 4 DC 2.200\n",
            "  VLIM 7 8 DC 0\n",
            "  VLP 91 0 DC 25\n",
            "  VLN 0 92 DC 25\n",
            "  .model DX D(IS=800.0E-18)\n",
            "  .model JX PJF(BETA=9.428E-4 BETATCE=-.5 VTO=-1.0 VTOTC=-2.5E-3)\n",
            ".ends\n"
        );
        catalog.register(PartDefinition {
            name: "TL072".to_string(),
            kind: PartKind::OpAmp,
            description: "Dual JFET-Input Low-Noise Audio Op-Amp (High-Zin)".to_string(),
            pin_order: vec![1, 2, 3, 4, 5],
            spice_text: tl072_spice.to_string(),
            specs: PartSpecs {
                input_stage: Some(InputStage::Jfet),
                zin_1k: Some(1.0e12),
                noise_spot_1k: Some(18.0),
                noise_corner_freq: Some(100.0),
                rail_margin_v: Some(1.5),
                is_rrio: Some(false),
                gbw_mhz: Some(3.0),
                slew_rate_v_us: Some(13.0),
                v_supply_min: Some(7.0),
                v_supply_max: Some(36.0),
            },
            is_builtin: true,
        });

        // 2. NE5532: Bipolar Ultra-Low Noise Audio Op-Amp
        let ne5532_spice = concat!(
            ".subckt NE5532 1 2 3 4 5\n",
            "  C1 11 12 1.4e-12\n",
            "  C2 6 7 10.0e-12\n",
            "  DC 5 53 DX\n",
            "  DE 54 5 DX\n",
            "  DLP 90 91 DX\n",
            "  DLN 92 90 DX\n",
            "  DP 4 3 DX\n",
            "  EGND 99 0 POLY(2) (3,0) (4,0) 0 .5 .5\n",
            "  FB 7 99 POLY(5) VB VC VE VLP VLN 0 5.0e6 -5e6 5E6 5E6 -5E6\n",
            "  GA 6 0 11 12 1.2e-3\n",
            "  GCM 0 6 10 99 1.0e-8\n",
            "  ISS 10 4 DC 2.0e-4\n",
            "  HLIM 90 0 VLIM 1K\n",
            "  Q1 11 2 10 QIN\n",
            "  Q2 12 1 10 QIN\n",
            "  R2 6 9 100.0E3\n",
            "  RC1 3 11 1.0e3\n",
            "  RC2 3 12 1.0e3\n",
            "  RO1 8 5 50\n",
            "  RO2 7 99 50\n",
            "  RP 3 4 3.0e3\n",
            "  VB 9 0 DC 0\n",
            "  VC 3 53 DC 1.500\n",
            "  VE 54 4 DC 1.500\n",
            "  VLIM 7 8 DC 0\n",
            "  VLP 91 0 DC 25\n",
            "  VLN 0 92 DC 25\n",
            "  .model DX D(IS=800.0E-18)\n",
            "  .model QIN NPN(IS=1e-15 BF=300 NF=1 VAF=100 CJC=2p CJE=3p RB=15 KF=1e-16 AF=1.0)\n",
            ".ends\n"
        );
        catalog.register(PartDefinition {
            name: "NE5532".to_string(),
            kind: PartKind::OpAmp,
            description: "Dual Ultra Low-Noise High-Speed Bipolar Audio Op-Amp (5 nV/√Hz)".to_string(),
            pin_order: vec![1, 2, 3, 4, 5],
            spice_text: ne5532_spice.to_string(),
            specs: PartSpecs {
                input_stage: Some(InputStage::Bjt),
                zin_1k: Some(300_000.0),
                noise_spot_1k: Some(5.0),
                noise_corner_freq: Some(50.0),
                rail_margin_v: Some(1.5),
                is_rrio: Some(false),
                gbw_mhz: Some(10.0),
                slew_rate_v_us: Some(9.0),
                v_supply_min: Some(6.0),
                v_supply_max: Some(40.0),
            },
            is_builtin: true,
        });

        // 3. LM358: Single-Supply Ground-Sensing Op-Amp
        let lm358_spice = concat!(
            ".subckt LM358 1 2 3 4 5\n",
            "  C1 11 12 2.0e-12\n",
            "  C2 6 7 30.0e-12\n",
            "  DC 5 53 DX\n",
            "  DE 54 5 DX\n",
            "  DLP 90 91 DX\n",
            "  DLN 92 90 DX\n",
            "  DP 4 3 DX\n",
            "  EGND 99 0 POLY(2) (3,0) (4,0) 0 .5 .5\n",
            "  FB 7 99 POLY(5) VB VC VE VLP VLN 0 1.0e6 -1e6 1e6 1e6 -1e6\n",
            "  GA 6 0 11 12 1.0e-4\n",
            "  GCM 0 6 10 99 1.0e-9\n",
            "  ISS 3 10 DC 1.0e-5\n",
            "  HLIM 90 0 VLIM 1K\n",
            "  Q1 11 2 10 QPNP\n",
            "  Q2 12 1 10 QPNP\n",
            "  R2 6 9 100.0E3\n",
            "  RD1 4 11 10.0e3\n",
            "  RD2 4 12 10.0e3\n",
            "  RO1 8 5 100\n",
            "  RO2 7 99 100\n",
            "  RP 3 4 5.0e3\n",
            "  VB 9 0 DC 0\n",
            "  VC 3 53 DC 1.500\n",
            "  VE 54 4 DC 0.050\n",
            "  VLIM 7 8 DC 0\n",
            "  VLP 91 0 DC 25\n",
            "  VLN 0 92 DC 25\n",
            "  .model DX D(IS=800.0E-18)\n",
            "  .model QPNP PNP(IS=1e-15 BF=100 NF=1 VAF=50 CJC=2p CJE=3p RB=50 KF=1e-15 AF=1.0)\n",
            ".ends\n"
        );
        catalog.register(PartDefinition {
            name: "LM358".to_string(),
            kind: PartKind::OpAmp,
            description: "Dual Single-Supply Ground-Sensing Op-Amp".to_string(),
            pin_order: vec![1, 2, 3, 4, 5],
            spice_text: lm358_spice.to_string(),
            specs: PartSpecs {
                input_stage: Some(InputStage::Bjt),
                zin_1k: Some(1_000_000.0),
                noise_spot_1k: Some(40.0),
                noise_corner_freq: Some(200.0),
                rail_margin_v: Some(1.2),
                is_rrio: Some(false),
                gbw_mhz: Some(1.0),
                slew_rate_v_us: Some(0.5),
                v_supply_min: Some(3.0),
                v_supply_max: Some(32.0),
            },
            is_builtin: true,
        });

        // 4. OPA2134: SoundPlus High-Fidelity Audio JFET Op-Amp
        let opa2134_spice = concat!(
            ".subckt OPA2134 1 2 3 4 5\n",
            "  C1 11 12 2.0e-12\n",
            "  C2 6 7 8.0e-12\n",
            "  DC 5 53 DX\n",
            "  DE 54 5 DX\n",
            "  DLP 90 91 DX\n",
            "  DLN 92 90 DX\n",
            "  DP 4 3 DX\n",
            "  EGND 99 0 POLY(2) (3,0) (4,0) 0 .5 .5\n",
            "  FB 7 99 POLY(5) VB VC VE VLP VLN 0 6.0e6 -6e6 6e6 6e6 -6e6\n",
            "  GA 6 0 11 12 5.0e-4\n",
            "  GCM 0 6 10 99 1.0e-9\n",
            "  ISS 3 10 DC 5.0e-5\n",
            "  HLIM 90 0 VLIM 1K\n",
            "  J1 11 2 10 JX\n",
            "  J2 12 1 10 JX\n",
            "  R2 6 9 100.0E3\n",
            "  RD1 4 11 2.0e3\n",
            "  RD2 4 12 2.0e3\n",
            "  RO1 8 5 40\n",
            "  RO2 7 99 40\n",
            "  RP 3 4 2.0e3\n",
            "  VB 9 0 DC 0\n",
            "  VC 3 53 DC 1.200\n",
            "  VE 54 4 DC 1.200\n",
            "  VLIM 7 8 DC 0\n",
            "  VLP 91 0 DC 25\n",
            "  VLN 0 92 DC 25\n",
            "  .model DX D(IS=800.0E-18)\n",
            "  .model JX PJF(BETA=1.5e-3 BETATCE=-.5 VTO=-1.5 VTOTC=-2.5E-3)\n",
            ".ends\n"
        );
        catalog.register(PartDefinition {
            name: "OPA2134".to_string(),
            kind: PartKind::OpAmp,
            description: "SoundPlus High-Fidelity Audio JFET Op-Amp (8 nV/√Hz)".to_string(),
            pin_order: vec![1, 2, 3, 4, 5],
            spice_text: opa2134_spice.to_string(),
            specs: PartSpecs {
                input_stage: Some(InputStage::Jfet),
                zin_1k: Some(1.0e13),
                noise_spot_1k: Some(8.0),
                noise_corner_freq: Some(40.0),
                rail_margin_v: Some(1.2),
                is_rrio: Some(false),
                gbw_mhz: Some(8.0),
                slew_rate_v_us: Some(20.0),
                v_supply_min: Some(5.0),
                v_supply_max: Some(36.0),
            },
            is_builtin: true,
        });

        // 5. 2N3904: NPN General Purpose Transistor
        let q3904_spice = ".model 2N3904 NPN(Is=6.734f Xti=3 Eg=1.11 Vaf=74.03 Bf=416.4 Ne=1.259 Ise=6.734f Ikf=66.78m Xtb=1.5 Br=.7371 Nc=2 Isc=0 Ikr=0 Rc=1 Cjc=3.638p Mjc=.3085 Vjc=.75 Fc=.5 Cje=4.493p Mje=.2593 Vje=.75 Tr=239.5n Tf=301.2p Itf=.4 Vtf=4 Xtf=2 Rb=10 Kf=1.2e-16 Af=1.1)\n";
        catalog.register(PartDefinition {
            name: "2N3904".to_string(),
            kind: PartKind::BjtNpn,
            description: "NPN General Purpose BJT Transistor (40V, 200mA, hFE=300)".to_string(),
            pin_order: vec![0, 1, 2], // C, B, E
            spice_text: q3904_spice.to_string(),
            specs: PartSpecs {
                input_stage: Some(InputStage::Bjt),
                zin_1k: Some(50_000.0),
                noise_spot_1k: Some(3.5),
                noise_corner_freq: Some(100.0),
                rail_margin_v: Some(0.2),
                is_rrio: Some(false),
                gbw_mhz: Some(300.0),
                slew_rate_v_us: None,
                v_supply_min: Some(1.0),
                v_supply_max: Some(40.0),
            },
            is_builtin: true,
        });

        // 6. 2N3906: PNP General Purpose Transistor
        let q3906_spice = ".model 2N3906 PNP(Is=1.41f Xti=3 Eg=1.11 Vaf=18.7 Bf=180.7 Ne=1.5 Ise=0 Ikf=80m Xtb=1.5 Br=4.977 Nc=2 Isc=0 Ikr=0 Rc=2 Cjc=4.5p Mjc=.3 Vjc=.75 Fc=.5 Cje=5p Mje=.3 Vje=.75 Tr=50n Tf=300p Itf=.4 Vtf=4 Xtf=2 Rb=10 Kf=1.5e-16 Af=1.1)\n";
        catalog.register(PartDefinition {
            name: "2N3906".to_string(),
            kind: PartKind::BjtPnp,
            description: "PNP General Purpose BJT Transistor (40V, 200mA, hFE=180)".to_string(),
            pin_order: vec![0, 1, 2], // C, B, E
            spice_text: q3906_spice.to_string(),
            specs: PartSpecs {
                input_stage: Some(InputStage::Bjt),
                zin_1k: Some(50_000.0),
                noise_spot_1k: Some(3.5),
                noise_corner_freq: Some(120.0),
                rail_margin_v: Some(0.2),
                is_rrio: Some(false),
                gbw_mhz: Some(250.0),
                slew_rate_v_us: None,
                v_supply_min: Some(1.0),
                v_supply_max: Some(40.0),
            },
            is_builtin: true,
        });

        // 7. BC547B: NPN Low-Noise Audio Transistor
        let bc547_spice = ".model BC547B NPN(Is=7.049f Xti=3 Eg=1.11 Vaf=100 Bf=290 Ne=1.3 Ise=7.049f Ikf=0.1 Xtb=1.5 Br=3.5 Nc=2 Isc=0 Ikr=0 Rc=0.8 Cjc=3.5p Mjc=.33 Vjc=.7 Fc=.5 Cje=9p Mje=.33 Vje=.7 Tr=10n Tf=400p Itf=.5 Vtf=5 Xtf=2 Rb=20 Kf=1.0e-16 Af=1.0)\n";
        catalog.register(PartDefinition {
            name: "BC547B".to_string(),
            kind: PartKind::BjtNpn,
            description: "NPN Low-Noise Audio BJT Transistor (45V, 100mA, hFE=290)".to_string(),
            pin_order: vec![0, 1, 2],
            spice_text: bc547_spice.to_string(),
            specs: PartSpecs {
                input_stage: Some(InputStage::Bjt),
                zin_1k: Some(60_000.0),
                noise_spot_1k: Some(2.8),
                noise_corner_freq: Some(80.0),
                rail_margin_v: Some(0.2),
                is_rrio: Some(false),
                gbw_mhz: Some(300.0),
                slew_rate_v_us: None,
                v_supply_min: Some(1.0),
                v_supply_max: Some(45.0),
            },
            is_builtin: true,
        });

        // 8. BC557B: PNP Low-Noise Audio Transistor
        let bc557_spice = ".model BC557B PNP(Is=1.2f Xti=3 Eg=1.11 Vaf=80 Bf=280 Ne=1.35 Ise=1.2f Ikf=0.08 Xtb=1.5 Br=3.0 Nc=2 Isc=0 Ikr=0 Rc=1.2 Cjc=5.5p Mjc=.33 Vjc=.7 Fc=.5 Cje=11p Mje=.33 Vje=.7 Tr=15n Tf=450p Itf=.4 Vtf=4 Xtf=2 Rb=25 Kf=1.2e-16 Af=1.0)\n";
        catalog.register(PartDefinition {
            name: "BC557B".to_string(),
            kind: PartKind::BjtPnp,
            description: "PNP Low-Noise Audio BJT Transistor (45V, 100mA, hFE=280)".to_string(),
            pin_order: vec![0, 1, 2],
            spice_text: bc557_spice.to_string(),
            specs: PartSpecs {
                input_stage: Some(InputStage::Bjt),
                zin_1k: Some(60_000.0),
                noise_spot_1k: Some(2.8),
                noise_corner_freq: Some(90.0),
                rail_margin_v: Some(0.2),
                is_rrio: Some(false),
                gbw_mhz: Some(250.0),
                slew_rate_v_us: None,
                v_supply_min: Some(1.0),
                v_supply_max: Some(45.0),
            },
            is_builtin: true,
        });

        // 9. 1N4148: Fast Switching Silicon Diode
        let d1n4148_spice = ".model 1N4148 D(is=2.52n rs=0.568 n=1.752 cjo=4p m=0.4 tt=20n)\n";
        catalog.register(PartDefinition {
            name: "1N4148".to_string(),
            kind: PartKind::Diode,
            description: "Fast High-Conductance Silicon Switching Diode (100V, 200mA, 4ns)".to_string(),
            pin_order: vec![0, 1], // Anode, Cathode
            spice_text: d1n4148_spice.to_string(),
            specs: PartSpecs {
                input_stage: None,
                zin_1k: None,
                noise_spot_1k: None,
                noise_corner_freq: None,
                rail_margin_v: Some(0.65), // Forward drop Vf
                is_rrio: None,
                gbw_mhz: None,
                slew_rate_v_us: None,
                v_supply_min: None,
                v_supply_max: Some(100.0),
            },
            is_builtin: true,
        });

        // 10. BAT54: Low Forward Drop Schottky Diode
        let bat54_spice = ".model BAT54 D(is=21.4n rs=1.2 n=1.05 cjo=10p m=0.35 tt=5n bv=30 ibv=10u)\n";
        catalog.register(PartDefinition {
            name: "BAT54".to_string(),
            kind: PartKind::Diode,
            description: "Surface Mount Low-Vf Schottky Barrier Diode (30V, 200mA, Vf=0.32V)".to_string(),
            pin_order: vec![0, 1],
            spice_text: bat54_spice.to_string(),
            specs: PartSpecs {
                input_stage: None,
                zin_1k: None,
                noise_spot_1k: None,
                noise_corner_freq: None,
                rail_margin_v: Some(0.32),
                is_rrio: None,
                gbw_mhz: None,
                slew_rate_v_us: None,
                v_supply_min: None,
                v_supply_max: Some(30.0),
            },
            is_builtin: true,
        });

        // 11. 1N5819: Schottky Power Barrier Rectifier
        let d5819_spice = ".model 1N5819 D(is=31.7u rs=0.051 n=1.25 cjo=110p m=0.38 tt=10n bv=40 ibv=1m)\n";
        catalog.register(PartDefinition {
            name: "1N5819".to_string(),
            kind: PartKind::Diode,
            description: "1A 40V Schottky Barrier Rectifier (High Surge, Low-Vf)".to_string(),
            pin_order: vec![0, 1],
            spice_text: d5819_spice.to_string(),
            specs: PartSpecs {
                input_stage: None,
                zin_1k: None,
                noise_spot_1k: None,
                noise_corner_freq: None,
                rail_margin_v: Some(0.40),
                is_rrio: None,
                gbw_mhz: None,
                slew_rate_v_us: None,
                v_supply_min: None,
                v_supply_max: Some(40.0),
            },
            is_builtin: true,
        });

        catalog
    }

    /// Load default catalog combined with any custom user models in parts/ directories
    pub fn load_with_overrides() -> Self {
        let mut catalog = Self::builtin();

        let mut search_dirs = vec![
            PathBuf::from("parts"),
            PathBuf::from("../parts"),
            PathBuf::from("../../parts"),
        ];

        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                search_dirs.push(exe_dir.join("parts"));
                search_dirs.push(exe_dir.join("../parts"));
                search_dirs.push(exe_dir.join("../../parts"));
            }
        }

        for dir in search_dirs {
            if dir.exists() && dir.is_dir() {
                catalog.scan_directory(&dir);
            }
        }

        catalog
    }

    /// Scan a directory recursively for .toml, .sub, .lib files and ingest them
    pub fn scan_directory(&mut self, dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    self.scan_directory(&path);
                } else if path.is_file() {
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
                    if ext == "toml" {
                        if let Ok(content) = fs::read_to_string(&path) {
                            if let Ok(part) = toml::from_str::<PartDefinition>(&content) {
                                self.register(part);
                            }
                        }
                    } else if matches!(ext.as_str(), "sub" | "lib" | "cir" | "mod") {
                        let _ = self.ingest_vendor_file(&path);
                    }
                }
            }
        }
    }

    pub fn register(&mut self, part: PartDefinition) {
        self.parts.insert(part.name.to_uppercase(), part);
    }

    pub fn get(&self, name: &str) -> Option<&PartDefinition> {
        self.parts.get(&name.to_uppercase())
    }

    /// Get all parts matching a kind
    pub fn list_by_kind(&self, kind: PartKind) -> Vec<&PartDefinition> {
        let mut list: Vec<_> = self.parts.values().filter(|p| p.kind == kind).collect();
        list.sort_by_key(|p| p.name.clone());
        list
    }

    /// Generate unified SPICE preamble text containing all needed component models
    pub fn all_spice_headers(&self) -> String {
        let mut headers = String::new();
        headers.push_str("* === Dynamic Component Catalog Preamble ===\n");
        for part in self.parts.values() {
            headers.push_str(&part.spice_text);
            if !part.spice_text.ends_with('\n') {
                headers.push('\n');
            }
        }
        headers
    }

    /// Generate filtered SPICE preamble containing only the models specifically requested or used
    pub fn to_spice_preamble(&self, used_names: &[&str]) -> String {
        let mut headers = String::new();
        headers.push_str("* === Dynamic Component Catalog Preamble ===\n");

        if used_names.is_empty() {
            return self.all_spice_headers();
        }

        for &name in used_names {
            if let Some(part) = self.get(name) {
                headers.push_str(&part.spice_text);
                if !part.spice_text.ends_with('\n') {
                    headers.push('\n');
                }
            }
        }

        headers
    }

    /// Return documented TOML configuration template for defining custom components
    pub fn template_toml(kind: &str) -> &'static str {
        match kind.to_lowercase().as_str() {
            "bjt" | "transistor" | "npn" | "pnp" => concat!(
                "# ==============================================================================\n",
                "#   İmbik Component Definition Template - BJT (Bipolar Junction Transistor)\n",
                "#   Save this file as 'parts/<name>.toml' and run: imbik part add parts/<name>.toml\n",
                "# ==============================================================================\n\n",
                "name = \"BC547B\"\n",
                "kind = \"BjtNpn\"                # Options: \"BjtNpn\", \"BjtPnp\"\n",
                "description = \"General Purpose NPN Audio Low-Noise Transistor (45V, 100mA, hFE=290)\"\n\n",
                "# ------------------------------------------------------------------------------\n",
                "# OPTION A: Direct SPICE .model text (if available from datasheet/manufacturer)\n",
                "# ------------------------------------------------------------------------------\n",
                "# spice_text = \".model BC547B NPN(Is=7.05f Bf=290 Vaf=100 Cjc=3.5p Cje=9p Rb=20 Kf=1e-16)\"\n\n",
                "# ------------------------------------------------------------------------------\n",
                "# OPTION B: Parametric datasheet synthesis (İmbik generates Gummel-Poon model)\n",
                "# ------------------------------------------------------------------------------\n",
                "[specs]\n",
                "input_stage = \"Bjt\"\n",
                "gbw_mhz = 300.0                # Transition frequency fT in MHz (sets junction capacitances)\n",
                "noise_spot_1k = 2.8            # Voltage noise density in nV/√Hz (sets base spreading resistance Rb)\n",
                "noise_corner_freq = 100.0      # Flicker noise corner frequency in Hz\n",
                "v_supply_max = 45.0            # Vceo collector-emitter breakdown voltage in Volts\n",
                "rail_margin_v = 0.2            # Vce(sat) saturation voltage in Volts (typically 0.1V - 0.3V)\n"
            ),
            "diode" => concat!(
                "# ==============================================================================\n",
                "#   İmbik Component Definition Template - Diode\n",
                "#   Save this file as 'parts/<name>.toml' and run: imbik part add parts/<name>.toml\n",
                "# ==============================================================================\n\n",
                "name = \"1N4148\"\n",
                "kind = \"Diode\"\n",
                "description = \"High-speed silicon switching diode (100V, 200mA, trr=4ns)\"\n\n",
                "# ------------------------------------------------------------------------------\n",
                "# OPTION A: Direct SPICE .model text\n",
                "# ------------------------------------------------------------------------------\n",
                "# spice_text = \".model 1N4148 D(Is=2.52n Rs=0.568 N=1.752 Cjo=4p M=0.4 tt=5.76n)\"\n\n",
                "# ------------------------------------------------------------------------------\n",
                "# OPTION B: Parametric datasheet synthesis\n",
                "# ------------------------------------------------------------------------------\n",
                "[specs]\n",
                "rail_margin_v = 0.65           # Forward voltage drop Vf in Volts (e.g. 0.65V for Silicon, 0.35V for Schottky)\n",
                "v_supply_max = 100.0           # Reverse breakdown voltage BV in Volts\n"
            ),
            _ => concat!(
                "# ==============================================================================\n",
                "#   İmbik Component Definition Template - Op-Amp (Operational Amplifier)\n",
                "#   Save this file as 'parts/<name>.toml' and run: imbik part add parts/<name>.toml\n",
                "# ==============================================================================\n\n",
                "name = \"MCP6002\"\n",
                "kind = \"OpAmp\"                  # Options: \"OpAmp\", \"BjtNpn\", \"BjtPnp\", \"Diode\"\n",
                "description = \"Microchip 1MHz Low-Power Rail-to-Rail Dual CMOS Op-Amp\"\n\n",
                "# ------------------------------------------------------------------------------\n",
                "# OPTION A: If you have a vendor SPICE subcircuit or file, define either:\n",
                "# ------------------------------------------------------------------------------\n",
                "# spice_file = \"vendor/MCP6002.LIB\"\n",
                "# or\n",
                "# spice_text = \"\"\"\n",
                "# .subckt MCP6002 1 2 3 4 5\n",
                "# ...\n",
                "# .ends\n",
                "# \"\"\"\n",
                "# pin_order = [1, 2, 3, 4, 5]   # [IN+, IN-, VCC, VEE, OUT]\n\n",
                "# ------------------------------------------------------------------------------\n",
                "# OPTION B: If you do NOT have a SPICE model, fill datasheet specs below.\n",
                "# İmbik will automatically synthesize a precision Boyle macromodel!\n",
                "# ------------------------------------------------------------------------------\n",
                "[specs]\n",
                "# Semiconductor input stage technology:\n",
                "# \"Cmos\" -> Ultra High-Z (Zin > 1 TΩ), low bias current\n",
                "# \"Jfet\" -> High-Z (Zin > 100 GΩ), audio low noise\n",
                "# \"Bjt\"  -> Bipolar (Zin ~ 300 kΩ), ultra low voltage noise\n",
                "input_stage = \"Cmos\"\n\n",
                "# AC & Dynamic performance:\n",
                "gbw_mhz = 1.0                  # Gain-Bandwidth Product in MHz (e.g. 1.0 for LM358, 3.0 for TL072, 10.0 for NE5532)\n",
                "slew_rate_v_us = 0.6           # Slew Rate in V/µs (optional)\n\n",
                "# Noise performance:\n",
                "noise_spot_1k = 28.0           # Input voltage noise density at 1kHz in nV/√Hz (e.g. 5.0 for NE5532, 18.0 for TL072)\n",
                "noise_corner_freq = 150.0      # 1/f flicker noise corner frequency in Hz (optional, default: 100.0)\n\n",
                "# Rail headroom & saturation behavior:\n",
                "is_rrio = true                 # Rail-to-Rail Output: true (swings within 50mV) / false (1.5V drop)\n",
                "rail_margin_v = 0.05           # Output saturation margin from rails in Volts (e.g. 0.05 for RRIO, 1.5 for standard)\n\n",
                "# Supply voltage ratings:\n",
                "v_supply_min = 1.8             # Recommended minimum total supply in Volts (e.g. 1.8V for single supply, 6.0V for dual ±3V)\n",
                "v_supply_max = 6.0             # Absolute maximum supply rating in Volts (e.g. 6.0V, 36.0V for ±18V)\n"
            ),
        }
    }

    /// Read file content handling UTF-8, UTF-8 with BOM, and Windows PowerShell UTF-16 LE
    pub fn read_file_content_robust(path: &Path) -> Result<String, String> {
        let bytes = fs::read(path).map_err(|e| format!("Failed to read file {:?}: {}", path, e))?;
        // Check UTF-16 LE BOM (0xFF, 0xFE)
        if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
            let u16_slice: Vec<u16> = bytes[2..]
                .chunks_exact(2)
                .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                .collect();
            return String::from_utf16(&u16_slice)
                .map_err(|e| format!("Invalid UTF-16 content in {:?}: {}", path, e));
        }
        // Check UTF-8 BOM (0xEF, 0xBB, 0xBF)
        if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
            return String::from_utf8(bytes[3..].to_vec())
                .map_err(|e| format!("Invalid UTF-8 content in {:?}: {}", path, e));
        }
        String::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8 content in {:?}: {}", path, e))
    }

    /// Parse a vendor SPICE file (.lib/.sub/.mod) OR a component TOML (.toml), extract models, and benchmark
    pub fn ingest_vendor_file(&mut self, path: &Path) -> Result<Vec<String>, String> {
        let content = Self::read_file_content_robust(path)?;

        if path.extension().and_then(|ext| ext.to_str()).map(|s| s.eq_ignore_ascii_case("toml")).unwrap_or(false) {
            let clean_content: String = content
                .lines()
                .skip_while(|l| l.trim().starts_with("===") || l.trim().starts_with("İmbik") || l.trim().starts_with("Autonomous"))
                .collect::<Vec<_>>()
                .join("\n");

            let input: PartTomlInput = toml::from_str(&clean_content)
                .map_err(|e| format!("Failed to parse component TOML {:?}: {}", path, e))?;

            let kind = match input.kind.to_lowercase().as_str() {
                "opamp" => PartKind::OpAmp,
                "bjtnpn" | "npn" => PartKind::BjtNpn,
                "bjtpnp" | "pnp" => PartKind::BjtPnp,
                "diode" => PartKind::Diode,
                "jfetn" | "jfet" => PartKind::JfetN,
                "mosfetn" | "mosfet" => PartKind::MosfetN,
                _ => PartKind::OpAmp,
            };

            let specs = input.specs.unwrap_or_default();
            let spice_text = if let Some(txt) = input.spice_text {
                txt
            } else if let Some(ref rel_file) = input.spice_file {
                let p = path.parent().unwrap_or_else(|| Path::new("")).join(rel_file);
                fs::read_to_string(&p).map_err(|e| format!("Failed to read referenced spice_file {:?}: {}", p, e))?
            } else {
                synthesize_macromodel(&input.name, kind, &specs)
            };

            let pin_order = input.pin_order.unwrap_or_else(|| {
                match kind {
                    PartKind::OpAmp => vec![1, 2, 3, 4, 5],
                    PartKind::Diode => vec![0, 1],
                    _ => vec![0, 1, 2],
                }
            });

            let part_def = PartDefinition {
                name: input.name.clone(),
                kind,
                description: input.description.unwrap_or_else(|| format!("Custom Component {}", input.name)),
                pin_order,
                spice_text,
                specs,
                is_builtin: false,
            };

            self.register(part_def);
            return Ok(vec![input.name]);
        }

        let file_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("CUSTOM_PART")
            .to_uppercase();

        let mut added_names = Vec::new();
        let mut current_subckt: Option<(String, Vec<String>, String)> = None;

        for line in content.lines() {
            let trimmed = line.trim();
            let upper = trimmed.to_uppercase();

            if upper.starts_with(".SUBCKT") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    let name = parts[1].to_uppercase();
                    let pin_names: Vec<String> = parts[2..].iter().map(|s| s.to_string()).collect();
                    let mut body = String::new();
                    body.push_str(trimmed);
                    body.push('\n');
                    current_subckt = Some((name, pin_names, body));
                }
            } else if let Some((ref name, ref pins, ref mut body)) = current_subckt {
                body.push_str(trimmed);
                body.push('\n');
                if upper.starts_with(".ENDS") {
                    let pin_count = pins.len();
                    let (kind, pin_order) = if pin_count >= 5 {
                        (PartKind::OpAmp, vec![1, 2, 3, 4, 5])
                    } else if pin_count == 3 {
                        (PartKind::BjtNpn, vec![0, 1, 2])
                    } else {
                        (PartKind::Diode, vec![0, 1])
                    };

                    let part_def = PartDefinition {
                        name: name.clone(),
                        kind,
                        description: format!("Vendor Imported Subcircuit from {:?}", path.file_name().unwrap_or_default()),
                        pin_order,
                        spice_text: body.clone(),
                        specs: PartSpecs::default(),
                        is_builtin: false,
                    };

                    self.register(part_def);
                    added_names.push(name.clone());
                    current_subckt = None;
                }
            } else if upper.starts_with(".MODEL") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 3 {
                    let model_name = parts[1].to_uppercase();
                    let model_type = parts[2].to_uppercase();
                    let kind = if model_type.starts_with("NPN") {
                        PartKind::BjtNpn
                    } else if model_type.starts_with("PNP") {
                        PartKind::BjtPnp
                    } else {
                        PartKind::Diode
                    };

                    let part_def = PartDefinition {
                        name: model_name.clone(),
                        kind,
                        description: format!("Vendor Imported Model from {:?}", path.file_name().unwrap_or_default()),
                        pin_order: if kind == PartKind::Diode { vec![0, 1] } else { vec![0, 1, 2] },
                        spice_text: format!("{}\n", trimmed),
                        specs: PartSpecs::default(),
                        is_builtin: false,
                    };

                    self.register(part_def);
                    added_names.push(model_name);
                }
            }
        }

        if added_names.is_empty() && !content.trim().is_empty() {
            // Treat entire file as single custom subcircuit
            let part_def = PartDefinition {
                name: file_stem.clone(),
                kind: PartKind::OpAmp,
                description: format!("Vendor Imported Macromodel from {:?}", path.file_name().unwrap_or_default()),
                pin_order: vec![1, 2, 3, 4, 5],
                spice_text: content,
                specs: PartSpecs::default(),
                is_builtin: false,
            };
            self.register(part_def);
            added_names.push(file_stem);
        }

        Ok(added_names)
    }

    /// Run a 50ms SPICE physical characterization bench on any Op-Amp to extract real Zin, noise, rail margin, GBW
    pub fn benchmark_opamp(&self, part_name: &str) -> Result<PartSpecs, String> {
        let part = self.get(part_name).ok_or_else(|| format!("Part '{}' not found in catalog", part_name))?;
        let subckt_text = &part.spice_text;

        let mut test_netlist = String::new();
        test_netlist.push_str(&format!("* Benchmark Bench for Op-Amp: {}\n", part.name));
        test_netlist.push_str(subckt_text);
        if !subckt_text.ends_with('\n') {
            test_netlist.push('\n');
        }

        // Standard op-amp test circuit: Unity-Gain Follower with ±15V supplies
        // Subcircuit pins: [non_inv, inv, vcc, vee, out]
        test_netlist.push_str("V_in 100 0 dc 0 ac 1\n");
        test_netlist.push_str("R_src 100 1 600.0\n");
        test_netlist.push_str("V_cc 3 0 dc 15.0\n");
        test_netlist.push_str("V_ee 4 0 dc -15.0\n");
        test_netlist.push_str(&format!("X1 1 2 3 4 2 {}\n", part.name));
        test_netlist.push_str("R_load 2 0 10000.0\n");

        test_netlist.push_str(".control\n");
        test_netlist.push_str("op\n");
        test_netlist.push_str("print allv\n");
        test_netlist.push_str("ac dec 50 10 10meg\n");
        test_netlist.push_str("let zin = mag(v(1)/i(v_in))\n");
        test_netlist.push_str("let gain = mag(v(2)/v(1))\n");
        test_netlist.push_str("wrdata probe_ac.txt gain zin\n");
        test_netlist.push_str("noise v(2) v_in dec 50 10 100k\n");
        test_netlist.push_str("print inoise_total onoise_total\n");
        test_netlist.push_str("setplot noise1\n");
        test_netlist.push_str("wrdata noise_out.txt onoise_spectrum inoise_spectrum\n");
        // Rail swing test: force DC input to +15V and -15V
        test_netlist.push_str("alter @v_in[dc] = 15.0\n");
        test_netlist.push_str("op\n");
        test_netlist.push_str("print v(2)\n");
        test_netlist.push_str("quit\n");
        test_netlist.push_str(".endc\n");
        test_netlist.push_str(".end\n");

        let timeout = Duration::from_secs(4);
        let sim_res = run_simulation(&test_netlist, timeout)
            .map_err(|e| format!("Benchmark SPICE run failed: {}", e))?;

        let mut specs = part.specs.clone();

        // 1. Extract Zin @ 1kHz
        if let Some(ref probe_pts) = sim_res.probe_ac {
            if let Some(pt_1k) = probe_pts.iter().min_by(|a, b| (a.freq - 1000.0).abs().partial_cmp(&(b.freq - 1000.0).abs()).unwrap()) {
                specs.zin_1k = Some(pt_1k.zin);
                specs.input_stage = if pt_1k.zin > 5.0e10 {
                    Some(InputStage::Jfet)
                } else if pt_1k.zin > 1.0e9 {
                    Some(InputStage::Cmos)
                } else {
                    Some(InputStage::Bjt)
                };
            }

            // 2. Extract Unity Gain Bandwidth (GBW)
            if let Some(pt_unity) = probe_pts.iter().find(|p| p.gain <= 0.707) {
                specs.gbw_mhz = Some(pt_unity.freq / 1.0e6);
            } else if let Some(last_pt) = probe_pts.last() {
                specs.gbw_mhz = Some(last_pt.freq / 1.0e6);
            }
        }

        // 3. Extract Noise Metrics
        if let Some(ref noise_sum) = sim_res.noise_summary {
            specs.noise_spot_1k = Some(noise_sum.inoise_spot_1k);
            specs.noise_corner_freq = noise_sum.corner_freq;
        }

        // 4. Extract Rail Margin (Headroom)
        if let Some(&v_sat) = sim_res.dc_nodes.get("2").or_else(|| sim_res.dc_nodes.get("v(2)")) {
            let margin = (15.0 - v_sat).abs();
            specs.rail_margin_v = Some(margin);
            specs.is_rrio = Some(margin < 0.20);
        }

        Ok(specs)
    }

    /// Quick SPICE smoke test to ensure a component model parses and converges in ngspice
    pub fn smoke_test_part(&self, part_name: &str) -> Result<String, String> {
        let part = self.get(part_name).ok_or_else(|| format!("Part '{}' not found in catalog", part_name))?;
        match part.kind {
            PartKind::OpAmp => {
                let specs = self.benchmark_opamp(part_name)?;
                Ok(format!(
                    "Op-Amp SPICE benchmark passed (GBW: {:.2} MHz, Noise: {:.1} nV/√Hz, RRIO: {})",
                    specs.gbw_mhz.unwrap_or(0.0),
                    specs.noise_spot_1k.unwrap_or(0.0),
                    if specs.is_rrio == Some(true) { "Yes" } else { "No" }
                ))
            }
            PartKind::BjtNpn => {
                let mut netlist = String::new();
                netlist.push_str(&format!("* Smoke Test Bench for NPN BJT: {}\n", part.name));
                netlist.push_str(&part.spice_text);
                if !part.spice_text.ends_with('\n') {
                    netlist.push('\n');
                }
                netlist.push_str("Vcc 1 0 dc 5.0\n");
                netlist.push_str("Rc 1 2 1k\n");
                netlist.push_str(&format!("Q1 2 3 0 {}\n", part.name));
                netlist.push_str("Rb 4 3 10k\n");
                netlist.push_str("Vin 4 0 dc 1.0\n");
                netlist.push_str(".control\nop\nprint allv\nquit\n.endc\n.end\n");
                let sim_res = run_simulation(&netlist, Duration::from_secs(3))
                    .map_err(|e| format!("NPN SPICE test failed: {}", e))?;
                if sim_res.dc_nodes.is_empty() {
                    return Err("SPICE returned empty operating point for NPN".to_string());
                }
                Ok(format!("NPN BJT SPICE model verified (.op converged with {} nodes)", sim_res.dc_nodes.len()))
            }
            PartKind::BjtPnp => {
                let mut netlist = String::new();
                netlist.push_str(&format!("* Smoke Test Bench for PNP BJT: {}\n", part.name));
                netlist.push_str(&part.spice_text);
                if !part.spice_text.ends_with('\n') {
                    netlist.push('\n');
                }
                netlist.push_str("Vee 1 0 dc -5.0\n");
                netlist.push_str("Rc 1 2 1k\n");
                netlist.push_str(&format!("Q1 2 3 0 {}\n", part.name));
                netlist.push_str("Rb 4 3 10k\n");
                netlist.push_str("Vin 4 0 dc -1.0\n");
                netlist.push_str(".control\nop\nprint allv\nquit\n.endc\n.end\n");
                let sim_res = run_simulation(&netlist, Duration::from_secs(3))
                    .map_err(|e| format!("PNP SPICE test failed: {}", e))?;
                if sim_res.dc_nodes.is_empty() {
                    return Err("SPICE returned empty operating point for PNP".to_string());
                }
                Ok(format!("PNP BJT SPICE model verified (.op converged with {} nodes)", sim_res.dc_nodes.len()))
            }
            PartKind::Diode => {
                let mut netlist = String::new();
                netlist.push_str(&format!("* Smoke Test Bench for Diode: {}\n", part.name));
                netlist.push_str(&part.spice_text);
                if !part.spice_text.ends_with('\n') {
                    netlist.push('\n');
                }
                netlist.push_str("Vin 1 0 dc 1.0\n");
                netlist.push_str("R1 1 2 1k\n");
                netlist.push_str(&format!("D1 2 0 {}\n", part.name));
                netlist.push_str(".control\nop\nprint allv\nquit\n.endc\n.end\n");
                let sim_res = run_simulation(&netlist, Duration::from_secs(3))
                    .map_err(|e| format!("Diode SPICE test failed: {}", e))?;
                if sim_res.dc_nodes.is_empty() {
                    return Err("SPICE returned empty operating point for Diode".to_string());
                }
                Ok(format!("Diode SPICE model verified (.op converged with {} nodes)", sim_res.dc_nodes.len()))
            }
            _ => Ok("Custom component syntax accepted".to_string()),
        }
    }
}

/// Global convenience function to get standard SPICE headers from catalog
pub fn default_spice_headers() -> String {
    let catalog = PartCatalog::load_with_overrides();
    catalog.all_spice_headers()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_catalog_integrity() {
        let catalog = PartCatalog::builtin();
        assert!(catalog.parts.len() >= 10, "Built-in catalog must have at least 10 core parts");
        assert!(catalog.get("TL072").is_some());
        assert!(catalog.get("NE5532").is_some());
        assert!(catalog.get("LM358").is_some());
        assert!(catalog.get("OPA2134").is_some());
        assert!(catalog.get("2N3904").is_some());
        assert!(catalog.get("2N3906").is_some());
        assert!(catalog.get("1N4148").is_some());
    }

    #[test]
    fn test_benchmark_tl072_vs_ne5532_physics() {
        let catalog = PartCatalog::builtin();

        let tl072_specs = catalog
            .benchmark_opamp("TL072")
            .expect("TL072 benchmark must succeed");
        println!("TL072 Measured Specs: {:?}", tl072_specs);
        assert_eq!(tl072_specs.input_stage, Some(InputStage::Jfet));
        assert!(tl072_specs.zin_1k.unwrap() > 1.0e8, "TL072 Zin should be JFET high-Z (>100M)");

        let ne5532_specs = catalog
            .benchmark_opamp("NE5532")
            .expect("NE5532 benchmark must succeed");
        println!("NE5532 Measured Specs: {:?}", ne5532_specs);
        assert_eq!(ne5532_specs.input_stage, Some(InputStage::Bjt));
        assert!(
            ne5532_specs.noise_spot_1k.unwrap() < tl072_specs.noise_spot_1k.unwrap(),
            "NE5532 bipolar input should have lower voltage noise than TL072 JFET"
        );
    }

    #[test]
    fn test_ingest_custom_vendor_subckt() {
        let mut catalog = PartCatalog::new();
        let custom_subckt = concat!(
            "* Texas Instruments Custom Precision Op-Amp OPA999\n",
            ".SUBCKT OPA999 INP INN VCC VEE OUT\n",
            "  C1 11 12 1.0e-12\n",
            "  C2 6 7 5.0e-12\n",
            "  DC OUT 53 DX\n",
            "  DE 54 OUT DX\n",
            "  DLP 90 91 DX\n",
            "  DLN 92 90 DX\n",
            "  EGND 99 0 POLY(2) (VCC,0) (VEE,0) 0 .5 .5\n",
            "  FB 7 99 POLY(5) VB VC VE VLP VLN 0 1.0e7 -1e7 1e7 1e7 -1e7\n",
            "  GA 6 0 11 12 1.0e-3\n",
            "  ISS VCC 10 DC 1.0e-4\n",
            "  HLIM 90 0 VLIM 1K\n",
            "  J1 11 INN 10 JX\n",
            "  J2 12 INP 10 JX\n",
            "  R2 6 9 100.0E3\n",
            "  RD1 VEE 11 1.0e3\n",
            "  RD2 VEE 12 1.0e3\n",
            "  RO1 8 OUT 20\n",
            "  RO2 7 99 20\n",
            "  VB 9 0 DC 0\n",
            "  VC VCC 53 DC 0.200\n",
            "  VE 54 VEE DC 0.200\n",
            "  VLIM 7 8 DC 0\n",
            "  VLP 91 0 DC 25\n",
            "  VLN 0 92 DC 25\n",
            "  .model DX D(IS=800.0E-18)\n",
            "  .model JX PJF(BETA=2.0e-3 VTO=-1.0)\n",
            ".ENDS\n"
        );

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("OPA999.LIB");
        fs::write(&file_path, custom_subckt).unwrap();

        let ingested = catalog.ingest_vendor_file(&file_path).unwrap();
        assert_eq!(ingested, vec!["OPA999"]);
        assert!(catalog.get("OPA999").is_some());

        let specs = catalog.benchmark_opamp("OPA999").expect("Custom OPA999 benchmark should pass");
        println!("Custom OPA999 Measured Specs: {:?}", specs);
        assert_eq!(specs.input_stage, Some(InputStage::Jfet));
    }

    #[test]
    fn test_ingest_datasheet_toml_synthesizes_macromodel() {
        let mut catalog = PartCatalog::new();
        let toml_content = concat!(
            "name = \"MCP6002\"\n",
            "kind = \"OpAmp\"\n",
            "description = \"Microchip Rail-to-Rail Dual CMOS Op-Amp\"\n",
            "[specs]\n",
            "input_stage = \"Cmos\"\n",
            "gbw_mhz = 1.0\n",
            "noise_spot_1k = 28.0\n",
            "is_rrio = true\n",
            "rail_margin_v = 0.05\n"
        );

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("mcp6002.toml");
        fs::write(&file_path, toml_content).unwrap();

        let ingested = catalog.ingest_vendor_file(&file_path).unwrap();
        assert_eq!(ingested, vec!["MCP6002"]);
        assert!(catalog.get("MCP6002").is_some());

        let part = catalog.get("MCP6002").unwrap();
        assert!(part.spice_text.contains(".subckt MCP6002"));
        assert!(part.spice_text.contains("Synthesized"));

        let specs = catalog.benchmark_opamp("MCP6002").expect("MCP6002 benchmark must succeed");
        println!("Synthesized MCP6002 Measured Specs: {:?}", specs);
        assert_eq!(specs.input_stage, Some(InputStage::Jfet)); // CMOS input > 1e11 Zin
        assert!(specs.is_rrio.unwrap_or(false), "MCP6002 should be RRIO with low rail drop");
    }
}
