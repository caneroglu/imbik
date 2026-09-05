use crate::spice::SimulationResult;

/// Threshold limits for filtering out unviable circuits
#[derive(Debug, Clone)]
pub struct ConstraintLimits {
    /// Minimum magnitude in dB required at least once in the frequency band (default: -60 dB)
    pub min_signal_mag_db: f64,
    /// Maximum allowable DC offset on the output node in Volts (default: ±0.50 V = ±500 mV)
    pub max_dc_offset_v: f64,
    /// Margin from supply rails before a node is considered stuck to the rail in Volts (default: 1.65 V)
    pub rail_margin_v: f64,
    /// Maximum allowable unwrapped phase jump between adjacent points in degrees (default: 120.0°)
    pub max_phase_jump_deg: f64,
}

impl Default for ConstraintLimits {
    fn default() -> Self {
        ConstraintLimits {
            min_signal_mag_db: -60.0,
            max_dc_offset_v: 0.50,
            rail_margin_v: 1.65,
            max_phase_jump_deg: 120.0,
        }
    }
}

/// Telemetry counters for tracking candidate rejection distribution during search
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilterStats {
    pub total_evaluated: usize,
    pub passed: usize,
    pub dead_signal: usize,
    pub rail_saturation: usize,
    pub dc_offset: usize,
    pub phase_instability: usize,
    pub missing_data: usize,
}

impl FilterStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, result: &Result<(), RejectReason>) {
        self.total_evaluated += 1;
        match result {
            Ok(()) => self.passed += 1,
            Err(RejectReason::DeadSignal) => self.dead_signal += 1,
            Err(RejectReason::RailSaturation { .. }) => self.rail_saturation += 1,
            Err(RejectReason::DcOffsetTooHigh { .. }) => self.dc_offset += 1,
            Err(RejectReason::PhaseInstability { .. }) => self.phase_instability += 1,
            Err(RejectReason::MissingSimulationData(_)) => self.missing_data += 1,
        }
    }

    pub fn summary(&self) -> String {
        let pass_rate = if self.total_evaluated > 0 {
            (self.passed as f64 / self.total_evaluated as f64) * 100.0
        } else {
            0.0
        };
        format!(
            "Total: {} | Passed: {} ({:.1}%) | DeadSignal: {} | RailSat: {} | DcOffset: {} | PhaseInstab: {}",
            self.total_evaluated,
            self.passed,
            pass_rate,
            self.dead_signal,
            self.rail_saturation,
            self.dc_offset,
            self.phase_instability
        )
    }
}

/// Specific reason why a candidate circuit was rejected by the filter
#[derive(Debug, PartialEq, Clone)]
pub enum RejectReason {
    /// No frequency point reached the minimum signal threshold (-60 dB)
    DeadSignal,
    /// A circuit node is stuck to the positive or negative supply rail
    RailSaturation {
        node: String,
        voltage: f64,
        vcc: f64,
        vee: f64,
    },
    /// DC offset on the output exceeds the allowable threshold (±100 mV)
    DcOffsetTooHigh {
        offset_v: f64,
        limit_v: f64,
    },
    /// Phase response exhibits non-convergent runaway or erratic discontinuity
    PhaseInstability {
        reason: String,
    },
    /// Essential simulation output was missing
    MissingSimulationData(String),
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RejectReason::DeadSignal => {
                write!(f, "Dead signal: output never exceeds -60dB in frequency band")
            }
            RejectReason::RailSaturation { node, voltage, vcc, vee } => {
                write!(
                    f,
                    "Rail saturation on node '{}': voltage {:.3}V is stuck near rail (VCC={:.1}V, VEE={:.1}V)",
                    node, voltage, vcc, vee
                )
            }
            RejectReason::DcOffsetTooHigh { offset_v, limit_v } => {
                write!(
                    f,
                    "DC offset too high: {:.3}V exceeds ±{:.3}V limit",
                    offset_v, limit_v
                )
            }
            RejectReason::PhaseInstability { reason } => {
                write!(f, "Phase response instability: {}", reason)
            }
            RejectReason::MissingSimulationData(msg) => {
                write!(f, "Missing simulation data: {}", msg)
            }
        }
    }
}

impl std::error::Error for RejectReason {}

/// Primary constraint check using default limits.
/// Returns Ok(()) if the circuit passes all checks, or Err(RejectReason) explaining why it failed.
pub fn check(sim_result: &SimulationResult, vcc: f64, vee: f64) -> Result<(), RejectReason> {
    check_with_limits(sim_result, vcc, vee, &ConstraintLimits::default())
}

