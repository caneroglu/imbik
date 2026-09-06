use crate::circuit::{Circuit, ComponentType, NODE_GND, NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE};
use crate::preset::{ObjectiveVector, Preset, ProbeType};
use crate::spice::{run_dc_operating_point, run_simulation, AcPoint, SpiceError, TranPoint};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    MissingNodeVoltage(usize),
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
            FitnessError::MissingNodeVoltage(node) => write!(
                f,
                "Operating point did not report a voltage for node {} - cannot judge feasibility",
                node
            ),
        }
    }
}

impl std::error::Error for FitnessError {}

impl From<SpiceError> for FitnessError {
    fn from(err: SpiceError) -> Self {
        FitnessError::Spice(err)
    }
}

/// Look up a DC node voltage from an `.op` dump.
///
/// Ground is the reference and is never printed by ngspice, so node 0 resolves to an
/// exact 0.0 V. Every other node MUST be present: these lookups previously fell back to
/// `unwrap_or(0.0)`, which meant a circuit whose operating point failed to print sailed
/// through the feasibility gate looking perfectly biased (0 V output, 0 V on every BJT
/// terminal). A missing node is now a hard rejection.
fn dc_node_voltage(dc_nodes: &HashMap<String, f64>, node: usize) -> Result<f64, FitnessError> {
    if node == NODE_GND {
        return Ok(0.0);
    }
    dc_nodes
        .get(&node.to_string())
        .or_else(|| dc_nodes.get(&format!("v({})", node)))
        .or_else(|| match node {
            NODE_IN => dc_nodes.get("in"),
            NODE_OUT => dc_nodes.get("out"),
            NODE_VCC => dc_nodes.get("vcc"),
            NODE_VEE => dc_nodes.get("vee"),
            _ => None,
        })
        .copied()
        .ok_or(FitnessError::MissingNodeVoltage(node))
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
    netlist.push_str("tran 1us 5ms 0 1us\n");
    netlist.push_str(&format!("wrdata tran_small.txt v({}) v({})\n", NODE_OUT, NODE_IN));
    netlist.push_str("alter @v_in[sin] = [ 0 2.0 1k ]\n");
    netlist.push_str("tran 1us 5ms 0 1us\n");
    netlist.push_str(&format!("wrdata tran_large.txt v({}) v({})\n", NODE_OUT, NODE_IN));
    netlist.push_str("alter @v_in[pulse] = [ 0.2 0 0 100ns 100ns 10us 1 ]\n");
    netlist.push_str("tran 10us 10ms 0 10us\n");
    netlist.push_str(&format!("wrdata tran_zero.txt v({})\n", NODE_OUT));
    netlist.push_str("quit\n");
    netlist.push_str(".endc\n");
    netlist.push_str(".end\n");

    netlist
}

