use crate::circuit::{Circuit, ComponentType, NODE_GND, NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE};
use crate::preset::{ObjectiveVector, Preset, ProbeType};
use crate::spice::{run_dc_operating_point, run_simulation, AcPoint, SpiceError, TranPoint};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DESCRIPTOR_DIM: usize = 11;
pub type BehaviorDescriptor = [f64; DESCRIPTOR_DIM];

#[derive(Debug)]
pub enum FitnessError {
    Spice(SpiceError),
    EmptySimulationResult,
    MissingAcData,
    MissingTranData,
    MissingProbeData,
    FeasibilityRailSaturation(f64),
    FeasibilityDcOffset(f64),
    FeasibilityBjtCutoff(String),
    FeasibilityBjtSaturation(String),
}

impl std::fmt::Display for FitnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FitnessError::Spice(e) => write!(f, "SPICE error: {}", e),
            FitnessError::EmptySimulationResult => write!(f, "Simulation returned empty output"),
            FitnessError::MissingAcData => write!(f, "Missing AC frequency data"),
            FitnessError::MissingTranData => write!(f, "Missing transient waveform data"),
            FitnessError::MissingProbeData => write!(f, "Missing probe AC data"),
            FitnessError::FeasibilityRailSaturation(v) => {
                write!(f, "Feasibility failed: output saturated at rail ({:.2}V)", v)
            }
            FitnessError::FeasibilityDcOffset(v) => {
                write!(f, "Feasibility failed: excessive DC offset ({:.2}V)", v)
            }
            FitnessError::FeasibilityBjtCutoff(msg) => {
                write!(f, "Feasibility failed: BJT in cutoff: {}", msg)
            }
            FitnessError::FeasibilityBjtSaturation(msg) => {
                write!(f, "Feasibility failed: BJT in saturation: {}", msg)
            }
        }
    }
}

impl std::error::Error for FitnessError {}

impl From<SpiceError> for FitnessError {
    fn from(err: SpiceError) -> Self {
        FitnessError::Spice(err)
    }
}

/// Generate comprehensive characterization SPICE netlist:
/// 1. OP point (.op print allv)
/// 2. AC frequency response (10Hz to 100kHz) -> wrdata ac_out.txt
/// 3. Small-signal TRAN (1kHz 100mV sine, 5ms) -> wrdata tran_small.txt
/// 4. Large-signal TRAN (1kHz 2.0V sine, 5ms) -> wrdata tran_large.txt
/// 5. Zero-input TRAN (0V, 10ms for self-oscillation) -> wrdata tran_zero.txt
pub fn to_characterization_netlist(circuit: &Circuit, title: &str, stray_cap_pf: f64) -> String {
    let mut netlist = String::new();

    netlist.push_str(&format!("* Characterization Netlist: {}\n", title));
    netlist.push_str(standard_spice_headers());

    // Default V_in with AC and initial small-signal sine (100mV)
    netlist.push_str(&format!(
        "V_in {} {} dc 0 ac 1 sin(0 0.1 1k)\n",
        NODE_IN, NODE_GND
    ));
    netlist.push_str(&format!("V_cc {} {} dc {:.2}\n", NODE_VCC, NODE_GND, circuit.vcc));
    netlist.push_str(&format!("V_ee {} {} dc {:.2}\n", NODE_VEE, NODE_GND, circuit.vee));

    for comp in &circuit.components {
        let prefix = comp.comp_type.to_char();
        let nodes_str = comp
            .nodes
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" ");

        netlist.push_str(&format!("{}{:<4} {} {}\n", prefix, comp.id, nodes_str, comp.value));
    }

    if stray_cap_pf > 0.0 {
        let mut id_counter = 8000;
        for &node in &circuit.nodes {
            if node != NODE_GND && node != NODE_VCC && node != NODE_VEE {
                netlist.push_str(&format!(
                    "C_stray_{} {} {} {:.2}pF\n",
                    id_counter, node, NODE_GND, stray_cap_pf
                ));
                id_counter += 1;
            }
        }
    }

    // Single .control block executing all analyses sequentially in ONE ngspice process
    netlist.push_str(".control\n");
    netlist.push_str("op\n");
    netlist.push_str("print allv\n");
    netlist.push_str("ac dec 50 10 100k\n");
    netlist.push_str(&format!("wrdata ac_out.txt v({})\n", NODE_OUT));
    netlist.push_str("tran 10us 5ms\n");
    netlist.push_str(&format!("wrdata tran_small.txt v({}) v({})\n", NODE_OUT, NODE_IN));
    netlist.push_str("alter @v_in[sin] = [ 0 2.0 1k ]\n");
    netlist.push_str("tran 10us 5ms\n");
    netlist.push_str(&format!("wrdata tran_large.txt v({}) v({})\n", NODE_OUT, NODE_IN));
    netlist.push_str("alter @v_in[sin] = [ 0 0 0 ]\n");
    netlist.push_str("tran 20us 10ms\n");
    netlist.push_str(&format!("wrdata tran_zero.txt v({})\n", NODE_OUT));
    netlist.push_str("quit\n");
    netlist.push_str(".endc\n");
    netlist.push_str(".end\n");

    netlist
}

pub fn standard_spice_headers() -> &'static str {
    ".include \"tl072.sub\"\n\
     .model 1N4148 D(is=2.52n rs=0.568 n=1.752 cjo=4p m=0.4 tt=20n)\n\
     .model 2N3904 NPN(Is=6.734f Xti=3 Eg=1.11 Vaf=74.03 Bf=416.4 Ne=1.259 Ise=6.734f Ikf=66.78m Xtb=1.5 Br=.7371 Nc=2 Isc=0 Ikr=0 Rc=1 Cjc=3.638p Mjc=.3085 Vjc=.75 Fc=.5 Cje=4.493p Mje=.2593 Vje=.75 Tr=239.5n Tf=301.2p Itf=.4 Vtf=4 Xtf=2 Rb=10)\n\
     .model 2N3906 PNP(Is=1.41f Xti=3 Eg=1.11 Vaf=18.7 Bf=180.7 Ne=1.5 Ise=0 Ikf=80m Xtb=1.5 Br=4.977 Nc=2 Isc=0 Ikr=0 Rc=2 Cjc=4.5p Mjc=.3 Vjc=.75 Fc=.5 Cje=5p Mje=.3 Vje=.75 Tr=50n Tf=300p Itf=.4 Vtf=4 Xtf=2 Rb=10)\n"
}