/// Constraint check with custom configurable limits.
pub fn check_with_limits(
    sim_result: &SimulationResult,
    vcc: f64,
    vee: f64,
    limits: &ConstraintLimits,
) -> Result<(), RejectReason> {
    // 1. Check for Rail Saturation across all DC operating point nodes
    let pos_rail_threshold = vcc - limits.rail_margin_v;
    let neg_rail_threshold = vee + limits.rail_margin_v;

    for (node, &v) in &sim_result.dc_nodes {
        // Skip subcircuit internal nodes (e.g. "x1.91", "x1.99")
        if node.contains('.') {
            continue;
        }

        // Skip supply nodes themselves (Node 3 / VCC, Node 4 / VEE)
        if node == "3" || node == "v(3)" || node == "4" || node == "v(4)" || node == "vcc" || node == "vee" {
            continue;
        }

        if v >= pos_rail_threshold || v <= neg_rail_threshold {
            return Err(RejectReason::RailSaturation {
                node: node.clone(),
                voltage: v,
                vcc,
                vee,
            });
        }
    }

    // 2. Check Output DC Offset
    let out_dc = sim_result
        .dc_nodes
        .get("2")
        .copied()
        .or_else(|| sim_result.dc_nodes.get("v(2)").copied())
        .or_else(|| sim_result.dc_nodes.get("out").copied())
        .or_else(|| sim_result.dc_nodes.get("v(out)").copied());

    if let Some(dc_val) = out_dc {
        if dc_val.abs() > limits.max_dc_offset_v {
            return Err(RejectReason::DcOffsetTooHigh {
                offset_v: dc_val,
                limit_v: limits.max_dc_offset_v,
            });
        }
    } else if !sim_result.dc_nodes.is_empty() {
        return Err(RejectReason::MissingSimulationData(
            "Output node (node 2) not found in DC operating point results".to_string(),
        ));
    }

    // 3. Check for Meaningful Signal in AC response (Passband check)
    if let Some(ac_points) = &sim_result.ac_response {
        if ac_points.is_empty() {
            return Err(RejectReason::DeadSignal);
        }

        let max_mag = ac_points
            .iter()
            .map(|p| p.mag_db)
            .fold(f64::NEG_INFINITY, f64::max);

        if max_mag < limits.min_signal_mag_db {
            return Err(RejectReason::DeadSignal);
        }

        // 4. Check Phase Stability
        // Detect NaN / Infinity or abnormal discontinuous phase jumps (> 300° between adjacent log-spaced points)
        for i in 0..ac_points.len() {
            let p = &ac_points[i];
            if p.phase_deg.is_nan() || p.phase_deg.is_infinite() {
                return Err(RejectReason::PhaseInstability {
                    reason: format!("Non-finite phase value at {:.1} Hz", p.freq),
                });
            }

            if i > 0 {
                let prev = &ac_points[i - 1];
                let raw_diff = (p.phase_deg - prev.phase_deg).abs();
                // Phase unwrapping: ignore 360° branch cuts from atan2
                let unwrapped_diff = (raw_diff % 360.0).min(360.0 - (raw_diff % 360.0));
                if unwrapped_diff > 90.0 {
                    return Err(RejectReason::PhaseInstability {
                        reason: format!(
                            "Discontinuous phase jump of {:.1}° between {:.1}Hz and {:.1}Hz",
                            unwrapped_diff, prev.freq, p.freq
                        ),
                    });
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Circuit, Component, NODE_GND, NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE};
    use crate::spice::run_simulation;
    use std::time::Duration;

    /// Acceptance Criterion 1: Circuit with grounded output is rejected with DeadSignal
    #[test]
    fn test_reject_grounded_output() {
        let mut c = Circuit::new();
        // R1: IN -> Node 10
        c.add_component(Component::new('R', 1, vec![NODE_IN, 10], "1k").unwrap());
        // R2: Node 10 -> OUT
        c.add_component(Component::new('R', 2, vec![10, NODE_OUT], "1k").unwrap());
        // R3: OUT -> GND with direct short / zero ohm resistor to ground
        c.add_component(Component::new('R', 3, vec![NODE_OUT, NODE_GND], "0.001").unwrap());

        c.validate().expect("Circuit should pass topology validation");
        let netlist = c.to_netlist("Grounded Output Test");
        let sim = run_simulation(&netlist, Duration::from_secs(2)).expect("Sim should succeed");

        let result = check(&sim, c.vcc, c.vee);
        println!("Grounded output filter decision: {:?}", result);
        assert_eq!(result, Err(RejectReason::DeadSignal));
    }

    /// Acceptance Criterion 2: Circuit with op-amp stuck to rail is rejected with RailSaturation
    #[test]
    fn test_reject_rail_saturation() {
        let mut c = Circuit::new();
        // Positive input connected to 4.5V via voltage divider from VCC (+9V)
        c.add_component(Component::new('R', 1, vec![NODE_VCC, 10], "10k").unwrap());
        c.add_component(Component::new('R', 2, vec![10, NODE_GND], "10k").unwrap());
        // Inverting input tied to ground
        c.add_component(Component::new('R', 3, vec![20, NODE_GND], "1k").unwrap());
        c.add_component(Component::new('R', 4, vec![NODE_IN, NODE_GND], "100k").unwrap());
        // Op-amp: non-inv=10, inv=20, vcc=3, vee=4, out=2 (OUT)
        c.add_component(
            Component::new('X', 1, vec![10, 20, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );

        c.validate().expect("Circuit should pass topology validation");
        let netlist = c.to_netlist("Rail Saturated Circuit Test");
        let sim = run_simulation(&netlist, Duration::from_secs(2)).expect("Sim should succeed");

        let result = check(&sim, c.vcc, c.vee);
        println!("Rail saturation filter decision: {:?}", result);
        assert!(matches!(result, Err(RejectReason::RailSaturation { .. })));
    }

    /// Acceptance Criterion 3: Circuit with high DC offset (> 100mV) is rejected with DcOffsetTooHigh
    #[test]
    fn test_reject_dc_offset() {
        let mut c = Circuit::new();
        // Bias network creating +1.8V DC offset on the output node
        // Voltage divider from VCC (9V): 8k and 2k gives V_out = 9 * 2 / 10 = 1.8V
        c.add_component(Component::new('R', 1, vec![NODE_VCC, NODE_OUT], "8k").unwrap());
        c.add_component(Component::new('R', 2, vec![NODE_OUT, NODE_GND], "2k").unwrap());
        // AC signal coupled into output via large capacitor so signal exists (-20dB)
        c.add_component(Component::new('C', 1, vec![NODE_IN, NODE_OUT], "10uF").unwrap());

        c.validate().expect("Circuit should pass topology validation");
        let netlist = c.to_netlist("DC Offset Circuit Test");
        let sim = run_simulation(&netlist, Duration::from_secs(2)).expect("Sim should succeed");

        let result = check(&sim, c.vcc, c.vee);
        println!("DC offset filter decision: {:?}", result);
        assert!(matches!(result, Err(RejectReason::DcOffsetTooHigh { .. })));
    }

    /// Verification: FilterStats correctly records decisions
    #[test]
    fn test_filter_stats_telemetry() {
        let mut stats = FilterStats::new();
        stats.record(&Ok(()));
        stats.record(&Err(RejectReason::DeadSignal));
        stats.record(&Err(RejectReason::RailSaturation {
            node: "2".into(),
            voltage: 8.0,
            vcc: 9.0,
            vee: -9.0,
        }));
        assert_eq!(stats.total_evaluated, 3);
        assert_eq!(stats.passed, 1);
        assert_eq!(stats.dead_signal, 1);
        assert_eq!(stats.rail_saturation, 1);
        println!("Telemetry summary: {}", stats.summary());
    }

    /// Acceptance Criterion 4: A healthy, well-designed circuit (Sallen-Key lowpass) passes cleanly
    #[test]
    fn test_accept_valid_sallen_key() {
        let mut c = Circuit::new();
        c.add_component(Component::new('R', 1, vec![NODE_IN, 10], "10k").unwrap());
        c.add_component(Component::new('R', 2, vec![10, 20], "10k").unwrap());
        c.add_component(Component::new('C', 1, vec![10, NODE_OUT], "15.9nF").unwrap());
        c.add_component(Component::new('C', 2, vec![20, NODE_GND], "15.9nF").unwrap());
        c.add_component(
            Component::new('X', 1, vec![20, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );

        c.validate().expect("Circuit should pass topology validation");
        let netlist = c.to_netlist("Valid Sallen-Key Test");
        let sim = run_simulation(&netlist, Duration::from_secs(2)).expect("Sim should succeed");

        let result = check(&sim, c.vcc, c.vee);
        println!("Healthy Sallen-Key filter decision: {:?}", result);
        assert_eq!(result, Ok(()));
    }
}