/// Re-export of the crate-wide canonical SPICE preamble.
///
/// Thin wrapper so existing call sites in this module keep working; the model
/// definitions live in exactly one place: `circuit::standard_spice_headers`.
pub fn standard_spice_headers() -> &'static str {
    crate::circuit::standard_spice_headers()
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

    let mut ac_probes = Vec::new();
    for (idx, p) in preset.probes.iter().enumerate() {
        if matches!(p.probe_type, ProbeType::Gain | ProbeType::Zin | ProbeType::Zout) {
            let vin = if p.condition.vin > 0.0 { p.condition.vin } else { 0.1 };
            let r_load = if p.condition.r_load > 0.0 { p.condition.r_load } else { 10000.0 };
            ac_probes.push((idx, vin, r_load, p.probe_type));
        }
    }

    let default_vin = ac_probes.first().map(|p| p.1).unwrap_or(0.1);
    let default_r_load = ac_probes.first().map(|p| p.2).unwrap_or(10000.0);

    netlist.push_str(&format!(
        "V_in {} {} dc 0 ac {:.4}\n",
        NODE_IN, NODE_GND, default_vin
    ));
    // Test current source for Zout measurement (normally 0 AC, altered to 1 AC during Zout sweep)
    netlist.push_str(&format!(
        "I_test_probe {} {} dc 0 ac 0\n",
        NODE_GND, NODE_OUT
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

    netlist.push_str(&format!("R_probe_load {} {} {:.2}\n", NODE_OUT, NODE_GND, default_r_load));

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

    // Compute AC transfer function, input impedance, and output impedance in one process
    netlist.push_str(".control\n");
    netlist.push_str("op\n");
    netlist.push_str("print allv\n");

    let all_same = ac_probes.windows(2).all(|w| {
        (w[0].1 - w[1].1).abs() < 1e-9 && (w[0].2 - w[1].2).abs() < 1e-9
    });

    if all_same || ac_probes.is_empty() {
        netlist.push_str("ac dec 50 10 100k\n");
        netlist.push_str(&format!("let gain = mag(v({})/v({}))\n", NODE_OUT, NODE_IN));
        netlist.push_str(&format!("let zin = mag(v({})/i(v_in))\n", NODE_IN));
        netlist.push_str("wrdata probe_ac.txt gain zin\n");
        netlist.push_str("alter @v_in[ac] = 0\n");
        netlist.push_str("alter @i_test_probe[ac] = 1\n");
        netlist.push_str("ac dec 50 10 100k\n");
        netlist.push_str(&format!("let zout = mag(v({}))\n", NODE_OUT));
        netlist.push_str("wrdata probe_zout.txt zout\n");
    } else {
        // Also write baseline probe_ac.txt for index 0 or fallback
        netlist.push_str("ac dec 50 10 100k\n");
        netlist.push_str(&format!("let gain = mag(v({})/v({}))\n", NODE_OUT, NODE_IN));
        netlist.push_str(&format!("let zin = mag(v({})/i(v_in))\n", NODE_IN));
        netlist.push_str("wrdata probe_ac.txt gain zin\n");
        netlist.push_str("alter @v_in[ac] = 0\n");
        netlist.push_str("alter @i_test_probe[ac] = 1\n");
        netlist.push_str("ac dec 50 10 100k\n");
        netlist.push_str(&format!("let zout = mag(v({}))\n", NODE_OUT));
        netlist.push_str("wrdata probe_zout.txt zout\n");

        for &(idx, vin, r_load, ptype) in &ac_probes {
            if ptype == ProbeType::Zout {
                netlist.push_str("alter @v_in[ac] = 0\n");
                netlist.push_str("alter @i_test_probe[ac] = 1\n");
                netlist.push_str(&format!("alter r_probe_load = {:.2}\n", r_load));
                netlist.push_str("ac dec 50 10 100k\n");
                netlist.push_str(&format!("let zout_{} = mag(v({}))\n", idx, NODE_OUT));
                netlist.push_str(&format!("wrdata probe_zout_{}.txt zout_{}\n", idx, idx));
            } else {
                netlist.push_str(&format!("alter @v_in[ac] = {:.4}\n", vin));
                netlist.push_str(&format!("alter r_probe_load = {:.2}\n", r_load));
                netlist.push_str("alter @i_test_probe[ac] = 0\n");
                netlist.push_str("ac dec 50 10 100k\n");
                netlist.push_str(&format!("let gain_{} = mag(v({})/v({}))\n", idx, NODE_OUT, NODE_IN));
                netlist.push_str(&format!("let zin_{} = mag(v({})/i(v_in))\n", idx, NODE_IN));
                netlist.push_str(&format!("wrdata probe_ac_{}.txt gain_{} zin_{}\n", idx, idx, idx));
            }
        }
    }

    netlist.push_str("quit\n");
    netlist.push_str(".endc\n");
    netlist.push_str(".end\n");

    netlist
}

/// Generate SPICE netlist for noise spectral density & integrated noise analysis
pub fn to_noise_netlist(
    circuit: &Circuit,
    title: &str,
    r_source: f64,
    r_load: f64,
    f_start: f64,
    f_stop: f64,
    stray_cap_pf: f64,
) -> String {
    let mut netlist = String::new();

    netlist.push_str(&format!("* Noise Analysis: {}\n", title));
    netlist.push_str(standard_spice_headers());

    // Power rails
    netlist.push_str(&format!("V_cc {} {} dc {:.2}\n", NODE_VCC, NODE_GND, circuit.vcc));
    netlist.push_str(&format!("V_ee {} {} dc {:.2}\n", NODE_VEE, NODE_GND, circuit.vee));

    // Input signal source with configurable source impedance
    if r_source > 0.001 {
        // Node 100 is internal ideal AC source node, connected to input node 1 via R_source
        netlist.push_str("V_in 100 0 dc 0 ac 1\n");
        netlist.push_str(&format!("R_source 100 {} {:.4}\n", NODE_IN, r_source));
    } else {
        netlist.push_str(&format!("V_in {} {} dc 0 ac 1\n", NODE_IN, NODE_GND));
    }

    // Circuit components
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

    // Load resistance on output node
    if r_load > 0.001 {
        netlist.push_str(&format!("R_noise_load {} {} {:.2}\n", NODE_OUT, NODE_GND, r_load));
    }

    // Stray capacitances if specified
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

    // Noise analysis control block
    netlist.push_str(".control\n");
    netlist.push_str("op\n");
    netlist.push_str("print allv\n");
    netlist.push_str(&format!("noise v({}) v_in dec 50 {:.1} {:.1}\n", NODE_OUT, f_start, f_stop));
    netlist.push_str("print inoise_total onoise_total\n");
    netlist.push_str("setplot noise1\n");
    netlist.push_str("wrdata noise_out.txt onoise_spectrum inoise_spectrum\n");
    netlist.push_str("quit\n");
    netlist.push_str(".endc\n");
    netlist.push_str(".end\n");

    netlist
}

/// Run stand-alone noise analysis on a circuit with given source and load impedances
pub fn evaluate_noise(
    circuit: &Circuit,
    r_source: f64,
    r_load: f64,
    stray_cap_pf: f64,
    timeout: Duration,
) -> Result<crate::spice::NoiseSummary, FitnessError> {
    let netlist = to_noise_netlist(
        circuit,
        "Circuit Noise Evaluation",
        r_source,
        r_load,
        10.0,
        100_000.0,
        stray_cap_pf,
    );

    let sim_res = run_simulation(&netlist, timeout)?;
    if let Some(mut sum) = sim_res.noise_summary {
        // Recalculate with exact r_source if needed
        if let Some(ref pts) = sim_res.noise_response {
            sum = crate::spice::parse_noise_summary(pts, "", Some(r_source));
        }
        Ok(sum)
    } else if let Some(ref pts) = sim_res.noise_response {
        Ok(crate::spice::parse_noise_summary(pts, "", Some(r_source)))
    } else {
        Err(FitnessError::MissingAcData)
    }
}

/// Helper function to perform log-frequency linear interpolation across probe AC sweeps
fn interpolate_probe<F>(probe_ac: &[crate::spice::ProbeAcPoint], target_freq: f64, extractor: F) -> f64
where
    F: Fn(&crate::spice::ProbeAcPoint) -> f64,
{
    if probe_ac.is_empty() {
        return 0.0;
    }
    if target_freq <= probe_ac[0].freq {
        return extractor(&probe_ac[0]);
    }
    if target_freq >= probe_ac.last().unwrap().freq {
        return extractor(probe_ac.last().unwrap());
    }

    for i in 1..probe_ac.len() {
        if probe_ac[i].freq >= target_freq {
            let p0 = &probe_ac[i - 1];
            let p1 = &probe_ac[i];
            let log_f0 = p0.freq.max(1e-6).log10();
            let log_f1 = p1.freq.max(1e-6).log10();
            let log_target = target_freq.max(1e-6).log10();
            let span = log_f1 - log_f0;
            let t = if span > 1e-9 {
                (log_target - log_f0) / span
            } else {
                0.0
            };
            let v0 = extractor(p0);
            let v1 = extractor(p1);
            return v0 + t * (v1 - v0);
        }
    }
    extractor(probe_ac.last().unwrap())
}

/// Evaluate circuit against preset targets:
/// 1. Feasibility Gate (.op Fast-Drop):
///    - Output rail saturation check
///    - DC offset check
///    - BJT Active Region check (Vbe >= 0.45V, Vce >= 0.15V)
/// 2. Pluggable AC Probe Simulation with log-frequency interpolation
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

    let v_out = dc_node_voltage(&dc_nodes, NODE_OUT)?;

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
            let vc = dc_node_voltage(&dc_nodes, comp.nodes[0])?;
            let vb = dc_node_voltage(&dc_nodes, comp.nodes[1])?;
            let ve = dc_node_voltage(&dc_nodes, comp.nodes[2])?;

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

    let has_ac_probes = preset.probes.iter().any(|p| matches!(p.probe_type, ProbeType::Gain | ProbeType::Zin | ProbeType::Zout));
    let default_probe_ac = sim_res.probe_ac.as_deref().unwrap_or(&[]);
    if has_ac_probes
        && default_probe_ac.is_empty()
        && sim_res
            .probe_ac_map
            .as_ref()
            .map(|m| m.is_empty())
            .unwrap_or(true)
    {
        return Err(FitnessError::MissingProbeData);
    }

    let mut obj_vec = ObjectiveVector::new();
    let mut noise_cache: HashMap<(u64, u64), crate::spice::NoiseSummary> = HashMap::new();

    for (target_idx, target) in preset.probes.iter().enumerate() {
        let target_probe_ac: &[crate::spice::ProbeAcPoint] = sim_res
            .probe_ac_map
            .as_ref()
            .and_then(|m| m.get(&target_idx))
            .map(|v| v.as_slice())
            .unwrap_or(default_probe_ac);

        let measured_val = match target.probe_type {
            ProbeType::Gain => interpolate_probe(target_probe_ac, target.condition.freq, |p| p.gain),
            ProbeType::Zin => interpolate_probe(target_probe_ac, target.condition.freq, |p| p.zin),
            ProbeType::Zout => interpolate_probe(target_probe_ac, target.condition.freq, |p| p.zout),
            ProbeType::DcOffset => v_out.abs(),
            ProbeType::Bom => circuit.components.len() as f64,
            ProbeType::Noise => {
                let r_src = if target.condition.r_source > 0.0 { target.condition.r_source } else { 600.0 };
                let r_ld = if target.condition.r_load > 0.0 { target.condition.r_load } else { 10000.0 };
                let key = (r_src.to_bits(), r_ld.to_bits());
                let noise_sum = if let Some(n) = noise_cache.get(&key) {
                    n.clone()
                } else {
                    let n = evaluate_noise(circuit, r_src, r_ld, stray_cap_pf, timeout)?;
                    noise_cache.insert(key, n.clone());
                    n
                };
                if (target.condition.freq - 100.0).abs() < 10.0 {
                    noise_sum.inoise_spot_100
                } else if (target.condition.freq - 10000.0).abs() < 100.0 {
                    noise_sum.inoise_spot_10k
                } else {
                    noise_sum.inoise_spot_1k
                }
            }
            ProbeType::NoiseFig => {
                let r_src = if target.condition.r_source > 0.0 { target.condition.r_source } else { 600.0 };
                let r_ld = if target.condition.r_load > 0.0 { target.condition.r_load } else { 10000.0 };
                let key = (r_src.to_bits(), r_ld.to_bits());
                let noise_sum = if let Some(n) = noise_cache.get(&key) {
                    n.clone()
                } else {
                    let n = evaluate_noise(circuit, r_src, r_ld, stray_cap_pf, timeout)?;
                    noise_cache.insert(key, n.clone());
                    n
                };
                noise_sum.noise_figure_db.unwrap_or(50.0)
            }
            ProbeType::NoiseTotal => {
                let r_src = if target.condition.r_source > 0.0 { target.condition.r_source } else { 600.0 };
                let r_ld = if target.condition.r_load > 0.0 { target.condition.r_load } else { 10000.0 };
                let key = (r_src.to_bits(), r_ld.to_bits());
                let noise_sum = if let Some(n) = noise_cache.get(&key) {
                    n.clone()
                } else {
                    let n = evaluate_noise(circuit, r_src, r_ld, stray_cap_pf, timeout)?;
                    noise_cache.insert(key, n.clone());
                    n
                };
                noise_sum.inoise_total_rms
            }
        };

        let score = target.score(measured_val);
        obj_vec.add(&target.name, measured_val, score, target.weight, target.is_required);
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
///
/// Ref: Passband maximum gain `max_mag_db` across frequency sweep.
/// - Low-pass / Band-pass: finds upper -3dB cutoff above the passband peak.
/// - High-pass: finds lower -3dB cutoff below the passband peak.
/// - Wideband buffer / flat response: no -3dB drop in 10Hz..100kHz -> has_filter = 0.0.
pub fn extract_ac_features(ac: &[AcPoint]) -> (f64, f64, f64, f64) {
    if ac.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }

    // 1. Find global peak magnitude and its index
    let mut max_idx = 0;
    let mut max_mag_db = f64::NEG_INFINITY;
    for (i, p) in ac.iter().enumerate() {
        if p.mag_db > max_mag_db {
            max_mag_db = p.mag_db;
            max_idx = i;
        }
    }

    let target_db = max_mag_db - 3.01;

    // 2. Search for upper cutoff (above peak: low-pass or band-pass high edge)
    let mut upper_cutoff_freq = None;
    let mut upper_cutoff_idx = None;
    for i in (max_idx + 1)..ac.len() {
        if ac[i].mag_db <= target_db {
            let p_prev = &ac[i - 1];
            let p_curr = &ac[i];
            let span = p_curr.mag_db - p_prev.mag_db;
            let freq = if span.abs() > 1e-6 {
                let frac = (target_db - p_prev.mag_db) / span;
                p_prev.freq + frac * (p_curr.freq - p_prev.freq)
            } else {
                p_curr.freq
            };
            upper_cutoff_freq = Some(freq);
            upper_cutoff_idx = Some(i);
            break;
        }
    }

    // 3. Search for lower cutoff (below peak: high-pass or band-pass low edge)
    let mut lower_cutoff_freq = None;
    let mut _lower_cutoff_idx = None;
    if max_idx > 0 {
        for i in (0..max_idx).rev() {
            if ac[i].mag_db <= target_db {
                let p_prev = &ac[i + 1];
                let p_curr = &ac[i];
                let span = p_curr.mag_db - p_prev.mag_db;
                let freq = if span.abs() > 1e-6 {
                    let frac = (target_db - p_prev.mag_db) / span;
                    p_prev.freq + frac * (p_curr.freq - p_prev.freq)
                } else {
                    p_curr.freq
                };
                lower_cutoff_freq = Some(freq);
                _lower_cutoff_idx = Some(i);
                break;
            }
        }
    }

    // 4. Determine filter cutoff frequency & rolloff
    let (has_filter, cutoff_freq, rolloff_norm) = match (upper_cutoff_freq, lower_cutoff_freq) {
        (Some(f_up), _) => {
            // Low-pass or Band-pass: upper cutoff dominates audio bandwidth
            let idx = upper_cutoff_idx.unwrap();
            let mut rolloff = 0.0;
            if idx < ac.len() - 1 {
                let last_pt = &ac[ac.len() - 1];
                let dec_span = (last_pt.freq / f_up.max(1.0)).log10();
                if dec_span > 0.2 {
                    let db_drop = last_pt.mag_db - target_db;
                    let slope_db_per_dec = db_drop / dec_span;
                    rolloff = (slope_db_per_dec / -40.0).clamp(-0.5, 1.5);
                }
            }
            (1.0, f_up, rolloff)
        }
        (None, Some(f_low)) => {
            // High-pass filter
            let mut rolloff = 0.0;
            let first_pt = &ac[0];
            let dec_span = (f_low.max(1.0) / first_pt.freq.max(1.0)).log10();
            if dec_span > 0.2 {
                let db_drop = first_pt.mag_db - target_db;
                let slope_db_per_dec = db_drop / dec_span;
                rolloff = (slope_db_per_dec / -40.0).clamp(-0.5, 1.5);
            }
            (1.0, f_low, rolloff)
        }
        (None, None) => (0.0, 0.0, 0.0),
    };

    let cutoff_norm = if has_filter > 0.5 {
        (cutoff_freq.clamp(10.0, 100_000.0).log10() / 5.0).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // 5. Peak Q / Resonance boost above passband floor
    // Lowpass: passband floor at low freq (ac[0]). Highpass: passband floor at high freq (ac.last()).
    let passband_floor_db = if upper_cutoff_freq.is_some() {
        ac[0].mag_db
    } else if lower_cutoff_freq.is_some() {
        ac.last().map(|p| p.mag_db).unwrap_or(max_mag_db)
    } else {
        max_mag_db
    };
    let peak_boost = (max_mag_db - passband_floor_db).max(0.0);
    let peak_q_norm = if has_filter > 0.5 {
        (peak_boost / 20.0).clamp(0.0, 1.5)
    } else {
        0.0
    };

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

/// Interpolate input voltage v_in at exact time `t` from sorted TranPoint slice
pub fn interpolate_tran_in(pts: &[TranPoint], t: f64) -> f64 {
    if pts.is_empty() {
        return 0.0;
    }
    let get_v_in = |p: &TranPoint| p.v_in.unwrap_or(0.0);
    if t <= pts[0].time {
        return get_v_in(&pts[0]);
    }
    if t >= pts[pts.len() - 1].time {
        return get_v_in(&pts[pts.len() - 1]);
    }

    match pts.binary_search_by(|p| p.time.partial_cmp(&t).unwrap_or(std::cmp::Ordering::Equal)) {
        Ok(i) => get_v_in(&pts[i]),
        Err(i) => {
            let p0 = &pts[i - 1];
            let p1 = &pts[i];
            let dt = p1.time - p0.time;
            let v0 = get_v_in(p0);
            let v1 = get_v_in(p1);
            if dt > 1e-12 {
                let frac = (t - p0.time) / dt;
                v0 + frac * (v1 - v0)
            } else {
                v0
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

    // Noise floor threshold: calibrated at 0.002 (-54 dB). Residual numerical noise below this is zeroed out.
    let clean_h2 = if h2 > 0.002 { h2.min(1.0) } else { 0.0 };
    let clean_h3 = if h3 > 0.002 { h3.min(1.0) } else { 0.0 };
    let clean_h5 = if h5 > 0.002 { h5.min(1.0) } else { 0.0 };

    (clean_h2, clean_h3, clean_h5)
}

/// Calculate asymmetry index in [-1.0, 1.0] for a waveform after subtracting DC mean
fn calc_asymmetry(pts: &[&TranPoint]) -> f64 {
    if pts.is_empty() {
        return 0.0;
    }
    let n = pts.len() as f64;
    let v_dc: f64 = pts.iter().map(|p| p.v_out).sum::<f64>() / n;

    let mut max_pos: f64 = 0.0;
    let mut max_neg: f64 = 0.0;
    for p in pts {
        let v_ac = p.v_out - v_dc;
        if v_ac > max_pos {
            max_pos = v_ac;
        }
        if -v_ac > max_neg {
            max_neg = -v_ac;
        }
    }

    let span = max_pos + max_neg;
    if span > 0.01 {
        ((max_pos - max_neg) / span).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Extract non-linear features from small-signal and large-signal transient data:
/// 1. Asymmetry delta: |asym(large) - asym(small)|
/// 2. H2 ratio (2nd harmonic / fundamental)
/// 3. H3 ratio (3rd harmonic / fundamental)
/// 4. H5 ratio (5th harmonic / fundamental)
/// 5. Gain compression: 1.0 - (G_large / G_small) using actual measured V_in Vpp
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

    // 2. Resampled Hann-windowed harmonic extraction on large drive (2 cycles: 3ms to 5ms, 512 samples)
    let (h2_ratio, h3_ratio, h5_ratio) =
        extract_harmonics_resampled(large, 1000.0, 0.003, 2.0, 512);

    // 3. Gain compression using actual v_in Vpp
    let v_pos_s = steady_small.iter().map(|p| p.v_out).fold(f64::NEG_INFINITY, f64::max);
    let v_neg_s = steady_small.iter().map(|p| p.v_out).fold(f64::INFINITY, f64::min);
    let vpp_small_out = (v_pos_s - v_neg_s).max(0.0);

    let vin_pos_s = steady_small.iter().filter_map(|p| p.v_in).fold(f64::NEG_INFINITY, f64::max);
    let vin_neg_s = steady_small.iter().filter_map(|p| p.v_in).fold(f64::INFINITY, f64::min);
    let vpp_small_in = if vin_pos_s.is_finite() && vin_neg_s.is_finite() && (vin_pos_s - vin_neg_s) > 1e-4 {
        vin_pos_s - vin_neg_s
    } else {
        0.2 // fallback if v_in is not recorded
    };
    let g_small = vpp_small_out / vpp_small_in.max(1e-6);

    let v_pos_l = steady_large.iter().map(|p| p.v_out).fold(f64::NEG_INFINITY, f64::max);
    let v_neg_l = steady_large.iter().map(|p| p.v_out).fold(f64::INFINITY, f64::min);
    let vpp_large_out = (v_pos_l - v_neg_l).max(0.0);

    let vin_pos_l = steady_large.iter().filter_map(|p| p.v_in).fold(f64::NEG_INFINITY, f64::max);
    let vin_neg_l = steady_large.iter().filter_map(|p| p.v_in).fold(f64::INFINITY, f64::min);
    let vpp_large_in = if vin_pos_l.is_finite() && vin_neg_l.is_finite() && (vin_pos_l - vin_neg_l) > 1e-4 {
        vin_pos_l - vin_neg_l
    } else {
        4.0 // fallback if v_in is not recorded
    };
    let g_large = vpp_large_out / vpp_large_in.max(1e-6);

    let compression = if g_small > 1e-3 {
        (1.0 - (g_large / g_small)).clamp(0.0, 1.0)
    } else {
        0.0
    };

    (asym_delta, h2_ratio, h3_ratio, h5_ratio, compression)
}

/// Extract autonomous dynamics: self-oscillation RMS voltage and frequency
///
/// Uses steady-state window t >= 5ms (after the 10us initial perturbation kick has decayed)
/// and robust autocorrelation / Schmitt-trigger hysteresis to extract fundamental frequency.
pub fn extract_oscillation_features(tran: &[TranPoint]) -> (f64, f64) {
    let steady_pts: Vec<&TranPoint> = tran.iter().filter(|p| p.time >= 0.005).collect();

    if steady_pts.len() < 20 {
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

    // 20 mV threshold for true autonomous oscillation
    if v_rms < 0.02 {
        return (0.0, 0.0);
    }

    let osc_rms_norm = (v_rms / 5.0).clamp(0.0, 1.0);

    // Uniform resampling for autocorrelation (500 points from 5ms to 10ms, dt = 10us)
    let num_samples = 500;
    let t_start = 0.005;
    let t_end = 0.010;
    let dt = (t_end - t_start) / (num_samples as f64);
    let mut v_ac = Vec::with_capacity(num_samples);
    for i in 0..num_samples {
        let t = t_start + (i as f64) * dt;
        v_ac.push(interpolate_tran(tran, t) - v_dc);
    }

    // Autocorrelation to find fundamental period T0
    let min_lag = 1;
    let max_lag = num_samples / 2;
    let mut r0 = 0.0;
    for &v in &v_ac {
        r0 += v * v;
    }

    let mut best_lag = 0;
    let mut best_r = f64::NEG_INFINITY;
    let mut found_trough = false;

    if r0 > 1e-8 {
        for lag in min_lag..max_lag {
            let mut r = 0.0;
            for i in 0..(num_samples - lag) {
                r += v_ac[i] * v_ac[i + lag];
            }
            // First look for autocorrelation to drop below 0.5*r0 (trough)
            if !found_trough {
                if r < 0.5 * r0 {
                    found_trough = true;
                }
            } else if r > best_r {
                best_r = r;
                best_lag = lag;
            }
        }
    }

    let freq = if best_lag > 0 && best_r > 0.3 * r0 {
        1.0 / ((best_lag as f64) * dt)
    } else {
        // Fallback: Schmitt trigger zero crossings with hysteresis = 0.3 * v_rms
        let hyst = 0.3 * v_rms;
        let mut crossings = 0;
        let mut state = steady_pts[0].v_out - v_dc >= 0.0;
        for p in steady_pts.iter().skip(1) {
            let v = p.v_out - v_dc;
            if state && v < -hyst {
                state = false;
                crossings += 1;
            } else if !state && v > hyst {
                state = true;
                crossings += 1;
            }
        }
        let t_span = (steady_pts.last().unwrap().time - steady_pts.first().unwrap().time).max(1e-4);
        (crossings as f64) / (2.0 * t_span)
    };

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
    #[serde(default)]
    pub is_hardware_verified: bool,
    #[serde(default)]
    pub noise_summary: Option<crate::spice::NoiseSummary>,
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
        noise_summary: Option<crate::spice::NoiseSummary>,
    ) -> bool {
        if self.entries.is_empty() {
            self.entries.push(ArchiveEntry {
                circuit,
                descriptor,
                mc_dev_db,
                generation,
                fitness,
                obj_summary,
                is_hardware_verified: false,
                noise_summary,
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
                is_hardware_verified: false,
                noise_summary,
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
        self.maybe_add_with_meta(circuit, descriptor, threshold, k, min_dist, mc_dev_db, 0, None, None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Circuit, Component, NODE_GND, NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE};
    use crate::preset::{ProbeKind, ProbeTarget, TestCondition};

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
        // asym_delta > 0.20
        assert!(
            desc_diode[4] > 0.20,
            "Diode clipper must have large asymmetry delta (> 0.20), got {:.4}",
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
        // Base bias: R1 (VCC to base=10) 75k, R2 (base=10 to VEE) 100k -> Vb ≈ +0.87V
        // Emitter resistor: Re (emitter=2 to VEE) 4.7k -> Ve ≈ +0.19V, Vbe ≈ 0.68V (ACTIVE!)
        // Collector tied to VCC (+9V) -> Vce = 9 - 0.19 = 8.81V (ACTIVE!)
        // Input coupling: Cin (IN=1 to base=10) 1uF
        // Output taken from emitter=2 (NODE_OUT)
        c.add_component(Component::new('Q', 1, vec![NODE_VCC, 10, NODE_OUT], "2N3904").unwrap());
        c.add_component(Component::new('R', 1, vec![NODE_VCC, 10], "75k").unwrap());
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

    #[test]
    fn test_calc_asymmetry_dc_offset_invariance() {
        // Pure symmetrical sine wave with large 2.5V DC offset
        let mut pure_sine_with_dc = Vec::new();
        let dt = 1e-5;
        for i in 0..500 {
            let t = (i as f64) * dt;
            let v_out = 2.5 + 0.5 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin();
            pure_sine_with_dc.push(TranPoint { time: t, v_out, v_in: Some(0.5 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()) });
        }
        let refs: Vec<&TranPoint> = pure_sine_with_dc.iter().collect();
        let asym = calc_asymmetry(&refs);
        println!("Symmetrical sine wave with 2.5V DC offset asymmetry: {:.6}", asym);
        assert!(asym.abs() < 1e-3, "Symmetrical sine wave MUST have asymmetry ~ 0.0 regardless of DC offset, got {:.6}", asym);

        // Asymmetric clipped wave (positive peak clipped at +0.2V relative to DC, negative swings to -1.0V)
        let mut clipped_wave = Vec::new();
        for i in 0..500 {
            let t = (i as f64) * dt;
            let raw_sin = (2.0 * std::f64::consts::PI * 1000.0 * t).sin();
            let v_out = 2.5 + if raw_sin > 0.2 { 0.2 } else { raw_sin };
            clipped_wave.push(TranPoint { time: t, v_out, v_in: Some(raw_sin) });
        }
        let refs_clipped: Vec<&TranPoint> = clipped_wave.iter().collect();
        let asym_clipped = calc_asymmetry(&refs_clipped);
        println!("Asymmetric clipped wave asymmetry: {:.6}", asym_clipped);
        assert!(asym_clipped.abs() > 0.25, "Asymmetric clipped wave must have high asymmetry magnitude (> 0.25), got {:.6}", asym_clipped);
    }

    #[test]
    fn test_ac_features_with_input_coupling_cap() {
        // Build AC response mimicking discrete circuit with 1uF coupling cap:
        // - 10 Hz: -14 dB (attenuated by input cap)
        // - 100 Hz: -1 dB
        // - 1 kHz to 100 kHz: flat 0.0 dB (wideband buffer)
        let mut ac_points = Vec::new();
        let freqs = [10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 5000.0, 10000.0, 50000.0, 100000.0];
        for &f in &freqs {
            // High-pass filter fc = 30 Hz: mag_db = 10 * log10( (f/30)^2 / (1 + (f/30)^2) )
            let ratio: f64 = f / 30.0;
            let mag_sq: f64 = ratio * ratio / (1.0 + ratio * ratio);
            let mag_db: f64 = 10.0 * mag_sq.log10();
            ac_points.push(AcPoint { freq: f, mag_db, phase_deg: 0.0 });
        }

        let (has_filter, cutoff_norm, rolloff_norm, peak_q_norm) = extract_ac_features(&ac_points);
        println!("Coupling cap AC features: has_filter={:.2}, cutoff_norm={:.4}, rolloff={:.4}, peak_q={:.4}",
            has_filter, cutoff_norm, rolloff_norm, peak_q_norm);

        // Max magnitude is ~0 dB at 1kHz..100kHz.
        // Lower cutoff at ~30 Hz is detected properly as high-pass!
        assert_eq!(has_filter, 1.0, "High-pass behavior from coupling capacitor must be detected (has_filter=1.0)");
        let fc = 10f64.powf(cutoff_norm * 5.0);
        println!("Detected high-pass cutoff: {:.1} Hz", fc);
        assert!((fc - 30.0).abs() < 10.0, "Cutoff should be ~30 Hz, got {:.1} Hz", fc);
        assert_eq!(peak_q_norm, 0.0, "No false resonance peak Q should be detected for smooth highpass coupling");
    }

    #[test]
    fn test_probe_ac_log_interpolation() {
        let probe_ac = vec![
            crate::spice::ProbeAcPoint { freq: 100.0, gain: 1.0, zin: 100_000.0, zout: 50.0 },
            crate::spice::ProbeAcPoint { freq: 1000.0, gain: 2.0, zin: 10_000.0, zout: 500.0 },
        ];
        // Test exact log-midpoint: sqrt(100 * 1000) = 316.2277 Hz
        let mid_freq = 316.227766;
        let interp_gain = interpolate_probe(&probe_ac, mid_freq, |p| p.gain);
        let interp_zin = interpolate_probe(&probe_ac, mid_freq, |p| p.zin);
        println!("Interpolated at {:.1} Hz: gain={:.4}, zin={:.1}", mid_freq, interp_gain, interp_zin);

        // Midpoint in log scale (log10(316.2277) = 2.5, midpoint between 2.0 and 3.0) should give exactly t = 0.5
        assert!((interp_gain - 1.5).abs() < 0.01, "Interpolated gain at log midpoint should be 1.5, got {:.4}", interp_gain);
        assert!((interp_zin - 55_000.0).abs() < 100.0, "Interpolated zin at log midpoint should be 55k, got {:.1}", interp_zin);
    }

    #[test]
    fn test_per_probe_conditions_netlist() {
        let seeds = crate::mutate::seed_discrete_population(1);
        let circuit = &seeds[0];

        let mut preset = Preset::buffer_default();
        // Add a second probe with heavy 1k load and higher vin
        preset.probes.push(ProbeTarget {
            name: "GainHeavyLoad".to_string(),
            probe_type: ProbeType::Gain,
            condition: TestCondition {
                freq: 1000.0,
                vin: 0.5,
                r_load: 1000.0,
                r_source: 600.0,
            },
            kind: ProbeKind::Closeness,
            want: 0.9,
            soft: 0.4,
            weight: 2.0,
            is_required: false,
        });

        let netlist = to_probe_netlist(circuit, &preset, 0.0);
        println!("Generated Multi-Condition Probe Netlist:\n{}", netlist);

        // Verify alter commands for custom condition appear in control script
        assert!(netlist.contains("alter @v_in[ac] = 0.5000"), "Netlist must alter vin for probe with vin=0.5");
        assert!(netlist.contains("alter r_probe_load = 1000.00"), "Netlist must alter r_load for probe with r_load=1k");
        assert!(netlist.contains("probe_ac_"), "Netlist must output indexed probe files for varying conditions");

        // Run simulation and evaluate preset
        let timeout = Duration::from_secs(5);
        let obj_vec = evaluate_preset(circuit, &preset, 0.0, timeout).expect("Multi-condition evaluation must succeed");
        assert_eq!(obj_vec.names.len(), preset.probes.len());
        println!("Multi-condition evaluation results: {:?}", obj_vec.summary());
    }

    #[test]
    fn test_pure_johnson_noise_evaluation() {
        use crate::circuit::{Component, ComponentType, NODE_GND, NODE_IN, NODE_OUT};

        // Passive pass-through circuit (a single 100 Ohm resistor between IN and OUT)
        let mut circuit = Circuit::new();
        circuit.add_component(Component {
            id: 1,
            comp_type: ComponentType::R,
            nodes: vec![NODE_IN, NODE_OUT],
            value: "100".to_string(),
        });
        // Pull-down 100k to ground
        circuit.add_component(Component {
            id: 2,
            comp_type: ComponentType::R,
            nodes: vec![NODE_OUT, NODE_GND],
            value: "100k".to_string(),
        });

        let timeout = Duration::from_secs(5);
        let noise_summary = evaluate_noise(&circuit, 600.0, 10000.0, 0.0, timeout)
            .expect("Noise evaluation on passive network must succeed");

        println!("Passive network noise summary: {:?}", noise_summary);
        // Thermal Johnson noise floor of ~700 Ohm equivalent: sqrt(4 * k * T * 700) ≈ 3.4 nV/√Hz
        assert!(
            noise_summary.inoise_spot_1k >= 2.8 && noise_summary.inoise_spot_1k <= 4.0,
            "Input-referred noise density at 1kHz should be ~3.4 nV/√Hz, got {:.3} nV/√Hz",
            noise_summary.inoise_spot_1k
        );

        // For a passive resistor network, noise figure should be close to passive insertion loss (low dB)
        if let Some(nf) = noise_summary.noise_figure_db {
            println!("Measured Noise Figure: {:.2} dB", nf);
            assert!(nf >= 0.0 && nf < 3.0, "Passive network NF should be low, got {:.2} dB", nf);
        }
    }

    #[test]
    fn test_bjt_noise_model_flicker_corner() {
        use crate::circuit::{Component, ComponentType, NODE_GND, NODE_IN, NODE_OUT, NODE_VCC};

        // Simple Common-Emitter BJT Amplifier
        let mut circuit = Circuit::new();
        // Base bias resistor from VCC: 470k
        circuit.add_component(Component {
            id: 1,
            comp_type: ComponentType::R,
            nodes: vec![NODE_VCC, 5],
            value: "470k".to_string(),
        });
        // Base input coupling capacitor: 10uF from IN to Base (node 5)
        circuit.add_component(Component {
            id: 2,
            comp_type: ComponentType::C,
            nodes: vec![NODE_IN, 5],
            value: "10u".to_string(),
        });
        // Collector load resistor: 4.7k from VCC to OUT (node 6)
        circuit.add_component(Component {
            id: 3,
            comp_type: ComponentType::R,
            nodes: vec![NODE_VCC, NODE_OUT],
            value: "4.7k".to_string(),
        });
        // BJT 2N3904 (NPN): Collector=OUT, Base=5, Emitter=GND
        circuit.add_component(Component {
            id: 4,
            comp_type: ComponentType::Q,
            nodes: vec![NODE_OUT, 5, NODE_GND],
            value: "2N3904".to_string(),
        });

        let timeout = Duration::from_secs(5);
        let noise_summary = evaluate_noise(&circuit, 600.0, 10000.0, 0.0, timeout)
            .expect("Noise evaluation on BJT amplifier must succeed");

        println!("BJT CE amplifier noise summary: {:?}", noise_summary);
        println!("  • 100 Hz spot noise: {:.2} nV/√Hz", noise_summary.inoise_spot_100);
        println!("  • 1 kHz spot noise:  {:.2} nV/√Hz", noise_summary.inoise_spot_1k);
        println!("  • 10 kHz spot noise: {:.2} nV/√Hz", noise_summary.inoise_spot_10k);
        println!("  • Total RMS noise:   {:.2} µV RMS", noise_summary.inoise_total_rms);
        if let Some(fc) = noise_summary.corner_freq {
            println!("  • 1/f Corner Freq:   {:.1} Hz", fc);
        }

        // Verify that 1/f flicker noise causes 100Hz noise to be higher than 10kHz thermal floor
        assert!(
            noise_summary.inoise_spot_100 > noise_summary.inoise_spot_10k,
            "100 Hz noise ({:.2} nV) should be greater than 10 kHz noise ({:.2} nV) due to BJT flicker noise (Kf)",
            noise_summary.inoise_spot_100,
            noise_summary.inoise_spot_10k
        );
    }

    #[test]
    fn test_low_noise_preamp_preset_evaluation() {
        let seeds = crate::mutate::seed_discrete_population(1);
        let circuit = &seeds[0];

        let preset = Preset::low_noise_preamp_default();
        let timeout = Duration::from_secs(5);
        let obj_vec = evaluate_preset(circuit, &preset, 0.0, timeout)
            .expect("Low-noise preamp preset evaluation must succeed");

        println!("Low-noise preamp preset evaluation results: {:?}", obj_vec.summary());
        assert_eq!(obj_vec.names.len(), preset.probes.len());
        assert!(obj_vec.names.iter().any(|n| n.contains("Noise")));
    }
}