/// Generate minimal DC operating point netlist for fast feasibility gating
pub fn to_op_netlist(circuit: &Circuit) -> String {
    let mut netlist = String::new();

    netlist.push_str("* Feasibility OP Netlist\n");
    netlist.push_str(standard_spice_headers());

    netlist.push_str(&format!("V_in {} {} dc 0\n", NODE_IN, NODE_GND));
    netlist.push_str(&format!("V_cc {} {} dc {:.2}\n", NODE_VCC, NODE_GND, circuit.vcc));
    netlist.push_str(&format!("V_ee {} {} dc {:.2}\n", NODE_VEE, NODE_GND, circuit.vee));

    for comp in &circuit.components {
        let prefix = comp.comp_type.to_char();
        let nodes_str = comp
            .nodes
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" ");

        netlist.push_str(&format!("{}{:<4} {} {}\n", prefix, comp.id, nodes_str, comp.value));
    }

    netlist.push_str(".control\n");
    netlist.push_str("op\n");
    netlist.push_str("print allv\n");
    netlist.push_str("quit\n");
    netlist.push_str(".endc\n");
    netlist.push_str(".end\n");

    netlist
}

/// Generate pluggable probe AC netlist for target evaluation
pub fn to_probe_netlist(circuit: &Circuit, preset: &Preset, stray_cap_pf: f64) -> String {
    let mut netlist = String::new();

    netlist.push_str(&format!("* Preset Probe Netlist: {}\n", preset.name));
    netlist.push_str(standard_spice_headers());

    netlist.push_str(&format!(
        "V_in {} {} dc 0 ac 1\n",
        NODE_IN, NODE_GND
    ));
    netlist.push_str(&format!("V_cc {} {} dc {:.2}\n", NODE_VCC, NODE_GND, circuit.vcc));
    netlist.push_str(&format!("V_ee {} {} dc {:.2}\n", NODE_VEE, NODE_GND, circuit.vee));

    for comp in &circuit.components {
        let prefix = comp.comp_type.to_char();
        let nodes_str = comp
            .nodes
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" ");

        netlist.push_str(&format!("{}{:<4} {} {}\n", prefix, comp.id, nodes_str, comp.value));
    }

    // Connect R_load from NODE_OUT to NODE_GND if specified in probes
    let mut r_load = 10000.0;
    for p in &preset.probes {
        if p.condition.r_load > 0.0 {
            r_load = p.condition.r_load;
            break;
        }
    }
    netlist.push_str(&format!("R_probe_load {} {} {:.2}\n", NODE_OUT, NODE_GND, r_load));

    if stray_cap_pf > 0.0 {
        let mut id_counter = 8000;
        for &node in &circuit.nodes {
            if node != NODE_GND && node != NODE_VCC && node != NODE_VEE {
                netlist.push_str(&format!(
                    "C_stray_{} {} {} {:.2}pF\n",
                    id_counter, node, NODE_GND, stray_cap_pf
                ));
                id_counter += 1;
            }
        }
    }

    // Compute AC transfer function and input impedance
    netlist.push_str(".control\n");
    netlist.push_str("op\n");
    netlist.push_str("print allv\n");
    netlist.push_str("ac dec 10 100 100k\n");
    netlist.push_str(&format!("let gain = mag(v({})/v({}))\n", NODE_OUT, NODE_IN));
    netlist.push_str(&format!("let zin = mag(v({})/i(v_in))\n", NODE_IN));
    netlist.push_str("wrdata probe_ac.txt gain zin\n");
    netlist.push_str("quit\n");
    netlist.push_str(".endc\n");
    netlist.push_str(".end\n");

    netlist
}

/// Evaluate circuit against preset targets:
/// 1. Feasibility Gate (.op Fast-Drop):
///    - Output rail saturation check
///    - DC offset check
///    - BJT Active Region check (Vbe >= 0.45V, Vce >= 0.15V)
/// 2. Pluggable AC Probe Simulation
/// 3. Returns ObjectiveVector with normalized scores and scalarized fitness
pub fn evaluate_preset(
    circuit: &Circuit,
    preset: &Preset,
    stray_cap_pf: f64,
    timeout: Duration,
) -> Result<ObjectiveVector, FitnessError> {
    // 1. Feasibility Gate (.op Fast-Drop)
    let op_netlist = to_op_netlist(circuit);
    let dc_nodes = run_dc_operating_point(&op_netlist, timeout)?;

    let v_out = dc_nodes
        .get(&NODE_OUT.to_string())
        .or_else(|| dc_nodes.get(&format!("v({})", NODE_OUT)))
        .or_else(|| dc_nodes.get("out"))
        .copied()
        .unwrap_or(0.0);

    // Check output rail saturation
    if v_out >= circuit.vcc - preset.feasibility_rail_margin
        || v_out <= circuit.vee + preset.feasibility_rail_margin
    {
        return Err(FitnessError::FeasibilityRailSaturation(v_out));
    }

    // Check excessive DC offset
    if v_out.abs() > preset.feasibility_max_dc {
        return Err(FitnessError::FeasibilityDcOffset(v_out));
    }

    // In discrete active missions, circuit must contain at least one active transistor
    if !preset.allow_opamps && !circuit.components.iter().any(|c| c.comp_type == ComponentType::Q) {
        return Err(FitnessError::FeasibilityBjtCutoff("Discrete active mission requires at least one active transistor".to_string()));
    }

    // Check BJT active region for every transistor in circuit
    for comp in &circuit.components {
        if comp.comp_type == ComponentType::Q && comp.nodes.len() >= 3 {
            let vc = dc_nodes
                .get(&comp.nodes[0].to_string())
                .or_else(|| dc_nodes.get(&format!("v({})", comp.nodes[0])))
                .copied()
                .unwrap_or(0.0);
            let vb = dc_nodes
                .get(&comp.nodes[1].to_string())
                .or_else(|| dc_nodes.get(&format!("v({})", comp.nodes[1])))
                .copied()
                .unwrap_or(0.0);
            let ve = dc_nodes
                .get(&comp.nodes[2].to_string())
                .or_else(|| dc_nodes.get(&format!("v({})", comp.nodes[2])))
                .copied()
                .unwrap_or(0.0);

            if comp.value.to_uppercase().contains("3906") || comp.value.to_uppercase().contains("PNP") {
                let veb = ve - vb;
                let vec = ve - vc;
                if veb < 0.45 {
                    return Err(FitnessError::FeasibilityBjtCutoff(format!("Q{} (PNP) Veb={:.2}V < 0.45V", comp.id, veb)));
                }
                if vec < 0.15 {
                    return Err(FitnessError::FeasibilityBjtSaturation(format!("Q{} (PNP) Vec={:.2}V < 0.15V", comp.id, vec)));
                }
            } else {
                let vbe = vb - ve;
                let vce = vc - ve;
                if vbe < 0.45 {
                    return Err(FitnessError::FeasibilityBjtCutoff(format!("Q{} (NPN) Vbe={:.2}V < 0.45V", comp.id, vbe)));
                }
                if vce < 0.15 {
                    return Err(FitnessError::FeasibilityBjtSaturation(format!("Q{} (NPN) Vce={:.2}V < 0.15V", comp.id, vce)));
                }
            }
        }
    }

    // 2. Pluggable Simulation (AC Probe Sweep)
    let probe_netlist = to_probe_netlist(circuit, preset, stray_cap_pf);
    let sim_res = run_simulation(&probe_netlist, timeout)?;

    let probe_ac = sim_res.probe_ac.ok_or(FitnessError::MissingProbeData)?;
    if probe_ac.is_empty() {
        return Err(FitnessError::MissingProbeData);
    }

    let mut obj_vec = ObjectiveVector::new();

    for target in &preset.probes {
        let measured_val = match target.probe_type {
            ProbeType::Gain => {
                let target_freq = target.condition.freq;
                let closest = probe_ac
                    .iter()
                    .min_by(|a, b| {
                        (a.freq - target_freq)
                            .abs()
                            .partial_cmp(&(b.freq - target_freq).abs())
                            .unwrap()
                    })
                    .map(|p| p.gain)
                    .unwrap_or(0.0);
                closest
            }
            ProbeType::Zin => {
                let target_freq = target.condition.freq;
                let closest = probe_ac
                    .iter()
                    .min_by(|a, b| {
                        (a.freq - target_freq)
                            .abs()
                            .partial_cmp(&(b.freq - target_freq).abs())
                            .unwrap()
                    })
                    .map(|p| p.zin)
                    .unwrap_or(0.0);
                closest
            }
            ProbeType::Zout => 0.0,
            ProbeType::DcOffset => v_out.abs(),
            ProbeType::Bom => circuit.components.len() as f64,
        };

        let score = target.score(measured_val);
        obj_vec.add(&target.name, measured_val, score, target.weight);
    }

    Ok(obj_vec)
}

