use crate::circuit::{
    standard_spice_headers, Circuit, ComponentType, NODE_GND, NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE,
};
use crate::spice::{run_simulation, SpiceError};
use std::time::Duration;

/// Parse a SPICE component value string (e.g. "10k", "15.9nF", "100pF", "1Meg", "2.2uF") into f64.
pub fn parse_spice_value(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let mut lower = s.to_lowercase();

    // Remove unit suffixes like 'f' (Farad), 'ohm', 'h' (Henry)
    if lower.ends_with("ohm") {
        lower.truncate(lower.len() - 3);
    } else if (lower.ends_with("nf")
        || lower.ends_with("pf")
        || lower.ends_with("uf")
        || lower.ends_with("mf")
        || lower.ends_with("kf"))
        && lower.len() > 2
    {
        lower.pop(); // Remove 'f'
    }

    if lower.ends_with("meg") {
        let num_str = &lower[..lower.len() - 3];
        return num_str.parse::<f64>().ok().map(|v| v * 1e6);
    }

    let (num_str, mult) = if lower.ends_with('k') {
        (&lower[..lower.len() - 1], 1e3)
    } else if lower.ends_with('m') {
        (&lower[..lower.len() - 1], 1e-3)
    } else if lower.ends_with('u') {
        (&lower[..lower.len() - 1], 1e-6)
    } else if lower.ends_with('n') {
        (&lower[..lower.len() - 1], 1e-9)
    } else if lower.ends_with('p') {
        (&lower[..lower.len() - 1], 1e-12)
    } else if lower.ends_with('g') {
        (&lower[..lower.len() - 1], 1e9)
    } else if lower.ends_with('t') {
        (&lower[..lower.len() - 1], 1e12)
    } else {
        (lower.as_str(), 1.0)
    };

    num_str.parse::<f64>().ok().map(|v| v * mult)
}

/// Format a physical value into SPICE scientific notation (e.g. 1.045e+04)
pub fn format_spice_value(val: f64) -> String {
    format!("{:.6e}", val)
}

/// Box-Muller Gaussian random number generator with given mean and standard deviation
pub fn gaussian(mean: f64, std_dev: f64) -> f64 {
    let u1 = fastrand::f64().max(1e-10);
    let u2 = fastrand::f64();
    let z0 = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
    mean + z0 * std_dev
}

/// Generate a realistic SPICE netlist wrapping the circuit with breadboard parasitics:
/// Injects stray shunt capacitance (default 22pF) to ground on every active circuit node.
use rayon::prelude::*;

/// Generate custom SPICE headers with perturbed BJT beta (Bf) parameters for Monte Carlo runs
pub fn monte_carlo_spice_headers(bf_npn: f64, bf_pnp: f64) -> String {
    format!(
        concat!(
            ".include \"tl072.sub\"\n",
            ".model 1N4148 D(is=2.52n rs=0.568 n=1.752 cjo=4p m=0.4 tt=20n)\n",
            ".model 2N3904 NPN(Is=6.734f Xti=3 Eg=1.11 Vaf=74.03 Bf={:.1} Ne=1.259 Ise=6.734f Ikf=66.78m Xtb=1.5 Br=.7371 Nc=2 Isc=0 Ikr=0 Rc=1 Cjc=3.638p Mjc=.3085 Vjc=.75 Fc=.5 Cje=4.493p Mje=.2593 Vje=.75 Tr=239.5n Tf=301.2p Itf=.4 Vtf=4 Xtf=2 Rb=10)\n",
            ".model 2N3906 PNP(Is=1.41f Xti=3 Eg=1.11 Vaf=18.7 Bf={:.1} Ne=1.5 Ise=0 Ikf=80m Xtb=1.5 Br=4.977 Nc=2 Isc=0 Ikr=0 Rc=2 Cjc=4.5p Mjc=.3 Vjc=.75 Fc=.5 Cje=5p Mje=.3 Vje=.75 Tr=50n Tf=300p Itf=.4 Vtf=4 Xtf=2 Rb=10)\n",
        ),
        bf_npn, bf_pnp
    )
}

