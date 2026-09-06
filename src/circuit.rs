use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

pub const NODE_GND: usize = 0;
pub const NODE_IN: usize = 1;
pub const NODE_OUT: usize = 2;
pub const NODE_VCC: usize = 3;
pub const NODE_VEE: usize = 4;

/// The single canonical SPICE model/include preamble shared by EVERY netlist
/// generator in the crate.
///
/// This preamble used to be copy-pasted into four separate generators, and two of
/// those copies silently omitted the BJT models. That made `to_tran_netlist` and the
/// entire Monte Carlo path fail on any circuit containing a transistor, which forced
/// `mc_dev_db` to `None` and capped every discrete discovery at COMMON rarity.
/// Never inline these lines again - always call this function.
pub fn standard_spice_headers() -> &'static str {
    concat!(
        ".include \"tl072.sub\"\n",
        ".model 1N4148 D(is=2.52n rs=0.568 n=1.752 cjo=4p m=0.4 tt=20n)\n",
        ".model 2N3904 NPN(Is=6.734f Xti=3 Eg=1.11 Vaf=74.03 Bf=416.4 Ne=1.259 Ise=6.734f Ikf=66.78m Xtb=1.5 Br=.7371 Nc=2 Isc=0 Ikr=0 Rc=1 Cjc=3.638p Mjc=.3085 Vjc=.75 Fc=.5 Cje=4.493p Mje=.2593 Vje=.75 Tr=239.5n Tf=301.2p Itf=.4 Vtf=4 Xtf=2 Rb=10 Kf=1.2e-16 Af=1.1)\n",
        ".model 2N3906 PNP(Is=1.41f Xti=3 Eg=1.11 Vaf=18.7 Bf=180.7 Ne=1.5 Ise=0 Ikf=80m Xtb=1.5 Br=4.977 Nc=2 Isc=0 Ikr=0 Rc=2 Cjc=4.5p Mjc=.3 Vjc=.75 Fc=.5 Cje=5p Mje=.3 Vje=.75 Tr=50n Tf=300p Itf=.4 Vtf=4 Xtf=2 Rb=10 Kf=1.5e-16 Af=1.1)\n",
    )
}

/// Permitted component types strictly white-listed.
/// Letters E, F, G, H, B, A are explicitly forbidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ComponentType {
    R, // Resistor
    C, // Capacitor
    D, // Diode
    Q, // BJT Transistor (3 pins: [collector, base, emitter])
    X, // Subcircuit (e.g. TL072 Op-Amp)
    V, // Independent Voltage Source
    I, // Independent Current Source
}

impl ComponentType {
    pub fn from_char(c: char) -> Result<Self, CircuitError> {
        match c.to_ascii_uppercase() {
            'R' => Ok(ComponentType::R),
            'C' => Ok(ComponentType::C),
            'D' => Ok(ComponentType::D),
            'Q' => Ok(ComponentType::Q),
            'X' => Ok(ComponentType::X),
            'V' => Ok(ComponentType::V),
            'I' => Ok(ComponentType::I),
            'E' | 'F' | 'G' | 'H' | 'B' | 'A' => {
                Err(CircuitError::ForbiddenComponentType(c))
            }
            other => Err(CircuitError::InvalidComponentType(other)),
        }
    }