/// Extract 10D Non-linear Behavior Descriptor:
/// - 3 dimensions AC Frequency Shape: cutoff, rolloff, peak Q
/// - 5 dimensions Non-linear Transient: asym delta (100mV vs 2V), H2, H3, H5 harmonics, gain compression
/// - 2 dimensions Autonomous Dynamics: self-oscillation RMS, oscillation frequency
pub fn extract_behavior_descriptor(
    circuit: &Circuit,
    stray_cap_pf: f64,
    timeout: Duration,
) -> Result<BehaviorDescriptor, FitnessError> {
    let netlist = to_characterization_netlist(circuit, "Descriptor Run", stray_cap_pf);
    let sim_res = run_simulation(&netlist, timeout)?;

    let ac_data = sim_res.ac_response.ok_or(FitnessError::MissingAcData)?;
    let tran_small = sim_res.tran_small.ok_or(FitnessError::MissingTranData)?;
    let tran_large = sim_res.tran_large.ok_or(FitnessError::MissingTranData)?;
    let tran_zero = sim_res.tran_zero.unwrap_or_default();

    // 1. AC Features (4 dims: has_filter, cutoff, rolloff, peak_q)
    let (ac_has_filter, ac_cutoff_norm, ac_rolloff_norm, ac_peak_q_norm) = extract_ac_features(&ac_data);

    // 2. Non-linear Transient Features (5 dims: asym_delta, H2, H3, H5, compression)
    let (asym_delta, h2_ratio, h3_ratio, h5_ratio, compression) =
        extract_nonlinear_features(&tran_small, &tran_large);

    // 3. Autonomous Dynamics / Self-Oscillation Features (2 dims: osc_rms, osc_freq)
    let (osc_rms_norm, osc_freq_norm) = extract_oscillation_features(&tran_zero);

    Ok([
        ac_has_filter,
        ac_cutoff_norm,
        ac_rolloff_norm,
        ac_peak_q_norm,
        asym_delta,
        h2_ratio,
        h3_ratio,
        h5_ratio,
        compression,
        osc_rms_norm,
        osc_freq_norm,
    ])
}

/// Extract normalized filter flag, cutoff frequency, roll-off slope, and peak Q
pub fn extract_ac_features(ac: &[AcPoint]) -> (f64, f64, f64, f64) {
    if ac.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }

    let ref_db = ac[0].mag_db;
    let target_db = ref_db - 3.01;

    let mut cutoff_freq = 0.0;
    let mut cutoff_idx = 0;
    let mut has_cutoff = false;

    for i in 1..ac.len() {
        if ac[i].mag_db <= target_db {
            let p_prev = &ac[i - 1];
            let p_curr = &ac[i];
            let span = p_curr.mag_db - p_prev.mag_db;
            if span.abs() > 1e-6 {
                let frac = (target_db - p_prev.mag_db) / span;
                cutoff_freq = p_prev.freq + frac * (p_curr.freq - p_prev.freq);
            } else {
                cutoff_freq = p_curr.freq;
            }
            cutoff_idx = i;
            has_cutoff = true;
            break;
        }
    }

    let has_filter = if has_cutoff { 1.0 } else { 0.0 };
    let cutoff_norm = if has_cutoff {
        (cutoff_freq.clamp(10.0, 100_000.0).log10() / 5.0).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let mut rolloff_norm = 0.0;
    if has_cutoff && cutoff_idx < ac.len() - 1 {
        let last_pt = &ac[ac.len() - 1];
        let dec_span = (last_pt.freq / cutoff_freq.max(1.0)).log10();
        if dec_span > 0.2 {
            let db_drop = last_pt.mag_db - target_db;
            let slope_db_per_dec = db_drop / dec_span;
            rolloff_norm = (slope_db_per_dec / -40.0).clamp(-0.5, 1.5);
        }
    }

    let max_mag_db = ac.iter().map(|p| p.mag_db).fold(f64::NEG_INFINITY, f64::max);
    let peak_boost = (max_mag_db - ref_db).max(0.0);
    let peak_q_norm = (peak_boost / 20.0).clamp(0.0, 1.5);

    (has_filter, cutoff_norm, rolloff_norm, peak_q_norm)
}