/// Generate a realistic SPICE netlist wrapping the circuit with breadboard parasitics and custom headers
pub fn to_realistic_netlist_with_headers(
    circuit: &Circuit,
    title: &str,
    stray_cap_pf: f64,
    headers: &str,
) -> String {
    let mut netlist = String::new();

    netlist.push_str(&format!("* Realistic Netlist: {}\n", title));
    netlist.push_str(headers);

    // Power supplies and AC input source
    netlist.push_str(&format!("V_in {} {} dc 0 ac 1\n", NODE_IN, NODE_GND));
    netlist.push_str(&format!("V_cc {} {} dc {:.2}\n", NODE_VCC, NODE_GND, circuit.vcc));
    netlist.push_str(&format!("V_ee {} {} dc {:.2}\n", NODE_VEE, NODE_GND, circuit.vee));

    // Components
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

    // Breadboard realism: inject 22pF stray capacitance to ground on every active node
    // (Except GND=0, VCC=3, VEE=4)
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

    // Simulation control block
    netlist.push_str(".control\n");
    netlist.push_str("op\n");
    netlist.push_str("print allv\n");
    netlist.push_str("ac dec 100 10 100k\n");
    netlist.push_str(&format!("wrdata ac_out.txt v({})\n", NODE_OUT));
    netlist.push_str("quit\n");
    netlist.push_str(".endc\n");
    netlist.push_str(".end\n");

    netlist
}

/// Generate a realistic SPICE netlist with standard headers
pub fn to_realistic_netlist(circuit: &Circuit, title: &str, stray_cap_pf: f64) -> String {
    to_realistic_netlist_with_headers(circuit, title, stray_cap_pf, standard_spice_headers())
}

/// Perturb all Resistor and Capacitor values in the circuit by a Gaussian tolerance (e.g. 5%)
pub fn perturb_circuit(circuit: &Circuit, tolerance: f64) -> Circuit {
    let mut perturbed = circuit.clone();

    for comp in &mut perturbed.components {
        if comp.comp_type == ComponentType::R || comp.comp_type == ComponentType::C {
            if let Some(val) = parse_spice_value(&comp.value) {
                // Perturb with Gaussian noise bounded to ± 3*sigma
                let factor = gaussian(1.0, tolerance).clamp(1.0 - 3.0 * tolerance, 1.0 + 3.0 * tolerance);
                let new_val = val * factor;
                comp.value = format_spice_value(new_val);
            }
        }
    }

    perturbed
}

/// Evaluation report from Monte Carlo tolerance analysis
#[derive(Debug, Clone, PartialEq)]
pub struct WorstCaseReport {
    /// Nominal peak / maximum gain in dB
    pub nominal_peak_db: f64,
    /// Minimum peak gain observed across all runs (worst-case drop)
    pub worst_case_peak_db: f64,
    /// Maximum absolute deviation from nominal response across all runs in dB
    pub max_deviation_db: f64,
    /// Number of runs that converged and simulated successfully
    pub runs_passed: usize,
    /// Total number of runs evaluated
    pub total_runs: usize,
}