    pub fn to_char(self) -> char {
        match self {
            ComponentType::R => 'R',
            ComponentType::C => 'C',
            ComponentType::D => 'D',
            ComponentType::Q => 'Q',
            ComponentType::X => 'X',
            ComponentType::V => 'V',
            ComponentType::I => 'I',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub comp_type: ComponentType,
    pub id: usize,
    pub nodes: Vec<usize>,
    pub value: String,
}

impl Component {
    /// Create a component. Rejects invalid/forbidden types, insufficient nodes,
    /// or shorted terminal connections.
    pub fn new(
        comp_type_char: char,
        id: usize,
        nodes: Vec<usize>,
        value: &str,
    ) -> Result<Self, CircuitError> {
        let comp_type = ComponentType::from_char(comp_type_char)?;

        // Verify minimum pin count
        let required_pins = match comp_type {
            ComponentType::R | ComponentType::C | ComponentType::D | ComponentType::V | ComponentType::I => 2,
            ComponentType::Q => 3, // [collector, base, emitter]
            ComponentType::X => 5, // [non-inv, inv, vcc, vee, out]
        };

        if nodes.len() < required_pins {
            return Err(CircuitError::InsufficientPins {
                comp: format!("{}{}", comp_type.to_char(), id),
                got: nodes.len(),
                expected: required_pins,
            });
        }

        // For two-terminal components, terminal nodes must not be the same (no short circuit)
        if (comp_type == ComponentType::R
            || comp_type == ComponentType::C
            || comp_type == ComponentType::D
            || comp_type == ComponentType::V
            || comp_type == ComponentType::I)
            && nodes[0] == nodes[1]
        {
            return Err(CircuitError::ShortedComponent {
                comp: format!("{}{}", comp_type.to_char(), id),
                node: nodes[0],
            });
        }

        // For BJT transistor, terminals must not all be shorted together
        if comp_type == ComponentType::Q && (nodes[0] == nodes[1] && nodes[1] == nodes[2]) {
            return Err(CircuitError::ShortedComponent {
                comp: format!("{}{}", comp_type.to_char(), id),
                node: nodes[0],
            });
        }

        Ok(Component {
            comp_type,
            id,
            nodes,
            value: value.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Circuit {
    pub nodes: HashSet<usize>,
    pub components: Vec<Component>,
    pub vcc: f64,
    pub vee: f64,
}

impl Default for Circuit {
    fn default() -> Self {
        let mut nodes = HashSet::new();
        nodes.insert(NODE_GND);
        nodes.insert(NODE_IN);
        nodes.insert(NODE_OUT);
        nodes.insert(NODE_VCC);
        nodes.insert(NODE_VEE);

        Circuit {
            nodes,
            components: Vec::new(),
            vcc: 9.0,
            vee: -9.0,
        }
    }
}

impl Circuit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a component to the circuit and register all its nodes
    pub fn add_component(&mut self, comp: Component) {
        for &node in &comp.nodes {
            self.nodes.insert(node);
        }
        self.components.push(comp);
    }

    /// Validate the circuit topology:
    /// 1. No float nodes: Every active non-ground node must be connected to at least 2 component pins
    ///    or be connected to external fixed nodes (IN, OUT, VCC, VEE).
    /// 2. No short-circuited component pins.
    /// 3. At least one path to ground (Node 0) from all active nodes.
    /// 4. Essential pins (IN, OUT) must be connected to components.
    pub fn validate(&self) -> Result<(), CircuitError> {
        if self.components.is_empty() {
            return Err(CircuitError::EmptyCircuit);
        }

        // Count pin attachments per node
        let mut pin_counts: HashMap<usize, usize> = HashMap::new();
        let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();

        for comp in &self.components {
            // Verify component terminal uniqueness for 2-pin devices
            if comp.nodes.len() == 2 && comp.nodes[0] == comp.nodes[1] {
                return Err(CircuitError::ShortedComponent {
                    comp: format!("{}{}", comp.comp_type.to_char(), comp.id),
                    node: comp.nodes[0],
                });
            }

            for &n in &comp.nodes {
                *pin_counts.entry(n).or_insert(0) += 1;
            }

            // Build adjacency graph for ground connectivity
            for i in 0..comp.nodes.len() {
                for j in (i + 1)..comp.nodes.len() {
                    let u = comp.nodes[i];
                    let v = comp.nodes[j];
                    adj.entry(u).or_default().push(v);
                    adj.entry(v).or_default().push(u);
                }
            }
        }

        // Essential node connectivity check
        if pin_counts.get(&NODE_IN).copied().unwrap_or(0) == 0 {
            return Err(CircuitError::DisconnectedEssentialNode("IN (node 1)".to_string()));
        }
        if pin_counts.get(&NODE_OUT).copied().unwrap_or(0) == 0 {
            return Err(CircuitError::DisconnectedEssentialNode("OUT (node 2)".to_string()));
        }

        // Float node check: Every internal node (not GND, IN, OUT, VCC, VEE)
        // must have at least 2 pin connections.
        for (&node, &count) in &pin_counts {
            if node != NODE_GND
                && node != NODE_IN
                && node != NODE_OUT
                && node != NODE_VCC
                && node != NODE_VEE
                && count < 2
            {
                return Err(CircuitError::FloatingNode(node));
            }
        }

        // Connectivity check: BFS from ground (Node 0) to ensure path to ground exists
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        // Node 0, Node 3 (VCC source), Node 4 (VEE source), Node 1 (IN source)
        // all have implicit DC/AC sources connected to ground in to_netlist().
        let ground_anchors = [NODE_GND, NODE_IN, NODE_VCC, NODE_VEE];
        for &anchor in &ground_anchors {
            if pin_counts.contains_key(&anchor) {
                visited.insert(anchor);
                queue.push_back(anchor);
            }
        }

        while let Some(curr) = queue.pop_front() {
            if let Some(neighbors) = adj.get(&curr) {
                for &next in neighbors {
                    if visited.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
        }

        // Ensure all active nodes in the circuit connect back to ground
        for &node in pin_counts.keys() {
            if !visited.contains(&node) {
                return Err(CircuitError::NoPathToGround(node));
            }
        }

        Ok(())
    }

    /// Generate a standard SPICE netlist string from the circuit IR
    pub fn to_netlist(&self, title: &str) -> String {
        let mut netlist = String::new();

        netlist.push_str(&format!("* {}\n", title));
        netlist.push_str(standard_spice_headers());

        // Power supply sources and input signal source
        netlist.push_str(&format!("V_in {} {} dc 0 ac 1\n", NODE_IN, NODE_GND));
        netlist.push_str(&format!("V_cc {} {} dc {:.2}\n", NODE_VCC, NODE_GND, self.vcc));
        netlist.push_str(&format!("V_ee {} {} dc {:.2}\n", NODE_VEE, NODE_GND, self.vee));

        // Component declarations
        for comp in &self.components {
            let prefix = comp.comp_type.to_char();
            let nodes_str = comp
                .nodes
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(" ");

            netlist.push_str(&format!("{}{:<4} {} {}\n", prefix, comp.id, nodes_str, comp.value));
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

    /// Generate a SPICE netlist string with transient analysis (sine wave input)
    pub fn to_tran_netlist(
        &self,
        title: &str,
        v_in_amp: f64,
        freq: f64,
        duration: f64,
    ) -> String {
        let mut netlist = String::new();

        netlist.push_str(&format!("* {}\n", title));
        netlist.push_str(standard_spice_headers());

        // Power supply sources and sine input source
        netlist.push_str(&format!(
            "V_in {} {} sin(0 {:.4} {:.1})\n",
            NODE_IN, NODE_GND, v_in_amp, freq
        ));
        netlist.push_str(&format!("V_cc {} {} dc {:.2}\n", NODE_VCC, NODE_GND, self.vcc));
        netlist.push_str(&format!("V_ee {} {} dc {:.2}\n", NODE_VEE, NODE_GND, self.vee));

        // Component declarations
        for comp in &self.components {
            let prefix = comp.comp_type.to_char();
            let nodes_str = comp
                .nodes
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(" ");

            netlist.push_str(&format!("{}{:<4} {} {}\n", prefix, comp.id, nodes_str, comp.value));
        }

        // 50 time steps per period for high fidelity waveforms
        let step = 1.0 / (freq * 50.0);
        netlist.push_str(".control\n");
        netlist.push_str("op\n");
        netlist.push_str("print allv\n");
        netlist.push_str(&format!("tran {:.6e} {:.6e}\n", step, duration));
        netlist.push_str(&format!("wrdata tran_out.txt v({}) v({})\n", NODE_OUT, NODE_IN));
        netlist.push_str("quit\n");
        netlist.push_str(".endc\n");
        netlist.push_str(".end\n");

        netlist
    }
}

#[derive(Debug, PartialEq)]
pub enum CircuitError {
    InvalidComponentType(char),
    ForbiddenComponentType(char),
    InsufficientPins { comp: String, got: usize, expected: usize },
    ShortedComponent { comp: String, node: usize },
    EmptyCircuit,
    FloatingNode(usize),
    DisconnectedEssentialNode(String),
    NoPathToGround(usize),
}

impl std::fmt::Display for CircuitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitError::InvalidComponentType(c) => write!(f, "Invalid component type: '{}'", c),
            CircuitError::ForbiddenComponentType(c) => {
                write!(f, "Component type '{}' is strictly forbidden (E, F, G, H, B, A are not allowed)", c)
            }
            CircuitError::InsufficientPins { comp, got, expected } => {
                write!(f, "Component {} has {} pins, expected at least {}", comp, got, expected)
            }
            CircuitError::ShortedComponent { comp, node } => {
                write!(f, "Component {} has shorted terminals on node {}", comp, node)
            }
            CircuitError::EmptyCircuit => write!(f, "Circuit has no components"),
            CircuitError::FloatingNode(n) => write!(f, "Node {} is floating (less than 2 connections)", n),
            CircuitError::DisconnectedEssentialNode(name) => write!(f, "Essential node {} is not connected", name),
            CircuitError::NoPathToGround(n) => write!(f, "Node {} has no conductive path to ground", n),
        }
    }
}

impl std::error::Error for CircuitError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard for the netlist-header duplication bug.
    ///
    /// The SPICE preamble was once copy-pasted into four generators and two copies
    /// silently omitted the BJT models. Nothing failed to compile; instead every
    /// transistor circuit failed at simulation time, which silently disabled the whole
    /// Monte Carlo robustness path. If you add a new netlist generator, add it here.
    #[test]
    fn every_netlist_generator_declares_every_device_model() {
        let mut c = Circuit::new();
        c.add_component(Component::new('Q', 1, vec![NODE_VCC, 10, NODE_OUT], "2N3904").unwrap());
        c.add_component(Component::new('R', 1, vec![NODE_VCC, 10], "100k").unwrap());
        c.add_component(Component::new('R', 2, vec![10, NODE_VEE], "110k").unwrap());
        c.add_component(Component::new('R', 3, vec![NODE_OUT, NODE_VEE], "4.7k").unwrap());
        c.add_component(Component::new('C', 1, vec![NODE_IN, 10], "1uF").unwrap());
        c.validate().unwrap();

        let preset = crate::preset::Preset::buffer_default();
        let generated = [
            ("to_netlist", c.to_netlist("t")),
            ("to_tran_netlist", c.to_tran_netlist("t", 0.1, 1000.0, 0.005)),
            ("to_realistic_netlist", crate::realism::to_realistic_netlist(&c, "t", 22.0)),
            ("to_characterization_netlist", crate::fitness::to_characterization_netlist(&c, "t", 22.0)),
            ("to_op_netlist", crate::fitness::to_op_netlist(&c)),
            ("to_probe_netlist", crate::fitness::to_probe_netlist(&c, &preset, 22.0)),
            ("to_noise_netlist", crate::fitness::to_noise_netlist(&c, "t", 600.0, 10000.0, 10.0, 100000.0, 22.0)),
        ];

        for (name, netlist) in &generated {
            for required in [".model 2N3904", ".model 2N3906", ".model 1N4148", "tl072.sub"] {
                assert!(
                    netlist.contains(required),
                    "netlist generator `{}` does not declare `{}`; a circuit using that                      device would fail in ngspice with an unknown-model error",
                    name,
                    required
                );
            }
        }
    }

    use crate::spice::run_simulation;
    use std::time::Duration;

    /// Verify whitelist and hard failure on forbidden element letters E, F, G, H, B, A
    #[test]
    fn test_whitelist_enforcement() {
        let forbidden = ['E', 'e', 'F', 'f', 'G', 'g', 'H', 'h', 'B', 'b', 'A', 'a'];
        for &letter in &forbidden {
            let res = Component::new(letter, 1, vec![1, 2], "1.0");
            assert!(
                matches!(res, Err(CircuitError::ForbiddenComponentType(_))),
                "Letter '{}' should be strictly forbidden",
                letter
            );
        }

        // Valid letters must succeed
        assert!(Component::new('R', 1, vec![1, 2], "1k").is_ok());
        assert!(Component::new('C', 1, vec![1, 2], "100n").is_ok());
        assert!(Component::new('D', 1, vec![1, 2], "1N4148").is_ok());
        assert!(Component::new('X', 1, vec![20, 2, 3, 4, 2], "TL072").is_ok());
        assert!(Component::new('V', 1, vec![1, 0], "5").is_ok());
        assert!(Component::new('I', 1, vec![1, 0], "1m").is_ok());
    }

    /// Verify validation rules (shorted component, floating node, path to ground)
    #[test]
    fn test_validation_rules() {
        // 1. Shorted component
        let shorted_res = Component::new('R', 1, vec![5, 5], "1k");
        assert!(matches!(shorted_res, Err(CircuitError::ShortedComponent { .. })));

        // 2. Floating node in circuit
        let mut c_float = Circuit::new();
        // R1: IN -> Node 10
        c_float.add_component(Component::new('R', 1, vec![NODE_IN, 10], "1k").unwrap());
        // R2: OUT -> GND
        c_float.add_component(Component::new('R', 2, vec![NODE_OUT, NODE_GND], "1k").unwrap());
        // Node 10 is connected to only R1 (floating node)
        let float_val = c_float.validate();
        assert!(matches!(float_val, Err(CircuitError::FloatingNode(10))));

        // 3. Isolated island with no path to ground
        let mut c_island = Circuit::new();
        c_island.add_component(Component::new('R', 1, vec![NODE_IN, NODE_OUT], "1k").unwrap());
        c_island.add_component(Component::new('R', 2, vec![NODE_OUT, NODE_GND], "1k").unwrap());
        // Floating loop between node 50 and 60
        c_island.add_component(Component::new('R', 3, vec![50, 60], "1k").unwrap());
        c_island.add_component(Component::new('R', 4, vec![60, 50], "2k").unwrap());
        let island_val = c_island.validate();
        assert!(matches!(island_val, Err(CircuitError::NoPathToGround(50 | 60))));
    }

    /// Stage 2 Acceptance Criterion:
    /// Hand-built Sallen-Key lowpass filter Circuit passes validation,
    /// compiles to SPICE netlist, simulates via S1 spice::run_simulation,
    /// and yields the correct frequency response (flat passband ~0dB, 2nd order steep drop).
    #[test]
    fn test_sallen_key_lowpass_acceptance() {
        let mut c = Circuit::new();

        // Standard Sallen-Key Lowpass Topology:
        // R1: IN (1) -> Node 10
        // R2: Node 10 -> Node 20 (Non-inverting input of op-amp)
        // C1: Node 10 -> OUT (2) (Positive feedback)
        // C2: Node 20 -> GND (0)
        // X1: TL072 Buffer [non-inv: 20, inv: 2 (OUT), vcc: 3, vee: 4, out: 2]
        c.add_component(Component::new('R', 1, vec![NODE_IN, 10], "10k").unwrap());
        c.add_component(Component::new('R', 2, vec![10, 20], "10k").unwrap());
        c.add_component(Component::new('C', 1, vec![10, NODE_OUT], "15.9nF").unwrap());
        c.add_component(Component::new('C', 2, vec![20, NODE_GND], "15.9nF").unwrap());
        c.add_component(
            Component::new('X', 1, vec![20, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );

        // Validation must pass cleanly
        c.validate().expect("Sallen-Key circuit should be valid");

        // Generate netlist
        let netlist = c.to_netlist("Sallen-Key 2nd Order Lowpass Filter");
        println!("Generated Netlist:\n{}", netlist);

        // Run through Stage 1 simulation engine
        let res = run_simulation(&netlist, Duration::from_secs(2)).expect("Simulation failed");
        let ac = res.ac_response.expect("AC response missing");
        assert!(!ac.is_empty(), "AC points should not be empty");

        // 1. Passband gain at low frequency (10 Hz) should be unity gain (~0 dB)
        let low_freq_point = &ac[0];
        println!(
            "Low frequency ({} Hz): {:.2} dB",
            low_freq_point.freq, low_freq_point.mag_db
        );
        assert!(
            (low_freq_point.mag_db - 0.0).abs() < 0.2,
            "Passband gain should be ~0dB, got {:.2}dB",
            low_freq_point.mag_db
        );

        // 2. Response at ~1 kHz cutoff frequency should show expected -6dB (equal R, equal C Q=0.5)
        let target_freq = 1000.0;
        let cutoff_pt = ac
            .iter()
            .min_by(|a, b| {
                (a.freq - target_freq)
                    .abs()
                    .partial_cmp(&(b.freq - target_freq).abs())
                    .unwrap()
            })
            .expect("Should find 1kHz point");

        println!(
            "Cutoff area ({} Hz): {:.2} dB, Phase: {:.2}°",
            cutoff_pt.freq, cutoff_pt.mag_db, cutoff_pt.phase_deg
        );
        assert!(
            (cutoff_pt.mag_db - (-6.0)).abs() < 1.0,
            "Cutoff magnitude at 1kHz should be ~ -6dB, got {:.2}dB",
            cutoff_pt.mag_db
        );

        // 3. Stopband at 10 kHz (one decade higher) should show steep 2nd order roll-off (< -30 dB)
        let stop_freq = 10000.0;
        let stop_pt = ac
            .iter()
            .min_by(|a, b| {
                (a.freq - stop_freq)
                    .abs()
                    .partial_cmp(&(b.freq - stop_freq).abs())
                    .unwrap()
            })
            .expect("Should find 10kHz point");

        println!(
            "Stopband ({} Hz): {:.2} dB",
            stop_pt.freq, stop_pt.mag_db
        );
        assert!(
            stop_pt.mag_db < -30.0,
            "10kHz stopband attenuation should be < -30dB, got {:.2}dB",
            stop_pt.mag_db
        );
    }

    /// Verification 1: Verify .op output accurately captures op-amp rail saturation
    #[test]
    fn test_op_rail_saturation() {
        let mut c = Circuit::new();

        // Deliberately drive op-amp into positive saturation:
        // Voltage divider from VCC (+9V):
        // R1: VCC (3) -> Node 10 (10k)
        // R2: Node 10 -> GND (0) (10k)  => V(10) = +4.5V
        // Inverting input tied to GND via R3: Node 20 -> GND (0) (1k) => V(20) = 0V
        // Input pin IN (1) connected to GND via 100k
        // Op-amp X1: non-inv=10, inv=20, vcc=3, vee=4, out=2 (OUT)
        // With V+ = 4.5V and V- = 0V, op-amp output must saturate to positive rail (+7.5V ~ +8.8V).
        c.add_component(Component::new('R', 1, vec![NODE_VCC, 10], "10k").unwrap());
        c.add_component(Component::new('R', 2, vec![10, NODE_GND], "10k").unwrap());
        c.add_component(Component::new('R', 3, vec![20, NODE_GND], "1k").unwrap());
        c.add_component(Component::new('R', 4, vec![NODE_IN, NODE_GND], "100k").unwrap());
        c.add_component(
            Component::new('X', 1, vec![10, 20, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );

        c.validate().expect("Circuit should validate");
        let netlist = c.to_netlist("OpAmp Positive Rail Saturation Test");
        let res = run_simulation(&netlist, Duration::from_secs(2)).expect("Simulation should run");

        // Verify that .op output contains the output node
        let out_v = res
            .dc_nodes
            .get("2")
            .copied()
            .or_else(|| res.dc_nodes.get("v(2)").copied())
            .expect("Output node '2' / 'v(2)' must exist in dc_nodes");

        println!(
            "Op-amp saturated DC output voltage: {:.3} V (VCC = {:.1} V)",
            out_v, c.vcc
        );

        // Saturation check: with +9V supply, saturated TL072 should be > 7.0V
        assert!(
            out_v > 7.0,
            "Expected op-amp output to be saturated near +9V rail (>7.0V), but got {:.3}V",
            out_v
        );
    }

    /// Verification 2: Verify .tran captures non-linear diode clipping & asymmetry
    #[test]
    fn test_diode_clipping_nonlinearity() {
        let mut c = Circuit::new();

        // Asymmetrical Diode Clipper:
        // R1: IN (1) -> OUT (2) (1k)
        // D1: OUT (2) -> GND (0) (1N4148, anode at 2, cathode at 0)
        // R2: OUT (2) -> GND (0) (100k load)
        c.add_component(Component::new('R', 1, vec![NODE_IN, NODE_OUT], "1k").unwrap());
        c.add_component(Component::new('D', 1, vec![NODE_OUT, NODE_GND], "1N4148").unwrap());
        c.add_component(Component::new('R', 2, vec![NODE_OUT, NODE_GND], "100k").unwrap());

        c.validate().expect("Clipper circuit should validate");

        // Test A: Small signal (100mV peak at 1kHz). Diode should NOT conduct.
        // Waveform should be symmetric: V_max ≈ +0.10V, V_min ≈ -0.10V.
        let netlist_small = c.to_tran_netlist("Diode Clipper Small Signal", 0.10, 1000.0, 0.003);
        let res_small =
            run_simulation(&netlist_small, Duration::from_secs(2)).expect("Small signal sim failed");
        let tran_small = res_small.tran_response.expect("Tran response missing");

        // Skip first 1ms (settling) and analyze 1ms to 3ms
        let steady_small: Vec<_> = tran_small.iter().filter(|p| p.time >= 0.001).collect();
        let max_small = steady_small
            .iter()
            .map(|p| p.v_out)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_small = steady_small
            .iter()
            .map(|p| p.v_out)
            .fold(f64::INFINITY, f64::min);
        let ratio_small = max_small.abs() / min_small.abs();

        println!(
            "Small Signal (100mV): V_max = {:.4} V, V_min = {:.4} V, Symmetry Ratio = {:.3}",
            max_small, min_small, ratio_small
        );
        // Should be highly symmetrical (ratio ≈ 1.0 ± 0.05)
        assert!(
            (ratio_small - 1.0).abs() < 0.05,
            "Small signal should be symmetrical (~1.0), got {:.3}",
            ratio_small
        );

        // Test B: Large signal (2.0V peak at 1kHz). Diode turns ON on positive half-cycle.
        // Positive cycle clamps to ~0.65V, negative cycle swings to -2.0V!
        let netlist_large = c.to_tran_netlist("Diode Clipper Large Signal", 2.0, 1000.0, 0.003);
        let res_large =
            run_simulation(&netlist_large, Duration::from_secs(2)).expect("Large signal sim failed");
        let tran_large = res_large.tran_response.expect("Tran response missing");

        let steady_large: Vec<_> = tran_large.iter().filter(|p| p.time >= 0.001).collect();
        let max_large = steady_large
            .iter()
            .map(|p| p.v_out)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_large = steady_large
            .iter()
            .map(|p| p.v_out)
            .fold(f64::INFINITY, f64::min);
        let ratio_large = max_large.abs() / min_large.abs();

        println!(
            "Large Signal (2.0V): V_max = {:.4} V, V_min = {:.4} V, Asymmetry Ratio = {:.3}",
            max_large, min_large, ratio_large
        );

        // Diode clamping asserts:
        assert!(
            max_large < 0.80,
            "Positive peak should be clipped below 0.8V by diode, got {:.3}V",
            max_large
        );
        assert!(
            min_large < -1.80,
            "Negative peak should swing cleanly near -2.0V, got {:.3}V",
            min_large
        );
        assert!(
            ratio_large < 0.50,
            "Asymmetry ratio must be < 0.50 showing massive non-linear distortion, got {:.3}",
            ratio_large
        );
    }
}