/// Interpolate voltage at exact time `t` from sorted TranPoint slice
pub fn interpolate_tran(pts: &[TranPoint], t: f64) -> f64 {
    if pts.is_empty() {
        return 0.0;
    }
    if t <= pts[0].time {
        return pts[0].v_out;
    }
    if t >= pts[pts.len() - 1].time {
        return pts[pts.len() - 1].v_out;
    }

    match pts.binary_search_by(|p| p.time.partial_cmp(&t).unwrap_or(std::cmp::Ordering::Equal)) {
        Ok(i) => pts[i].v_out,
        Err(i) => {
            let p0 = &pts[i - 1];
            let p1 = &pts[i];
            let dt = p1.time - p0.time;
            if dt > 1e-12 {
                let frac = (t - p0.time) / dt;
                p0.v_out + frac * (p1.v_out - p0.v_out)
            } else {
                p0.v_out
            }
        }
    }
}

/// Extract H2, H3, H5 harmonic ratios relative to fundamental with Hann windowing over integer periods
pub fn extract_harmonics_resampled(
    pts: &[TranPoint],
    f0: f64,
    t_start: f64,
    n_cycles: f64,
    num_samples: usize,
) -> (f64, f64, f64) {
    if pts.len() < 10 {
        return (0.0, 0.0, 0.0);
    }

    let t_end = t_start + n_cycles / f0;
    let dt = (t_end - t_start) / (num_samples as f64);

    let mut samples = Vec::with_capacity(num_samples);
    for i in 0..num_samples {
        let t = t_start + (i as f64) * dt;
        samples.push(interpolate_tran(pts, t));
    }

    let v_dc: f64 = samples.iter().sum::<f64>() / (num_samples as f64);

    let mut w_sum = 0.0;
    let mut a1 = 0.0;
    let mut b1 = 0.0;
    let mut a2 = 0.0;
    let mut b2 = 0.0;
    let mut a3 = 0.0;
    let mut b3 = 0.0;
    let mut a5 = 0.0;
    let mut b5 = 0.0;

    for (i, &s) in samples.iter().enumerate() {
        let frac = (i as f64) / (num_samples as f64);
        // Hann window: 0.5 * (1 - cos(2*pi*frac))
        let w = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * frac).cos());
        w_sum += w;

        let v = (s - v_dc) * w;
        let phi1 = 2.0 * std::f64::consts::PI * n_cycles * frac;
        a1 += v * phi1.cos();
        b1 += v * phi1.sin();

        let phi2 = 2.0 * std::f64::consts::PI * (2.0 * n_cycles) * frac;
        a2 += v * phi2.cos();
        b2 += v * phi2.sin();

        let phi3 = 2.0 * std::f64::consts::PI * (3.0 * n_cycles) * frac;
        a3 += v * phi3.cos();
        b3 += v * phi3.sin();

        let phi5 = 2.0 * std::f64::consts::PI * (5.0 * n_cycles) * frac;
        a5 += v * phi5.cos();
        b5 += v * phi5.sin();
    }

    if w_sum < 1e-6 {
        return (0.0, 0.0, 0.0);
    }

    let p1 = a1 * a1 + b1 * b1;
    let p2 = a2 * a2 + b2 * b2;
    let p3 = a3 * a3 + b3 * b3;
    let p5 = a5 * a5 + b5 * b5;

    if p1 < 1e-8 {
        return (0.0, 0.0, 0.0);
    }

    let h2 = (p2 / p1).sqrt();
    let h3 = (p3 / p1).sqrt();
    let h5 = (p5 / p1).sqrt();

    // Noise floor threshold: anything below 0.0001 (-80dB) is treated as true linear zero
    let clean_h2 = if h2 > 0.0001 { h2.min(1.0) } else { 0.0 };
    let clean_h3 = if h3 > 0.0001 { h3.min(1.0) } else { 0.0 };
    let clean_h5 = if h5 > 0.0001 { h5.min(1.0) } else { 0.0 };

    (clean_h2, clean_h3, clean_h5)
}