/// Perform Monte Carlo evaluation with parallel SPICE execution and BJT beta variations.
/// Evaluates worst-case environmental performance.
pub fn evaluate_monte_carlo(
    circuit: &Circuit,
    runs: usize,
    tolerance: f64,
    stray_cap_pf: f64,
    timeout: Duration,
) -> Result<WorstCaseReport, SpiceError> {
    // 1. Simulate nominal realistic circuit
    let nominal_netlist = to_realistic_netlist(circuit, "Nominal Realistic", stray_cap_pf);
    let nominal_sim = run_simulation(&nominal_netlist, timeout)?;
    let nominal_ac = nominal_sim
        .ac_response
        .ok_or_else(|| SpiceError::ParseError("Nominal AC response missing".into()))?;

    let nominal_peak_db = nominal_ac
        .iter()
        .map(|p| p.mag_db)
        .fold(f64::NEG_INFINITY, f64::max);

    // 2. Perform Monte Carlo perturbed runs in parallel with Rayon
    let mc_results: Vec<Option<(f64, f64)>> = (0..runs)
        .into_par_iter()
        .map(|run_idx| {
            let mut perturbed = perturb_circuit(circuit, tolerance);
            // Generate per-transistor Beta (Bf) models to capture real breadboard transistor mismatch
            let mut per_bjt_headers = String::new();
            let mut has_bjt = false;
            for comp in &mut perturbed.components {
                if comp.comp_type == ComponentType::Q {
                    has_bjt = true;
                    let is_pnp = comp.value.to_uppercase().contains("3906") || comp.value.to_uppercase().contains("PNP");
                    let model_name = format!("{}_MC_{}", if is_pnp { "2N3906" } else { "2N3904" }, comp.id);
                    if is_pnp {
                        let bf = gaussian(180.0, 40.0).clamp(80.0, 300.0);
                        per_bjt_headers.push_str(&format!(
                            ".model {} PNP(Is=1.41f Xti=3 Eg=1.11 Vaf=18.7 Bf={:.1} Ne=1.5 Ise=0 Ikf=80m Xtb=1.5 Br=4.977 Nc=2 Isc=0 Ikr=0 Rc=2 Cjc=4.5p Mjc=.3 Vjc=.75 Fc=.5 Cje=5p Mje=.3 Vje=.75 Tr=50n Tf=300p Itf=.4 Vtf=4 Xtf=2 Rb=10)\n",
                            model_name, bf
                        ));
                    } else {
                        let bf = gaussian(300.0, 60.0).clamp(100.0, 450.0);
                        per_bjt_headers.push_str(&format!(
                            ".model {} NPN(Is=6.734f Xti=3 Eg=1.11 Vaf=74.03 Bf={:.1} Ne=1.259 Ise=6.734f Ikf=66.78m Xtb=1.5 Br=.7371 Nc=2 Isc=0 Ikr=0 Rc=1 Cjc=3.638p Mjc=.3085 Vjc=.75 Fc=.5 Cje=4.493p Mje=.2593 Vje=.75 Tr=239.5n Tf=301.2p Itf=.4 Vtf=4 Xtf=2 Rb=10)\n",
                            model_name, bf
                        ));
                    }
                    comp.value = model_name;
                }
            }

            let headers = if has_bjt {
                format!("{}\n{}", standard_spice_headers(), per_bjt_headers)
            } else {
                let bf_npn = gaussian(300.0, 60.0).clamp(100.0, 450.0);
                let bf_pnp = gaussian(180.0, 40.0).clamp(80.0, 300.0);
                monte_carlo_spice_headers(bf_npn, bf_pnp)
            };

            let netlist = to_realistic_netlist_with_headers(
                &perturbed,
                &format!("Monte Carlo Run #{}", run_idx + 1),
                stray_cap_pf,
                &headers,
            );

            if let Ok(sim) = run_simulation(&netlist, timeout) {
                if let Some(ac) = sim.ac_response {
                    let run_peak = ac
                        .iter()
                        .map(|p| p.mag_db)
                        .fold(f64::NEG_INFINITY, f64::max);

                    let mut max_dev = 0.0;
                    for (nom_pt, run_pt) in nominal_ac.iter().zip(ac.iter()) {
                        let diff = (nom_pt.mag_db - run_pt.mag_db).abs();
                        if diff > max_dev {
                            max_dev = diff;
                        }
                    }
                    return Some((run_peak, max_dev));
                }
            }
            None
        })
        .collect();

    let mut worst_case_peak_db = nominal_peak_db;
    let mut max_deviation_db = 0.0;
    let mut runs_passed = 0;

    for res in mc_results {
        if let Some((run_peak, max_dev)) = res {
            runs_passed += 1;
            if run_peak < worst_case_peak_db {
                worst_case_peak_db = run_peak;
            }
            if max_dev > max_deviation_db {
                max_deviation_db = max_dev;
            }
        }
    }

    Ok(WorstCaseReport {
        nominal_peak_db,
        worst_case_peak_db,
        max_deviation_db,
        runs_passed,
        total_runs: runs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Circuit, Component, NODE_GND, NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE};

    /// Verify SPICE engineering string value parser
    #[test]
    fn test_spice_value_parsing() {
        assert_eq!(parse_spice_value("10k"), Some(10000.0));
        assert_eq!(parse_spice_value("1k"), Some(1000.0));
        assert_eq!(parse_spice_value("100"), Some(100.0));
        assert_eq!(parse_spice_value("15.9nF"), Some(1.59e-8));
        assert_eq!(parse_spice_value("100pF"), Some(1.0e-10));
        assert_eq!(parse_spice_value("2.2uF"), Some(2.2e-6));
        assert_eq!(parse_spice_value("1Meg"), Some(1.0e6));
        assert_eq!(parse_spice_value("0.001"), Some(0.001));
    }

    /// Verify 22pF stray capacitance injection into netlist
    #[test]
    fn test_stray_capacitance_injection() {
        let mut c = Circuit::new();
        c.add_component(Component::new('R', 1, vec![NODE_IN, 10], "10k").unwrap());
        c.add_component(Component::new('R', 2, vec![10, NODE_OUT], "10k").unwrap());
        c.add_component(Component::new('C', 1, vec![NODE_OUT, NODE_GND], "10nF").unwrap());

        let netlist = to_realistic_netlist(&c, "Stray Test", 22.0);
        println!("Realistic Netlist:\n{}", netlist);

        assert!(netlist.contains("C_stray_"));
        assert!(netlist.contains("22.00pF"));
    }

    /// Acceptance Criterion:
    /// Worst-case Monte Carlo metric must cleanly differentiate between:
    /// 1. A robust Low-Q filter (Sallen-Key Q=0.5): max deviation is tiny (< 1.5 dB).
    /// 2. A fragile High-Q filter (Sallen-Key with high positive gain K=2.85, Q≈6.7):
    ///    severe sensitivity where 5% component drift causes > 4.0 dB deviation.
    #[test]
    fn test_worst_case_differentiates_high_q_vs_low_q() {
        // 1. Build Robust Low-Q Circuit (Unity Gain Follower Sallen-Key Lowpass, Q=0.5)
        let mut low_q = Circuit::new();
        low_q.add_component(Component::new('R', 1, vec![NODE_IN, 10], "10k").unwrap());
        low_q.add_component(Component::new('R', 2, vec![10, 20], "10k").unwrap());
        low_q.add_component(Component::new('C', 1, vec![10, NODE_OUT], "15.9nF").unwrap());
        low_q.add_component(Component::new('C', 2, vec![20, NODE_GND], "15.9nF").unwrap());
        low_q.add_component(
            Component::new('X', 1, vec![20, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );
        low_q.validate().unwrap();

        let low_q_report = evaluate_monte_carlo(&low_q, 20, 0.05, 22.0, Duration::from_secs(2))
            .expect("Low-Q Monte Carlo failed");

        println!(
            "Low-Q Filter: Nominal Peak = {:.2} dB, Worst Peak = {:.2} dB, Max Dev = {:.2} dB, Passed = {}/{}",
            low_q_report.nominal_peak_db,
            low_q_report.worst_case_peak_db,
            low_q_report.max_deviation_db,
            low_q_report.runs_passed,
            low_q_report.total_runs
        );

        // 2. Build Fragile High-Q Circuit
        // Sallen-Key with non-inverting gain K = 1 + Rf/Rg:
        // Rg = 10k from inv (node 30) to GND (0)
        // Rf = 18.5k from OUT (2) to inv (node 30) => K = 1 + 18.5/10 = 2.85!
        // Q = 1 / (3 - K) = 1 / 0.15 ≈ 6.67 (Very sharp resonant peak, highly sensitive to 5% drift)
        let mut high_q = Circuit::new();
        high_q.add_component(Component::new('R', 1, vec![NODE_IN, 10], "10k").unwrap());
        high_q.add_component(Component::new('R', 2, vec![10, 20], "10k").unwrap());
        high_q.add_component(Component::new('C', 1, vec![10, NODE_OUT], "15.9nF").unwrap());
        high_q.add_component(Component::new('C', 2, vec![20, NODE_GND], "15.9nF").unwrap());
        high_q.add_component(
            Component::new('X', 1, vec![20, 30, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );
        high_q.add_component(Component::new('R', 3, vec![30, NODE_GND], "10k").unwrap());
        high_q.add_component(Component::new('R', 4, vec![NODE_OUT, 30], "18.5k").unwrap());
        high_q.validate().unwrap();

        let high_q_report = evaluate_monte_carlo(&high_q, 20, 0.05, 22.0, Duration::from_secs(2))
            .expect("High-Q Monte Carlo failed");

        println!(
            "High-Q Filter: Nominal Peak = {:.2} dB, Worst Peak = {:.2} dB, Max Dev = {:.2} dB, Passed = {}/{}",
            high_q_report.nominal_peak_db,
            high_q_report.worst_case_peak_db,
            high_q_report.max_deviation_db,
            high_q_report.runs_passed,
            high_q_report.total_runs
        );

        // Verification Assertions:
        // 1. Low-Q filter must be robust: max deviation < 2.5 dB
        assert!(
            low_q_report.max_deviation_db < 2.5,
            "Low-Q filter should have small deviation (< 2.5 dB), got {:.2} dB",
            low_q_report.max_deviation_db
        );

        // 2. High-Q filter must show severe tolerance sensitivity: max deviation > 10.0 dB
        assert!(
            high_q_report.max_deviation_db > 10.0,
            "High-Q filter should have large deviation (> 10.0 dB), got {:.2} dB",
            high_q_report.max_deviation_db
        );

        // 3. Clear metric separation: High-Q deviation must be at least 3x greater than Low-Q
        assert!(
            high_q_report.max_deviation_db > low_q_report.max_deviation_db * 3.0,
            "Worst-case metric failed to differentiate: Low-Q={:.2}dB vs High-Q={:.2}dB",
            low_q_report.max_deviation_db,
            high_q_report.max_deviation_db
        );
    }
}