/// Calculate asymmetry index in [-1.0, 1.0] for a waveform
fn calc_asymmetry(pts: &[&TranPoint]) -> f64 {
    let v_pos = pts.iter().map(|p| p.v_out).fold(f64::NEG_INFINITY, f64::max);
    let v_neg = pts.iter().map(|p| p.v_out).fold(f64::INFINITY, f64::min);
    let span = v_pos.abs() + v_neg.abs();
    if span > 0.01 {
        ((v_pos - v_neg.abs()) / span).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Extract non-linear features from small-signal (100mV) and large-signal (2V) transient data:
/// 1. Asymmetry delta: |asym(2V) - asym(100mV)|
/// 2. H2 ratio (2nd harmonic / fundamental)
/// 3. H3 ratio (3rd harmonic / fundamental)
/// 4. H5 ratio (5th harmonic / fundamental)
/// 5. Gain compression: 1.0 - (G_large / G_small)
pub fn extract_nonlinear_features(
    small: &[TranPoint],
    large: &[TranPoint],
) -> (f64, f64, f64, f64, f64) {
    let steady_small: Vec<&TranPoint> = small.iter().filter(|p| p.time >= 0.003).collect();
    let steady_large: Vec<&TranPoint> = large.iter().filter(|p| p.time >= 0.003).collect();

    if steady_small.len() < 10 || steady_large.len() < 10 {
        return (0.0, 0.0, 0.0, 0.0, 0.0);
    }

    // 1. Asymmetry delta
    let asym_small = calc_asymmetry(&steady_small);
    let asym_large = calc_asymmetry(&steady_large);
    let asym_delta = (asym_large - asym_small).abs().clamp(0.0, 1.0);

    // 2. Resampled Hann-windowed harmonic extraction on 2V drive (2 cycles: 3ms to 5ms)
    let (h2_ratio, h3_ratio, h5_ratio) =
        extract_harmonics_resampled(large, 1000.0, 0.003, 2.0, 256);

    // 3. Gain compression
    let v_pos_s = steady_small.iter().map(|p| p.v_out).fold(f64::NEG_INFINITY, f64::max);
    let v_neg_s = steady_small.iter().map(|p| p.v_out).fold(f64::INFINITY, f64::min);
    let vpp_small = (v_pos_s - v_neg_s).max(0.0);
    let g_small = vpp_small / 0.2; // 0.1V peak = 0.2Vpp

    let v_pos_l = steady_large.iter().map(|p| p.v_out).fold(f64::NEG_INFINITY, f64::max);
    let v_neg_l = steady_large.iter().map(|p| p.v_out).fold(f64::INFINITY, f64::min);
    let vpp_large = (v_pos_l - v_neg_l).max(0.0);
    let g_large = vpp_large / 4.0; // 2.0V peak = 4.0Vpp

    let compression = if g_small > 1e-3 {
        (1.0 - (g_large / g_small)).clamp(0.0, 1.0)
    } else {
        0.0
    };

    (asym_delta, h2_ratio, h3_ratio, h5_ratio, compression)
}

/// Extract autonomous dynamics: self-oscillation RMS voltage and frequency
pub fn extract_oscillation_features(tran: &[TranPoint]) -> (f64, f64) {
    let steady_pts: Vec<&TranPoint> = tran.iter().filter(|p| p.time >= 0.005).collect();

    if steady_pts.len() < 10 {
        return (0.0, 0.0);
    }

    let n = steady_pts.len() as f64;
    let v_dc: f64 = steady_pts.iter().map(|p| p.v_out).sum::<f64>() / n;
    let v_ac_var: f64 = steady_pts
        .iter()
        .map(|p| (p.v_out - v_dc).powi(2))
        .sum::<f64>()
        / n;
    let v_rms = v_ac_var.sqrt();

    if v_rms < 0.02 {
        return (0.0, 0.0);
    }

    let osc_rms_norm = (v_rms / 5.0).clamp(0.0, 1.0);

    let mut crossings = 0;
    let mut prev_sign = steady_pts[0].v_out - v_dc >= 0.0;
    for p in steady_pts.iter().skip(1) {
        let curr_sign = p.v_out - v_dc >= 0.0;
        if curr_sign != prev_sign {
            crossings += 1;
            prev_sign = curr_sign;
        }
    }

    let t_start = steady_pts.first().unwrap().time;
    let t_end = steady_pts.last().unwrap().time;
    let t_span = (t_end - t_start).max(1e-4);

    let freq = (crossings as f64) / (2.0 * t_span);
    let osc_freq_norm = if freq > 1.0 {
        (freq.clamp(1.0, 100_000.0).log10() / 5.0).clamp(0.0, 1.0)
    } else {
        0.0
    };

    (osc_rms_norm, osc_freq_norm)
}

/// Calculate Euclidean distance between two 10D behavior descriptors
pub fn euclidean_distance(a: &BehaviorDescriptor, b: &BehaviorDescriptor) -> f64 {
    let mut sum = 0.0;
    for i in 0..DESCRIPTOR_DIM {
        let diff = a[i] - b[i];
        sum += diff * diff;
    }
    sum.sqrt()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub circuit: Circuit,
    pub descriptor: BehaviorDescriptor,
    #[serde(default)]
    pub mc_dev_db: Option<f64>,
    #[serde(default)]
    pub generation: usize,
    #[serde(default)]
    pub fitness: Option<f64>,
    #[serde(default)]
    pub obj_summary: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NoveltyArchive {
    pub entries: Vec<ArchiveEntry>,
}

impl NoveltyArchive {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Calculate distance to the single nearest neighbor in the archive.
    pub fn min_distance(&self, descriptor: &BehaviorDescriptor) -> f64 {
        self.entries
            .iter()
            .map(|entry| euclidean_distance(descriptor, &entry.descriptor))
            .fold(f64::INFINITY, f64::min)
    }

    /// Calculate average Euclidean distance to the k nearest neighbors.
    pub fn novelty_score(&self, descriptor: &BehaviorDescriptor, k: usize) -> f64 {
        if self.entries.is_empty() {
            return 10.0;
        }

        let mut distances: Vec<f64> = self
            .entries
            .iter()
            .map(|entry| euclidean_distance(descriptor, &entry.descriptor))
            .collect();

        distances.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let count = k.min(distances.len());
        if count == 0 {
            return 10.0;
        }

        let sum: f64 = distances.iter().take(count).sum();
        sum / count as f64
    }

    /// Attempt to add circuit to archive with full discovery metadata
    pub fn maybe_add_with_meta(
        &mut self,
        circuit: Circuit,
        descriptor: BehaviorDescriptor,
        threshold: f64,
        k: usize,
        min_dist: f64,
        mc_dev_db: Option<f64>,
        generation: usize,
        fitness: Option<f64>,
        obj_summary: Option<String>,
    ) -> bool {
        if self.entries.is_empty() {
            self.entries.push(ArchiveEntry {
                circuit,
                descriptor,
                mc_dev_db,
                generation,
                fitness,
                obj_summary,
            });
            return true;
        }

        let nearest = self.min_distance(&descriptor);
        if nearest < min_dist {
            return false;
        }

        if self.entries.len() < k || self.novelty_score(&descriptor, k) >= threshold {
            self.entries.push(ArchiveEntry {
                circuit,
                descriptor,
                mc_dev_db,
                generation,
                fitness,
                obj_summary,
            });
            true
        } else {
            false
        }
    }

    /// Attempt to add circuit to archive if:
    /// 1. Distance to closest neighbor >= min_dist (rejects duplicates & near-clones)
    /// 2. Average k-NN novelty score >= threshold (or archive has < k entries)
    pub fn maybe_add(
        &mut self,
        circuit: Circuit,
        descriptor: BehaviorDescriptor,
        threshold: f64,
        k: usize,
        min_dist: f64,
        mc_dev_db: Option<f64>,
    ) -> bool {
        self.maybe_add_with_meta(circuit, descriptor, threshold, k, min_dist, mc_dev_db, 0, None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Circuit, Component, NODE_GND, NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE};

    /// Standard 1kHz Sallen-Key Lowpass Filter (R=10k, C=15.9nF)
    fn build_sallen_key_standard() -> Circuit {
        let mut c = Circuit::new();
        c.add_component(Component::new('R', 1, vec![NODE_IN, 10], "10k").unwrap());
        c.add_component(Component::new('R', 2, vec![10, 20], "10k").unwrap());
        c.add_component(Component::new('C', 1, vec![10, NODE_OUT], "15.9nF").unwrap());
        c.add_component(Component::new('C', 2, vec![20, NODE_GND], "15.9nF").unwrap());
        c.add_component(
            Component::new('X', 1, vec![20, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );
        c.validate().unwrap();
        c
    }

    /// Sallen-Key Lowpass Filter variant (R=12k, C=13.3nF -> fc ≈ 997Hz)
    fn build_sallen_key_variant() -> Circuit {
        let mut c = Circuit::new();
        c.add_component(Component::new('R', 1, vec![NODE_IN, 10], "12k").unwrap());
        c.add_component(Component::new('R', 2, vec![10, 20], "12k").unwrap());
        c.add_component(Component::new('C', 1, vec![10, NODE_OUT], "13.3nF").unwrap());
        c.add_component(Component::new('C', 2, vec![20, NODE_GND], "13.3nF").unwrap());
        c.add_component(
            Component::new('X', 1, vec![20, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );
        c.validate().unwrap();
        c
    }

    /// Asymmetric Diode Clipper (Resistor + 1N4148 Diode + TL072 Buffer)
    fn build_diode_clipper() -> Circuit {
        let mut c = Circuit::new();
        c.add_component(Component::new('R', 1, vec![NODE_IN, 10], "1k").unwrap());
        c.add_component(Component::new('D', 1, vec![10, NODE_GND], "1N4148").unwrap());
        c.add_component(
            Component::new('X', 1, vec![10, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );
        c.validate().unwrap();
        c
    }

    /// Acceptance Criterion:
    /// 1. Sallen-Key variants must produce almost identical descriptors (distance < 0.15).
    /// 2. Sallen-Key vs Diode Clipper must produce strongly divergent descriptors (distance > 1.0).
    /// 3. NoveltyArchive rejects redundant filter variant, but admits novel Diode Clipper!
    #[test]
    fn test_10d_novelty_differentiates_filter_vs_nonlinear_clipper() {
        let sk_std = build_sallen_key_standard();
        let sk_var = build_sallen_key_variant();
        let diode = build_diode_clipper();

        let timeout = Duration::from_secs(3);

        let desc_std = extract_behavior_descriptor(&sk_std, 22.0, timeout)
            .expect("Failed to extract desc for standard Sallen-Key");
        let desc_var = extract_behavior_descriptor(&sk_var, 22.0, timeout)
            .expect("Failed to extract desc for variant Sallen-Key");
        let desc_diode = extract_behavior_descriptor(&diode, 22.0, timeout)
            .expect("Failed to extract desc for Diode Clipper");

        println!("Standard Sallen-Key 10D Descriptor:\n  {:?}", desc_std);
        println!("Variant Sallen-Key 10D Descriptor:\n  {:?}", desc_var);
        println!("Diode Clipper 10D Descriptor:\n  {:?}", desc_diode);

        let dist_variants = euclidean_distance(&desc_std, &desc_var);
        let dist_diode_vs_sk = euclidean_distance(&desc_std, &desc_diode);

        println!("Distance (SK std vs SK var): {:.4}", dist_variants);
        println!("Distance (SK std vs Diode):  {:.4}", dist_diode_vs_sk);

        // Linear filter variants must be clustered tightly together (< 0.15)
        assert!(
            dist_variants < 0.15,
            "Filter variants should be very close (< 0.15), got {:.4}",
            dist_variants
        );

        // Diode clipper with non-linear distortion, asymmetry, and compression must be distant (> 1.0)
        assert!(
            dist_diode_vs_sk > 1.0,
            "Diode clipper must be distant from linear filter (> 1.0), got {:.4}",
            dist_diode_vs_sk
        );

        // Verification of AC filter action:
        assert_eq!(desc_std[0], 1.0, "Standard Sallen-Key must be detected as a filter (has_filter=1.0)");
        assert_eq!(desc_var[0], 1.0, "Variant Sallen-Key must be detected as a filter (has_filter=1.0)");
        assert_eq!(desc_diode[0], 0.0, "Diode clipper must NOT be detected as a filter (has_filter=0.0)");
        assert_eq!(desc_diode[1], 0.0, "Diode clipper cutoff must be 0.0 (no saturation at 1.0)");

        // Verification of non-linear dimensions on Diode Clipper:
        // asym_delta > 0.3
        assert!(
            desc_diode[4] > 0.3,
            "Diode clipper must have large asymmetry delta (> 0.3), got {:.4}",
            desc_diode[4]
        );
        // H2 (even harmonic) > 0.1
        assert!(
            desc_diode[5] > 0.1,
            "Diode clipper must exhibit strong H2 even harmonic (> 0.1), got {:.4}",
            desc_diode[5]
        );
        // True physical spectral decay: H2 > H3 > H5
        assert!(
            desc_diode[5] > desc_diode[6],
            "Physical harmonic decay violated: H2 ({:.4}) must be > H3 ({:.4})",
            desc_diode[5], desc_diode[6]
        );
        assert!(
            desc_diode[6] > desc_diode[7],
            "Physical harmonic decay violated: H3 ({:.4}) must be > H5 ({:.4})",
            desc_diode[6], desc_diode[7]
        );

        // Linear filter harmonics must be clean (no artificial floor at 0.0098)
        assert!(
            desc_std[5] < 0.005,
            "Linear filter H2 must be clean (< 0.005), got {:.4}",
            desc_std[5]
        );
        assert!(
            desc_std[6] < 0.005,
            "Linear filter H3 must be clean (< 0.005), got {:.4}",
            desc_std[6]
        );
        assert!(
            desc_std[7] < 0.005,
            "Linear filter H5 must be clean (< 0.005), got {:.4}",
            desc_std[7]
        );

        // Compression > 0.1
        assert!(
            desc_diode[8] > 0.1,
            "Diode clipper must exhibit gain compression (> 0.1), got {:.4}",
            desc_diode[8]
        );

        // Novelty Archive test
        let mut archive = NoveltyArchive::new();
        let k = 1;
        let threshold = 0.5;

        // Seed with standard Sallen-Key
        assert!(archive.maybe_add(sk_std.clone(), desc_std, threshold, k, 0.25, None));
        assert_eq!(archive.len(), 1);

        // Sallen-Key variant must be rejected (novelty < 0.5 and min_dist < 0.25)
        let score_var = archive.novelty_score(&desc_var, k);
        println!("Novelty score for SK variant: {:.4} (threshold={})", score_var, threshold);
        assert!(score_var < threshold);
        assert!(!archive.maybe_add(sk_var, desc_var, threshold, k, 0.25, None));
        assert_eq!(archive.len(), 1);

        // Diode clipper must be accepted (novelty >= 0.5 and min_dist >= 0.25)
        let score_diode = archive.novelty_score(&desc_diode, k);
        println!("Novelty score for Diode Clipper: {:.4} (threshold={})", score_diode, threshold);
        assert!(score_diode >= threshold);
        assert!(archive.maybe_add(diode, desc_diode, threshold, k, 0.25, None));
        assert_eq!(archive.len(), 2);
    }

    #[test]
    fn test_opamp_buffer_harmonics_sanity() {
        let mut c = Circuit::new();
        c.add_component(
            Component::new('X', 1, vec![NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );
        c.validate().unwrap();

        let timeout = Duration::from_secs(3);
        let desc = extract_behavior_descriptor(&c, 0.0, timeout).expect("Buffer sim failed");
        println!("Opamp Buffer 11D Descriptor:\n  {:?}", desc);
        println!("Buffer H2: {:.6}, H3: {:.6}, H5: {:.6}", desc[5], desc[6], desc[7]);
    }

    #[test]
    fn test_buffer_preset_evaluation() {
        let mut c = Circuit::new();
        c.add_component(
            Component::new('X', 1, vec![NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );
        c.validate().unwrap();

        let mut preset = Preset::buffer_default();
        preset.allow_opamps = true;
        let timeout = Duration::from_secs(3);
        let obj_vec = evaluate_preset(&c, &preset, 0.0, timeout).expect("Buffer preset evaluation failed");

        println!("Buffer Objective Vector:\n  {}", obj_vec.summary());
        println!("Scalarized Fitness: {:.4}", obj_vec.scalarized());

        assert!(obj_vec.scalarized() > 0.90, "TL072 buffer should score very high on buffer preset (> 0.90)");
    }

    #[test]
    fn test_feasibility_gate_rejects_open_loop_opamp_saturation() {
        let mut c = Circuit::new();
        // Open-loop op-amp with IN+ tied to VCC (+9V) and IN- tied to GND (0V) -> Output firmly clamped to +9V rail!
        c.add_component(
            Component::new('R', 1, vec![NODE_IN, NODE_VCC], "10k").unwrap(),
        );
        c.add_component(
            Component::new('X', 1, vec![NODE_VCC, NODE_GND, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );
        c.validate().unwrap();

        let mut preset = Preset::buffer_default();
        preset.allow_opamps = true;
        let timeout = Duration::from_secs(3);
        let res = evaluate_preset(&c, &preset, 0.0, timeout);

        assert!(res.is_err(), "Open-loop op-amp saturated at rail MUST be rejected by feasibility gate!");
        match res.unwrap_err() {
            FitnessError::FeasibilityRailSaturation(v) => {
                println!("Confirmed Feasibility Gate rejected rail saturation at {:.2}V", v);
            }
            FitnessError::FeasibilityDcOffset(v) => {
                println!("Confirmed Feasibility Gate rejected excessive DC offset at {:.2}V", v);
                assert!(v >= 5.0, "Output should be saturated near rail, got {:.2}V", v);
            }
            other => panic!("Expected FeasibilityRailSaturation or FeasibilityDcOffset, got {:?}", other),
        }
    }

    #[test]
    fn test_feasibility_gate_rejects_bjt_cutoff() {
        let mut c = Circuit::new();
        // Unbiased BJT: Base tied to GND (0V), Emitter tied to GND (0V), Collector tied to VCC -> Vbe = 0V < 0.45V (CUTOFF!)
        c.add_component(
            Component::new('Q', 1, vec![NODE_VCC, NODE_GND, NODE_GND], "2N3904").unwrap(),
        );
        c.add_component(
            Component::new('R', 1, vec![NODE_IN, NODE_OUT], "10k").unwrap(),
        );
        c.validate().unwrap();

        let preset = Preset::buffer_default();
        let timeout = Duration::from_secs(3);
        let res = evaluate_preset(&c, &preset, 0.0, timeout);

        assert!(res.is_err(), "Cutoff BJT MUST be rejected by feasibility gate!");
        match res.unwrap_err() {
            FitnessError::FeasibilityBjtCutoff(msg) => {
                println!("Confirmed Feasibility Gate rejected BJT in cutoff: {}", msg);
            }
            other => panic!("Expected FeasibilityBjtCutoff, got {:?}", other),
        }
    }

    #[test]
    fn test_feasibility_gate_accepts_properly_biased_emitter_follower() {
        let mut c = Circuit::new();
        // Properly biased discrete NPN Emitter Follower:
        // Base bias: R1 (VCC to base=10) 100k, R2 (base=10 to VEE) 100k -> Vb ≈ 0V
        // Emitter resistor: Re (emitter=2 to VEE) 10k -> Ve ≈ -0.65V, Vbe ≈ 0.65V (ACTIVE!)
        // Collector tied to VCC (+9V) -> Vce = 9 - (-0.65) = 9.65V (ACTIVE!)
        // Input coupling: Cin (IN=1 to base=10) 1uF
        // Output taken from emitter=2 (NODE_OUT)
        c.add_component(Component::new('Q', 1, vec![NODE_VCC, 10, NODE_OUT], "2N3904").unwrap());
        c.add_component(Component::new('R', 1, vec![NODE_VCC, 10], "100k").unwrap());
        c.add_component(Component::new('R', 2, vec![10, NODE_VEE], "100k").unwrap());
        c.add_component(Component::new('R', 3, vec![NODE_OUT, NODE_VEE], "4.7k").unwrap());
        c.add_component(Component::new('C', 1, vec![NODE_IN, 10], "1uF").unwrap());
        c.validate().unwrap();

        let preset = Preset::buffer_default();
        let timeout = Duration::from_secs(3);
        let obj_vec = evaluate_preset(&c, &preset, 0.0, timeout).expect("Biased Emitter Follower must PASS feasibility gate!");

        println!("Emitter Follower Objective Vector:\n  {}", obj_vec.summary());
        println!("Emitter Follower Fitness: {:.6}", obj_vec.scalarized());
        assert!(obj_vec.scalarized() > 0.50, "Discrete emitter follower should score well (> 0.50)");
    }

    #[test]
    fn test_feasibility_gate_rejects_bjt_saturation() {
        let mut c = Circuit::new();
        // Saturated BJT: Base pulled hard to +9V through 1k, Collector tied to GND through 10k -> Vce ≈ 0.05V < 0.15V (SATURATION!)
        c.add_component(Component::new('Q', 1, vec![NODE_OUT, 10, NODE_GND], "2N3904").unwrap());
        c.add_component(Component::new('R', 1, vec![NODE_VCC, 10], "1k").unwrap());
        c.add_component(Component::new('R', 2, vec![NODE_VCC, NODE_OUT], "10k").unwrap());
        c.add_component(Component::new('R', 3, vec![NODE_IN, 10], "10k").unwrap());
        c.validate().unwrap();

        let preset = Preset::buffer_default();
        let timeout = Duration::from_secs(3);
        let res = evaluate_preset(&c, &preset, 0.0, timeout);

        assert!(res.is_err(), "Saturated BJT MUST be rejected by feasibility gate!");
        match res.unwrap_err() {
            FitnessError::FeasibilityBjtSaturation(msg) => {
                println!("Confirmed Feasibility Gate rejected BJT in saturation: {}", msg);
            }
            FitnessError::FeasibilityRailSaturation(v) => {
                println!("Confirmed Feasibility Gate rejected rail saturation at {:.2}V", v);
            }
            FitnessError::FeasibilityDcOffset(v) => {
                println!("Confirmed Feasibility Gate rejected excessive DC offset at {:.2}V", v);
            }
            other => panic!("Expected BJT saturation or DC failure, got {:?}", other),
        }
    }

    #[test]
    fn test_feasibility_gate_rejects_dead_rail_stuck_output() {
        let mut c = Circuit::new();
        // Dead circuit: Output tied directly to VCC (+9V) through 100 ohm resistor with input disconnected from output
        c.add_component(Component::new('R', 1, vec![NODE_VCC, NODE_OUT], "100").unwrap());
        c.add_component(Component::new('R', 2, vec![NODE_IN, NODE_GND], "10k").unwrap());
        c.validate().unwrap();

        let preset = Preset::buffer_default();
        let timeout = Duration::from_secs(3);
        let res = evaluate_preset(&c, &preset, 0.0, timeout);

        assert!(res.is_err(), "Rail-stuck circuit MUST be rejected by feasibility gate!");
        match res.unwrap_err() {
            FitnessError::FeasibilityRailSaturation(v) => {
                println!("Confirmed Feasibility Gate rejected rail saturation at {:.2}V", v);
                assert!(v >= 8.0, "Output should be near +9V rail, got {:.2}V", v);
            }
            FitnessError::FeasibilityDcOffset(v) => {
                println!("Confirmed Feasibility Gate rejected excessive DC offset at {:.2}V", v);
                assert!(v >= 5.0, "DC offset should be high, got {:.2}V", v);
            }
            other => panic!("Expected rail saturation or DC offset error, got {:?}", other),
        }
    }
    #[test]
    fn test_discovery_4_parallel_pullup_dc_operating_point() {
        let mut c = Circuit::new();
        // Discovery #4 topology (Gen 20 winner):
        // Q1 (NPN 2N3904): c=VCC (+9V), b=10, e=OUT
        // R1 (100k) from VCC to 10
        // R2 (100k) from 10 to VEE (-9V)
        // R3 (4.3k) from OUT to VEE (-9V)
        // C1 (1uF) from IN to 10
        // R5 (300k) in parallel pull-up from VCC to 10
        // Effective pull-up: R_up = 100k || 300k = 75k Ohm
        // Open-circuit Thevenin: V_th = -9 + 18 * (100 / 175) = +1.2857V
        // Thevenin resistance: R_th = 75k || 100k = 42.857k Ohm
        // With base current I_b ≈ 12.6 uA -> V_drop = 0.54V -> V_B ≈ +0.72V
        // Output at emitter: V_E = V_B - V_be (0.68V) = +0.04V!
        c.add_component(Component::new('Q', 1, vec![NODE_VCC, 10, NODE_OUT], "2N3904").unwrap());
        c.add_component(Component::new('R', 1, vec![NODE_VCC, 10], "100k").unwrap());
        c.add_component(Component::new('R', 2, vec![10, NODE_VEE], "100k").unwrap());
        c.add_component(Component::new('R', 3, vec![NODE_OUT, NODE_VEE], "4.3k").unwrap());
        c.add_component(Component::new('C', 1, vec![NODE_IN, 10], "1uF").unwrap());
        c.add_component(Component::new('R', 5, vec![NODE_VCC, 10], "300k").unwrap());
        c.validate().unwrap();

        let op_netlist = to_op_netlist(&c);
        let timeout = Duration::from_secs(3);
        let dc_nodes = run_dc_operating_point(&op_netlist, timeout).expect("OP simulation failed");

        let v_b = *dc_nodes.get("10").or_else(|| dc_nodes.get("v(10)")).expect("Node 10 missing");
        let v_out = *dc_nodes.get(&NODE_OUT.to_string()).or_else(|| dc_nodes.get(&format!("v({})", NODE_OUT))).expect("Node OUT missing");

        println!("Discovery #4 DC Analysis: V_Base = {:.4}V, V_Out = {:.4}V", v_b, v_out);

        // Assert that Base is ~ +0.72V and Output is ~ +0.04V (exactly cancelling Vbe!)
        assert!((v_b - 0.723).abs() < 0.05, "V_Base should be ~ +0.72V, got {:.4}V", v_b);
        assert!((v_out - 0.040).abs() < 0.05, "V_Out should be ~ +0.04V, got {:.4}V", v_out);
    }

    #[test]
    fn test_bootstrapped_discrete_follower_high_zin() {
        let mut c = Circuit::new();
        // Bootstrapped Discrete Emitter Follower:
        // Q1 (2N3904): c=VCC, b=10, e=OUT
        // Bias divider: R1 (56k, VCC to 20), R2 (100k, 20 to VEE)
        // Isolation resistor: R_iso (150k, 20 to 10)
        // Bootstrap capacitor: C_boot (10uF, OUT to 20)
        // Emitter load: R_e (10k, OUT to VEE)
        // Input coupling: C_in (1uF, IN to 10)
        c.add_component(Component::new('Q', 1, vec![NODE_VCC, 10, NODE_OUT], "2N3904").unwrap());
        c.add_component(Component::new('R', 1, vec![NODE_VCC, 20], "56k").unwrap());
        c.add_component(Component::new('R', 2, vec![20, NODE_VEE], "100k").unwrap());
        c.add_component(Component::new('R', 3, vec![NODE_OUT, NODE_VEE], "10k").unwrap());
        c.add_component(Component::new('R', 4, vec![20, 10], "150k").unwrap());
        c.add_component(Component::new('C', 1, vec![NODE_IN, 10], "1uF").unwrap());
        c.add_component(Component::new('C', 2, vec![NODE_OUT, 20], "10uF").unwrap());
        c.validate().unwrap();

        let preset = Preset::buffer_default();
        let timeout = Duration::from_secs(3);
        // Test with real 22pF stray capacitance
        let obj_vec = evaluate_preset(&c, &preset, 22.0, timeout).expect("Bootstrapped follower evaluation failed");

        println!("Bootstrapped Follower Objectives (with 22pF stray):\n  {}", obj_vec.summary());
        println!("Bootstrapped Follower Fitness: {:.6}", obj_vec.scalarized());

        let zin_val = obj_vec.values[1]; // Zin@1kHz
        println!("Measured Zin@1kHz: {:.1} Ohm", zin_val);

        // Unbootstrapped Zin is limited to ~47k Ohm.
        // Bootstrapping boosts Zin > 600k Ohm (>12x improvement) even with 22pF stray capacitance!
        assert!(zin_val > 600_000.0, "Bootstrapped Zin should exceed 600k Ohm (12x higher than unbootstrapped 47k), got {:.1} Ohm", zin_val);
    }
}


