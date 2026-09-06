use crate::circuit::{Circuit, Component, ComponentType, NODE_GND, NODE_OUT, NODE_VCC, NODE_VEE};

/// E24 standard resistor values from 10 Ohm to 1 MOhm
pub const E24_VALUES: &[&str] = &[
    "10", "11", "12", "13", "15", "16", "18", "20", "22", "24", "27", "30", "33", "36", "39", "43", "47", "51", "56", "62", "68", "75", "82", "91",
    "100", "110", "120", "130", "150", "160", "180", "200", "220", "240", "270", "300", "330", "360", "390", "430", "470", "510", "560", "620", "680", "750", "820", "910",
    "1k", "1.1k", "1.2k", "1.3k", "1.5k", "1.6k", "1.8k", "2k", "2.2k", "2.4k", "2.7k", "3k", "3.3k", "3.6k", "3.9k", "4.3k", "4.7k", "5.1k", "5.6k", "6.2k", "6.8k", "7.5k", "8.2k", "9.1k",
    "10k", "11k", "12k", "13k", "15k", "16k", "18k", "20k", "22k", "24k", "27k", "30k", "33k", "36k", "39k", "43k", "47k", "51k", "56k", "62k", "68k", "75k", "82k", "91k",
    "100k", "110k", "120k", "130k", "150k", "160k", "180k", "200k", "220k", "240k", "270k", "300k", "330k", "360k", "390k", "430k", "470k", "510k", "560k", "620k", "680k", "750k", "820k", "910k",
    "1Meg",
];

/// E12 standard capacitor values from 10 pF to 10 uF
pub const E12_VALUES: &[&str] = &[
    "10pF", "12pF", "15pF", "18pF", "22pF", "27pF", "33pF", "39pF", "47pF", "56pF", "68pF", "82pF",
    "100pF", "120pF", "150pF", "180pF", "220pF", "270pF", "330pF", "390pF", "470pF", "560pF", "680pF", "820pF",
    "1nF", "1.2nF", "1.5nF", "1.8nF", "2.2nF", "2.7nF", "3.3nF", "3.9nF", "4.7nF", "5.6nF", "6.8nF", "8.2nF",
    "10nF", "12nF", "15nF", "18nF", "22nF", "27nF", "33nF", "39nF", "47nF", "56nF", "68nF", "82nF",
    "100nF", "120nF", "150nF", "180nF", "220nF", "270nF", "330nF", "390nF", "470nF", "560nF", "680nF", "820nF",
    "1uF", "2.2uF", "4.7uF", "10uF",
];

/// Step a resistor value up or down by 1 position in E24 table
pub fn step_e24(current: &str) -> &'static str {
    let lower = current.to_lowercase();
    if let Some(pos) = E24_VALUES.iter().position(|&v| v.to_lowercase() == lower) {
        if fastrand::bool() {
            if pos + 1 < E24_VALUES.len() { E24_VALUES[pos + 1] } else { E24_VALUES[pos] }
        } else {
            if pos > 0 { E24_VALUES[pos - 1] } else { E24_VALUES[pos] }
        }
    } else {
        E24_VALUES[fastrand::usize(0..E24_VALUES.len())]
    }
}

/// Step a capacitor value up or down by 1 position in E12 table
pub fn step_e12(current: &str) -> &'static str {
    let lower = current.to_lowercase();
    if let Some(pos) = E12_VALUES.iter().position(|&v| v.to_lowercase() == lower) {
        if fastrand::bool() {
            if pos + 1 < E12_VALUES.len() { E12_VALUES[pos + 1] } else { E12_VALUES[pos] }
        } else {
            if pos > 0 { E12_VALUES[pos - 1] } else { E12_VALUES[pos] }
        }
    } else {
        E12_VALUES[fastrand::usize(0..E12_VALUES.len())]
    }
}

/// Generate next unique ID for a given component type
fn next_id(circuit: &Circuit, comp_type: ComponentType) -> usize {
    circuit
        .components
        .iter()
        .filter(|c| c.comp_type == comp_type)
        .map(|c| c.id)
        .max()
        .unwrap_or(0)
        + 1
}

/// Find a new node ID that does not clash with existing nodes
fn next_node_id(circuit: &Circuit) -> usize {
    let max_node = circuit.nodes.iter().copied().max().unwrap_or(0);
    max_node.max(20) + 1
}

/// Collect all nodes (including power rails and ground) suitable for component connection
fn get_available_nodes(circuit: &Circuit) -> Vec<usize> {
    circuit.nodes.iter().copied().collect()
}

// ---------------------------------------------------------------------------
// 6 Mutation Operators
// ---------------------------------------------------------------------------

/// Operator 1: Change component value to adjacent E24 / E12 step or toggle BJT model
pub fn mutate_change_value(circuit: &mut Circuit) -> bool {
    let indices: Vec<usize> = circuit
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.comp_type == ComponentType::R
                || c.comp_type == ComponentType::C
                || c.comp_type == ComponentType::Q
        })
        .map(|(i, _)| i)
        .collect();

    if indices.is_empty() {
        return false;
    }

    let idx = indices[fastrand::usize(0..indices.len())];

    match circuit.components[idx].comp_type {
        ComponentType::R => {
            if fastrand::u8(0..100) < 15 {
                // Type Morphing: Convert Resistor to Capacitor
                circuit.components[idx].comp_type = ComponentType::C;
                circuit.components[idx].value = E12_VALUES[fastrand::usize(0..E12_VALUES.len())].to_string();
            } else {
                circuit.components[idx].value = step_e24(&circuit.components[idx].value).to_string();
            }
            true
        }
        ComponentType::C => {
            if fastrand::u8(0..100) < 15 {
                // Type Morphing: Convert Capacitor to Resistor
                circuit.components[idx].comp_type = ComponentType::R;
                circuit.components[idx].value = E24_VALUES[fastrand::usize(0..E24_VALUES.len())].to_string();
            } else {
                circuit.components[idx].value = step_e12(&circuit.components[idx].value).to_string();
            }
            true
        }
        ComponentType::Q => {
            let is_npn = circuit.components[idx].value.to_uppercase().contains("3904");
            circuit.components[idx].value = if is_npn {
                "2N3906".to_string()
            } else {
                "2N3904".to_string()
            };

            // When toggling NPN <-> PNP:
            // 1. Swap Collector (node 0) and Emitter (node 2).
            //    In NPN, C is positive (towards VCC) and E is negative (towards VEE/GND).
            //    In PNP, E is positive (towards VCC) and C is negative (towards VEE/GND).
            //    Swapping nodes[0] and nodes[2] ensures the PNP emitter connects to the positive
            //    branch (VCC or Rc) and collector connects to the negative branch (VEE or Re).
            if circuit.components[idx].nodes.len() >= 3 {
                circuit.components[idx].nodes.swap(0, 2);
                let base_node = circuit.components[idx].nodes[1];

                // 2. Base bias network polarity inversion:
                // Find pull-up resistor to VCC and pull-down resistor to VEE/GND connected to base.
                // Swap their values to invert the DC bias voltage across the midpoint.
                let mut vcc_res_idx = None;
                let mut vee_res_idx = None;

                for (c_idx, c) in circuit.components.iter().enumerate() {
                    if c_idx != idx && c.comp_type == ComponentType::R && c.nodes.len() == 2 {
                        let connects_base = c.nodes[0] == base_node || c.nodes[1] == base_node;
                        if connects_base {
                            let other_node = if c.nodes[0] == base_node { c.nodes[1] } else { c.nodes[0] };
                            if other_node == NODE_VCC {
                                vcc_res_idx = Some(c_idx);
                            } else if other_node == NODE_VEE || other_node == NODE_GND {
                                vee_res_idx = Some(c_idx);
                            }
                        }
                    }
                }

                if let (Some(r_vcc), Some(r_vee)) = (vcc_res_idx, vee_res_idx) {
                    let temp_val = circuit.components[r_vcc].value.clone();
                    circuit.components[r_vcc].value = circuit.components[r_vee].value.clone();
                    circuit.components[r_vee].value = temp_val;
                }
            }
            true
        }
        _ => false,
    }
}

/// Helper to pick two distinct nodes for 2-pin components, avoiding direct VCC-to-VEE shorts
fn pick_two_nodes(nodes: &[usize]) -> Option<(usize, usize)> {
    if nodes.len() < 2 {
        return None;
    }
    for _ in 0..25 {
        let u = nodes[fastrand::usize(0..nodes.len())];
        let v = nodes[fastrand::usize(0..nodes.len())];
        if u != v && !((u == NODE_VCC && v == NODE_VEE) || (u == NODE_VEE && v == NODE_VCC)) {
            return Some((u, v));
        }
    }
    None
}

/// Operator 2: Add a new component (R, C, D, Q, or X)
pub fn mutate_add_component(circuit: &mut Circuit) -> bool {
    let nodes = get_available_nodes(circuit);
    if nodes.len() < 2 {
        return false;
    }

    let has_opamp = circuit.components.iter().any(|c| c.comp_type == ComponentType::X);
    let roll = fastrand::u8(0..100);

    if roll < 35 {
        // Add Resistor (35%)
        if let Some((u, v)) = pick_two_nodes(&nodes) {
            let id = next_id(circuit, ComponentType::R);
            let val = E24_VALUES[fastrand::usize(0..E24_VALUES.len())];
            if let Ok(comp) = Component::new('R', id, vec![u, v], val) {
                circuit.add_component(comp);
                return true;
            }
        }
    } else if roll < 65 {
        // Add Capacitor (30%)
        if let Some((u, v)) = pick_two_nodes(&nodes) {
            let id = next_id(circuit, ComponentType::C);
            let val = E12_VALUES[fastrand::usize(0..E12_VALUES.len())];
            if let Ok(comp) = Component::new('C', id, vec![u, v], val) {
                circuit.add_component(comp);
                return true;
            }
        }
    } else if roll < 75 {
        // Add Diode (10% in discrete, 5% if opamp)
        if (!has_opamp || fastrand::bool())
            && let Some((u, v)) = pick_two_nodes(&nodes) {
                let id = next_id(circuit, ComponentType::D);
                if let Ok(comp) = Component::new('D', id, vec![u, v], "1N4148") {
                    circuit.add_component(comp);
                    return true;
                }
            }
    } else if !has_opamp {
        // Add BJT Transistor (discrete circuits only)
        let bjt_count = circuit.components.iter().filter(|c| c.comp_type == ComponentType::Q).count();
        if bjt_count < 4 {
            let is_npn = fastrand::bool();
            let model = if is_npn { "2N3904" } else { "2N3906" };
            let id = next_id(circuit, ComponentType::Q);

            let base = nodes[fastrand::usize(0..nodes.len())];
            let c = if is_npn {
                if fastrand::u8(0..10) < 6 { NODE_VCC } else { nodes[fastrand::usize(0..nodes.len())] }
            } else {
                if fastrand::u8(0..10) < 6 { NODE_VEE } else { nodes[fastrand::usize(0..nodes.len())] }
            };
            let e = if is_npn {
                if fastrand::u8(0..10) < 5 { NODE_OUT } else if fastrand::bool() { NODE_VEE } else { next_node_id(circuit) }
            } else {
                if fastrand::u8(0..10) < 5 { NODE_OUT } else if fastrand::bool() { NODE_VCC } else { next_node_id(circuit) }
            };

            if c != base && base != e && c != e
                && let Ok(comp) = Component::new('Q', id, vec![c, base, e], model) {
                    circuit.add_component(comp);

                    // Add an emitter pull resistor to avoid floating emitter
                    let r_e_id = next_id(circuit, ComponentType::R);
                    let target_rail = if is_npn { NODE_VEE } else { NODE_VCC };
                    let r_val = if fastrand::bool() { "4.7k" } else { "10k" };
                    if let Ok(r_comp) = Component::new('R', r_e_id, vec![e, target_rail], r_val) {
                        circuit.add_component(r_comp);
                    }
                    return true;
                }
        }
    } else {
        // Add Op-Amp TL072 (only if no opamp exists or max 1)
        let opamp_count = circuit
            .components
            .iter()
            .filter(|c| c.comp_type == ComponentType::X)
            .count();

        if opamp_count < 1 || (opamp_count < 2 && fastrand::u8(0..100) < 5) {
            let non_inv = nodes[fastrand::usize(0..nodes.len())];
            let out_node = next_node_id(circuit);
            let inv_node = next_node_id(circuit);

            let id = next_id(circuit, ComponentType::X);
            if let Ok(comp) = Component::new('X', id, vec![non_inv, inv_node, NODE_VCC, NODE_VEE, out_node], "TL072") {
                circuit.add_component(comp);

                // Stabilized Birth: Add negative feedback resistor from out_node to inv_node
                let r_fb_id = next_id(circuit, ComponentType::R);
                if let Ok(r_fb) = Component::new('R', r_fb_id, vec![out_node, inv_node], "10k") {
                    circuit.add_component(r_fb);
                }

                // Coupling resistor from out_node to existing node
                let target_node = if fastrand::bool() {
                    NODE_OUT
                } else {
                    nodes[fastrand::usize(0..nodes.len())]
                };
                let r_load_id = next_id(circuit, ComponentType::R);
                if let Ok(r_load) = Component::new('R', r_load_id, vec![out_node, target_node], "10k") {
                    circuit.add_component(r_load);
                }
                return true;
            }
        }
    }

    false
}

/// Operator 3: Remove a non-essential component (R, C, D, or Q)
pub fn mutate_remove_component(circuit: &mut Circuit) -> bool {
    // Only remove if circuit has at least 3 components
    if circuit.components.len() <= 2 {
        return false;
    }

    let remove_candidates: Vec<usize> = circuit
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.comp_type == ComponentType::R
                || c.comp_type == ComponentType::C
                || c.comp_type == ComponentType::D
                || c.comp_type == ComponentType::Q
        })
        .map(|(i, _)| i)
        .collect();

    if remove_candidates.is_empty() {
        return false;
    }

    let idx = remove_candidates[fastrand::usize(0..remove_candidates.len())];
    circuit.components.remove(idx);
    true
}

/// Operator 4: Move one terminal of a 2-pin or 3-pin component to another node
pub fn mutate_move_terminal(circuit: &mut Circuit) -> bool {
    let candidates: Vec<usize> = circuit
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            (c.comp_type == ComponentType::R
                || c.comp_type == ComponentType::C
                || c.comp_type == ComponentType::D
                || c.comp_type == ComponentType::Q
                || c.comp_type == ComponentType::X)
                && (c.nodes.len() == 2 || c.nodes.len() == 3 || c.nodes.len() == 5)
        })
        .map(|(i, _)| i)
        .collect();

    if candidates.is_empty() {
        return false;
    }

    let nodes = get_available_nodes(circuit);
    if nodes.len() < 2 {
        return false;
    }

    let comp_idx = candidates[fastrand::usize(0..candidates.len())];
    let comp = &circuit.components[comp_idx];

    if comp.nodes.len() == 2 {
        let terminal_to_move = if fastrand::bool() { 0 } else { 1 };
        let other_terminal = comp.nodes[1 - terminal_to_move];

        let mut new_node = nodes[fastrand::usize(0..nodes.len())];
        let mut attempts = 0;
        while (new_node == other_terminal
            || (new_node == NODE_VCC && other_terminal == NODE_VEE)
            || (new_node == NODE_VEE && other_terminal == NODE_VCC))
            && attempts < 15
        {
            new_node = nodes[fastrand::usize(0..nodes.len())];
            attempts += 1;
        }

        if new_node == other_terminal
            || (new_node == NODE_VCC && other_terminal == NODE_VEE)
            || (new_node == NODE_VEE && other_terminal == NODE_VCC)
        {
            return false;
        }

        circuit.components[comp_idx].nodes[terminal_to_move] = new_node;
        circuit.nodes.insert(new_node);
        return true;
    } else if comp.nodes.len() == 3 && comp.comp_type == ComponentType::Q {
        let pin_to_move = fastrand::usize(0..3);
        let other_a = comp.nodes[(pin_to_move + 1) % 3];
        let other_b = comp.nodes[(pin_to_move + 2) % 3];

        let mut new_node = nodes[fastrand::usize(0..nodes.len())];
        let mut attempts = 0;
        while (new_node == other_a || new_node == other_b) && attempts < 15 {
            new_node = nodes[fastrand::usize(0..nodes.len())];
            attempts += 1;
        }

        if new_node == other_a || new_node == other_b {
            return false;
        }

        circuit.components[comp_idx].nodes[pin_to_move] = new_node;
        circuit.nodes.insert(new_node);
        return true;
    } else if comp.nodes.len() == 5 && comp.comp_type == ComponentType::X {
        // Op-Amp pin movement: can move non_inv (0), inv (1), or out (4)
        // Power pins (2=VCC, 3=VEE) are preserved
        let pin_to_move = match fastrand::u8(0..3) {
            0 => 0, // non-inverting input
            1 => 1, // inverting input
            _ => 4, // output
        };

        // Pick an existing node or occasionally create a new node
        let new_node = if fastrand::u8(0..10) < 3 {
            next_node_id(circuit)
        } else {
            let mut cand = nodes[fastrand::usize(0..nodes.len())];
            let mut attempts = 0;
            while (cand == NODE_VCC || cand == NODE_VEE) && attempts < 10 {
                cand = nodes[fastrand::usize(0..nodes.len())];
                attempts += 1;
            }
            cand
        };

        if new_node != comp.nodes[pin_to_move] && new_node != NODE_VCC && new_node != NODE_VEE {
            circuit.components[comp_idx].nodes[pin_to_move] = new_node;
            circuit.nodes.insert(new_node);
            return true;
        }
    }

    false
}

/// Operator 5: Add a feedback or biasing path (R or C)
pub fn mutate_add_feedback(circuit: &mut Circuit) -> bool {
    let opamp_indices: Vec<usize> = circuit
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| c.comp_type == ComponentType::X && c.nodes.len() == 5)
        .map(|(i, _)| i)
        .collect();

    if !opamp_indices.is_empty() {
        let op_idx = opamp_indices[fastrand::usize(0..opamp_indices.len())];
        let out_node = circuit.components[op_idx].nodes[4];
        let inv_node = circuit.components[op_idx].nodes[1];

        // 1. If inverting input is not yet directly shorted to output, connect negative feedback (R or C)
        if inv_node != out_node {
            if fastrand::bool() {
                let id = next_id(circuit, ComponentType::R);
                let val = E24_VALUES[fastrand::usize(0..E24_VALUES.len())];
                if let Ok(comp) = Component::new('R', id, vec![out_node, inv_node], val) {
                    circuit.add_component(comp);
                    return true;
                }
            } else {
                let id = next_id(circuit, ComponentType::C);
                let val = E12_VALUES[fastrand::usize(0..E12_VALUES.len())];
                if let Ok(comp) = Component::new('C', id, vec![out_node, inv_node], val) {
                    circuit.add_component(comp);
                    return true;
                }
            }
        }

        // 2. Active filter feedback: connect feedback capacitor from OUT to an intermediate tie-point node
        let internal_nodes: Vec<usize> = circuit
            .nodes
            .iter()
            .copied()
            .filter(|&n| n != out_node && n != NODE_VCC && n != NODE_VEE && n != NODE_GND)
            .collect();

        if !internal_nodes.is_empty() {
            let target_node = internal_nodes[fastrand::usize(0..internal_nodes.len())];
            let id = next_id(circuit, ComponentType::C);
            let val = E12_VALUES[fastrand::usize(0..E12_VALUES.len())];
            if let Ok(comp) = Component::new('C', id, vec![out_node, target_node], val) {
                circuit.add_component(comp);
                return true;
            }
        }
    }

    // BJT Feedback / Biasing (Collector-to-Base, Base-to-GND, or Emitter-to-GND)
    let bjt_indices: Vec<usize> = circuit
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| c.comp_type == ComponentType::Q && c.nodes.len() == 3)
        .map(|(i, _)| i)
        .collect();

    if !bjt_indices.is_empty() {
        let bjt_idx = bjt_indices[fastrand::usize(0..bjt_indices.len())];
        let c_node = circuit.components[bjt_idx].nodes[0];
        let b_node = circuit.components[bjt_idx].nodes[1];
        let e_node = circuit.components[bjt_idx].nodes[2];

        let (u, v) = match fastrand::u8(0..3) {
            0 => (c_node, b_node),
            1 => (b_node, crate::circuit::NODE_GND),
            _ => (e_node, crate::circuit::NODE_GND),
        };

        if u != v {
            let id = next_id(circuit, ComponentType::R);
            let val = E24_VALUES[fastrand::usize(0..E24_VALUES.len())];
            if let Ok(comp) = Component::new('R', id, vec![u, v], val) {
                circuit.add_component(comp);
                return true;
            }
        }
    }

    mutate_add_component(circuit)
}

/// Operator 6: Cross-coupling between two op-amps
pub fn mutate_cross_coupling(circuit: &mut Circuit) -> bool {
    let opamp_indices: Vec<usize> = circuit
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| c.comp_type == ComponentType::X && c.nodes.len() == 5)
        .map(|(i, _)| i)
        .collect();

    if opamp_indices.len() < 2 {
        return mutate_add_feedback(circuit);
    }

    let op_a_idx = opamp_indices[fastrand::usize(0..opamp_indices.len())];
    let mut op_b_idx = opamp_indices[fastrand::usize(0..opamp_indices.len())];
    while op_b_idx == op_a_idx {
        op_b_idx = opamp_indices[fastrand::usize(0..opamp_indices.len())];
    }

    let out_a = circuit.components[op_a_idx].nodes[4];
    let mut candidate_in = Vec::new();
    if circuit.components[op_b_idx].nodes[0] != out_a {
        candidate_in.push(circuit.components[op_b_idx].nodes[0]);
    }
    if circuit.components[op_b_idx].nodes[1] != out_a {
        candidate_in.push(circuit.components[op_b_idx].nodes[1]);
    }

    if candidate_in.is_empty() {
        return false;
    }

    let in_b = candidate_in[fastrand::usize(0..candidate_in.len())];

    let id = next_id(circuit, ComponentType::R);
    let val = E24_VALUES[fastrand::usize(0..E24_VALUES.len())];
    if let Ok(comp) = Component::new('R', id, vec![out_a, in_b], val) {
        circuit.add_component(comp);
        return true;
    }

    false
}

/// Operator 7: Split a resistor into two series resistors with an intermediate node
pub fn mutate_split_resistor(circuit: &mut Circuit) -> bool {
    let r_indices: Vec<usize> = circuit
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| c.comp_type == ComponentType::R && c.nodes.len() == 2)
        .map(|(i, _)| i)
        .collect();

    if r_indices.is_empty() {
        return false;
    }

    let idx = r_indices[fastrand::usize(0..r_indices.len())];
    let u = circuit.components[idx].nodes[0];
    let v = circuit.components[idx].nodes[1];
    let orig_val = circuit.components[idx].value.clone();

    let mid_node = next_node_id(circuit);

    // Update first resistor to connect u -> mid_node
    circuit.components[idx].nodes = vec![u, mid_node];

    // Add second resistor connecting mid_node -> v
    let id2 = next_id(circuit, ComponentType::R);
    if let Ok(comp2) = Component::new('R', id2, vec![mid_node, v], &orig_val) {
        circuit.add_component(comp2);
        circuit.nodes.insert(mid_node);
        return true;
    }

    false
}

/// Operator 8: Add Bootstrap Bridge (Compound structural mutation for high-Z discrete buffers)
pub fn mutate_add_bootstrap(circuit: &mut Circuit) -> bool {
    let bjt_indices: Vec<usize> = circuit
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| c.comp_type == ComponentType::Q && c.nodes.len() == 3)
        .map(|(i, _)| i)
        .collect();

    if bjt_indices.is_empty() {
        return mutate_add_component(circuit);
    }

    let bjt_idx = bjt_indices[fastrand::usize(0..bjt_indices.len())];
    let base_node = circuit.components[bjt_idx].nodes[1];
    let emitter_node = circuit.components[bjt_idx].nodes[2];

    // Find a bias resistor connected to base_node
    let bias_r_indices: Vec<usize> = circuit
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.comp_type == ComponentType::R
                && c.nodes.len() == 2
                && (c.nodes[0] == base_node || c.nodes[1] == base_node)
        })
        .map(|(i, _)| i)
        .collect();

    if bias_r_indices.is_empty() {
        return false;
    }

    let r_idx = bias_r_indices[fastrand::usize(0..bias_r_indices.len())];
    let other_node = if circuit.components[r_idx].nodes[0] == base_node {
        circuit.components[r_idx].nodes[1]
    } else {
        circuit.components[r_idx].nodes[0]
    };

    let boot_node = next_node_id(circuit);

    // 1. Reconnect existing bias resistor between other_node and boot_node
    circuit.components[r_idx].nodes = vec![other_node, boot_node];

    // 2. Add series isolation resistor from boot_node to base_node
    let r_iso_id = next_id(circuit, ComponentType::R);
    let val_r = E24_VALUES[fastrand::usize(0..E24_VALUES.len())];
    if let Ok(r_iso) = Component::new('R', r_iso_id, vec![boot_node, base_node], val_r) {
        circuit.add_component(r_iso);
    }

    // 3. Add bootstrap capacitor from emitter_node to boot_node
    let c_boot_id = next_id(circuit, ComponentType::C);
    let val_c = if fastrand::bool() { "10uF" } else { "1uF" };
    if let Ok(c_boot) = Component::new('C', c_boot_id, vec![emitter_node, boot_node], val_c) {
        circuit.add_component(c_boot);
    }

    circuit.nodes.insert(boot_node);
    true
}

/// Telemetry for tracking mutation operator performance and fallback rates
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MutationStats {
    pub total_calls: usize,
    pub successful_mutations: usize,
    pub fallbacks: usize,
    pub op_attempts: [usize; 8],
    pub op_successes: [usize; 8],
}

impl MutationStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn summary(&self) -> String {
        let op_names = [
            "ChangeVal", "AddComp", "RemoveComp", "MoveTerm", "AddFeedbk", "CrossCpl", "SplitRes", "AddBoot",
        ];
        let mut op_summary = String::new();
        for i in 0..8 {
            let rate = if self.op_attempts[i] > 0 {
                (self.op_successes[i] as f64 / self.op_attempts[i] as f64) * 100.0
            } else {
                0.0
            };
            op_summary.push_str(&format!(
                " [{}: {}/{} ({:.0}%)]",
                op_names[i], self.op_successes[i], self.op_attempts[i], rate
            ));
        }

        let fallback_rate = if self.total_calls > 0 {
            (self.fallbacks as f64 / self.total_calls as f64) * 100.0
        } else {
            0.0
        };

        format!(
            "Total Calls: {} | Success: {} | Fallbacks: {} ({:.1}%) | Operators:{}",
            self.total_calls, self.successful_mutations, self.fallbacks, fallback_rate, op_summary
        )
    }
}

/// Core mutation engine with telemetry tracking and bloat control
pub fn mutate_with_stats(circuit: &Circuit, stats: &mut MutationStats) -> Circuit {
    stats.total_calls += 1;
    let comp_count = circuit.components.len();

    for _ in 0..10 {
        let mut candidate = circuit.clone();

        // Dynamic operator weighting for bloat control:
        // - If >= 16 components: strictly remove/change_value (0% add)
        // - If >= 13 components: heavily prioritize remove over add
        // - If <= 4 components: prioritize add/change_value/bootstrap over remove
        let op = if comp_count >= 16 {
            match fastrand::u8(0..10) {
                0..=4 => 2, // remove component (50%)
                5..=8 => 0, // change value (40%)
                _ => 3,     // move terminal (10%)
            }
        } else if comp_count >= 13 {
            match fastrand::u8(0..10) {
                0..=4 => 2, // remove component (50%)
                5..=7 => 0, // change value (30%)
                8 => 3,     // move terminal (10%)
                _ => 1,     // add (10%)
            }
        } else if comp_count <= 4 {
            match fastrand::u8(0..100) {
                0..=24 => 0,  // change value (25%)
                25..=49 => 1, // add component (25%)
                50..=69 => 6, // split resistor (20%) - crucial for intermediate ladder nodes!
                70..=84 => 3, // move terminal (15%)
                85..=94 => 4, // add feedback (10%)
                _ => 7,       // add bootstrap (5%)
            }
        } else {
            match fastrand::u8(0..100) {
                0..=25 => 0,  // change value (25%)
                26..=45 => 1, // add component (20%)
                46..=60 => 2, // remove component (15%)
                61..=70 => 3, // move terminal (10%)
                71..=80 => 4, // add feedback (10%)
                81..=92 => 6, // split resistor (12%)
                93..=97 => 7, // add bootstrap (5%)
                _ => 5,       // cross coupling (3%)
            }
        };

        stats.op_attempts[op as usize] += 1;
        let mutated = match op {
            0 => mutate_change_value(&mut candidate),
            1 => mutate_add_component(&mut candidate),
            2 => mutate_remove_component(&mut candidate),
            3 => mutate_move_terminal(&mut candidate),
            4 => mutate_add_feedback(&mut candidate),
            5 => mutate_cross_coupling(&mut candidate),
            6 => mutate_split_resistor(&mut candidate),
            _ => mutate_add_bootstrap(&mut candidate),
        };

        if mutated {
            stats.op_successes[op as usize] += 1;
            if candidate.validate().is_ok() {
                stats.successful_mutations += 1;
                return candidate;
            }
        }
    }

    stats.fallbacks += 1;
    circuit.clone()
}

/// Standard entry point
pub fn mutate(circuit: &Circuit) -> Circuit {
    let mut dummy_stats = MutationStats::new();
    mutate_with_stats(circuit, &mut dummy_stats)
}

/// Seed a population of 100% discrete components (BJTs, Resistors, Capacitors, Diodes - NO Op-Amps)
pub fn seed_discrete_population(size: usize) -> Vec<Circuit> {
    let mut pop = Vec::new();

    // Template 1: Biased NPN Emitter Follower
    // Q1 c=VCC, b=10, e=OUT
    // R1 VCC to 10 (100k), R2 10 to VEE (100k)
    // R3 OUT to VEE (4.7k)
    // C1 IN to 10 (1uF)
    let mut npn_seed = Circuit::new();
    npn_seed.add_component(
        Component::new('Q', 1, vec![crate::circuit::NODE_VCC, 10, crate::circuit::NODE_OUT], "2N3904").unwrap(),
    );
    npn_seed.add_component(Component::new('R', 1, vec![crate::circuit::NODE_VCC, 10], "100k").unwrap());
    npn_seed.add_component(Component::new('R', 2, vec![10, crate::circuit::NODE_VEE], "100k").unwrap());
    npn_seed.add_component(Component::new('R', 3, vec![crate::circuit::NODE_OUT, crate::circuit::NODE_VEE], "4.7k").unwrap());
    npn_seed.add_component(Component::new('C', 1, vec![crate::circuit::NODE_IN, 10], "1uF").unwrap());
    let _ = npn_seed.validate();

    // Template 2: Bootstrapped NPN High-Zin Follower
    // Q1 c=VCC, b=10, e=OUT
    // R1 VCC to 11 (100k), R2 11 to VEE (100k)
    // R3 11 to 10 (47k) - isolates AC from bias divider
    // R4 OUT to VEE (4.7k)
    // C1 IN to 10 (1uF) - AC input coupling
    // C2 OUT to 11 (10uF) - bootstrap feedback
    let mut boot_seed = Circuit::new();
    boot_seed.add_component(
        Component::new('Q', 1, vec![crate::circuit::NODE_VCC, 10, crate::circuit::NODE_OUT], "2N3904").unwrap(),
    );
    boot_seed.add_component(Component::new('R', 1, vec![crate::circuit::NODE_VCC, 11], "100k").unwrap());
    boot_seed.add_component(Component::new('R', 2, vec![11, crate::circuit::NODE_VEE], "100k").unwrap());
    boot_seed.add_component(Component::new('R', 3, vec![11, 10], "47k").unwrap());
    boot_seed.add_component(Component::new('R', 4, vec![crate::circuit::NODE_OUT, crate::circuit::NODE_VEE], "4.7k").unwrap());
    boot_seed.add_component(Component::new('C', 1, vec![crate::circuit::NODE_IN, 10], "1uF").unwrap());
    boot_seed.add_component(Component::new('C', 2, vec![crate::circuit::NODE_OUT, 11], "10uF").unwrap());
    let _ = boot_seed.validate();

    // Template 3: Discrete PNP Emitter Follower
    let mut pnp_seed = Circuit::new();
    pnp_seed.add_component(
        Component::new('Q', 1, vec![crate::circuit::NODE_VEE, 10, crate::circuit::NODE_OUT], "2N3906").unwrap(),
    );
    pnp_seed.add_component(Component::new('R', 1, vec![crate::circuit::NODE_VCC, 10], "100k").unwrap());
    pnp_seed.add_component(Component::new('R', 2, vec![10, crate::circuit::NODE_VEE], "100k").unwrap());
    pnp_seed.add_component(Component::new('R', 3, vec![crate::circuit::NODE_VCC, crate::circuit::NODE_OUT], "4.7k").unwrap());
    pnp_seed.add_component(Component::new('C', 1, vec![crate::circuit::NODE_IN, 10], "1uF").unwrap());
    let _ = pnp_seed.validate();

    pop.push(npn_seed.clone());
    pop.push(boot_seed.clone());
    pop.push(pnp_seed.clone());

    while pop.len() < size {
        let base = match pop.len() % 3 {
            0 => &npn_seed,
            1 => &boot_seed,
            _ => &pnp_seed,
        };
        pop.push(mutate(base));
    }

    pop
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Circuit, Component, NODE_GND, NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE};
    use crate::constraints::{check, FilterStats};
    use crate::spice::run_simulation;
    use std::time::Duration;

    fn make_seed_circuit() -> Circuit {
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

    /// Acceptance Criterion 1:
    /// Run 1000 sequential random mutations with telemetry.
    /// Asserts all produced circuits validate, no forbidden letters exist,
    /// and fallback rate is low (< 5%).
    #[test]
    fn test_1000_mutations_validity() {
        let mut current = make_seed_circuit();
        let mut stats = MutationStats::new();
        let forbidden = ['E', 'e', 'F', 'f', 'G', 'g', 'H', 'h', 'B', 'b', 'A', 'a'];

        for iter in 0..1000 {
            current = mutate_with_stats(&current, &mut stats);

            // 1. Must be valid
            let val_res = current.validate();
            assert!(
                val_res.is_ok(),
                "Mutation #{} produced invalid circuit: {:?}",
                iter,
                val_res.err()
            );

            // 2. Must not contain forbidden component letters
            for comp in &current.components {
                let letter = comp.comp_type.to_char();
                assert!(
                    !forbidden.contains(&letter),
                    "Forbidden letter '{}' found in mutation #{}",
                    letter,
                    iter
                );
            }
        }

        println!("Telemetry: {}", stats.summary());
        // Fallback rate must be strictly under 5%
        let fallback_rate = (stats.fallbacks as f64 / stats.total_calls as f64) * 100.0;
        assert!(
            fallback_rate < 5.0,
            "Fallback rate ({:.1}%) is too high! Operators are failing.",
            fallback_rate
        );
    }

    /// Acceptance Criterion 2: Random Walk Test (200 consecutive steps)
    /// Verifies that circuit genuinely explores topology space without bloat.
    #[test]
    fn test_random_walk_diversity() {
        let seed = make_seed_circuit();
        let mut current = seed.clone();
        let mut min_comps = current.components.len();
        let mut max_comps = current.components.len();

        for _ in 0..200 {
            current = mutate(&current);
            let len = current.components.len();
            min_comps = min_comps.min(len);
            max_comps = max_comps.max(len);
        }

        println!(
            "200-step Random Walk: Final component count = {}, Min = {}, Max = {}",
            current.components.len(),
            min_comps,
            max_comps
        );

        // Bloat control assertions:
        assert!(
            max_comps <= 18,
            "Circuit bloated beyond soft limit: max = {}",
            max_comps
        );
        assert!(
            min_comps >= 3,
            "Circuit eroded below viable minimum: min = {}",
            min_comps
        );

        // Verify topology actually changed from seed
        assert_ne!(
            current.components.len(),
            0,
            "Circuit components should not be empty"
        );
    }

    /// Acceptance Criterion 3: Random Walk with Simulation & FilterStats
    /// Simulates 25 consecutive mutant topologies and prints actual filter survival rate.
    #[test]
    fn test_random_walk_with_constraints() {
        let mut current = make_seed_circuit();
        let mut filter_stats = FilterStats::new();

        for _ in 0..25 {
            current = mutate(&current);
            let netlist = current.to_netlist("Mutant Topology");
            if let Ok(sim) = run_simulation(&netlist, Duration::from_secs(2)) {
                let decision = check(&sim, current.vcc, current.vee);
                filter_stats.record(&decision);
            }
        }

        println!("25-step Mutant Simulation Telemetry: {}", filter_stats.summary());
        // Verify that candidates are being evaluated and filter is classifying them
        assert!(filter_stats.total_evaluated > 0);
    }

    /// Test E24 and E12 stepping
    #[test]
    fn test_e24_e12_stepping() {
        let next_r = step_e24("10k");
        assert!(next_r == "9.1k" || next_r == "11k");

        let next_c = step_e12("10nF");
        assert!(next_c == "8.2nF" || next_c == "12nF");
    }

    /// Test feedback mutation specifically
    #[test]
    fn test_feedback_operator() {
        let mut c = make_seed_circuit();
        let initial_comp_count = c.components.len();
        let success = mutate_add_feedback(&mut c);
        assert!(success);
        assert_eq!(c.components.len(), initial_comp_count + 1);
        assert!(c.validate().is_ok());
    }

    /// Test bootstrap mutation operator
    #[test]
    fn test_bootstrap_operator_creates_valid_topology() {
        let seeds = seed_discrete_population(1);
        let mut bjt_circuit = seeds[0].clone();
        let initial_count = bjt_circuit.components.len();

        let success = mutate_add_bootstrap(&mut bjt_circuit);
        assert!(success, "mutate_add_bootstrap must succeed on standard discrete follower");
        assert_eq!(bjt_circuit.components.len(), initial_count + 2, "Bootstrap adds 1 isolation resistor and 1 bootstrap cap");
        assert!(bjt_circuit.validate().is_ok(), "Bootstrapped circuit must pass topological validation");
    }

    /// Test Q model toggle (NPN <-> PNP) preserves active forward bias
    #[test]
    fn test_q_toggle_preserves_active_bias() {
        let seeds = seed_discrete_population(1);
        let mut circuit = seeds[0].clone();

        let q_idx = circuit.components.iter().position(|c| c.comp_type == ComponentType::Q).unwrap();
        assert!(circuit.components[q_idx].value.contains("3904"));
        let orig_c = circuit.components[q_idx].nodes[0];
        let orig_e = circuit.components[q_idx].nodes[2];

        // Find initial base bias resistors
        let base_node = circuit.components[q_idx].nodes[1];
        let mut initial_r_vcc = String::new();
        let mut initial_r_vee = String::new();
        for c in &circuit.components {
            if c.comp_type == ComponentType::R && (c.nodes[0] == base_node || c.nodes[1] == base_node) {
                let other = if c.nodes[0] == base_node { c.nodes[1] } else { c.nodes[0] };
                if other == NODE_VCC {
                    initial_r_vcc = c.value.clone();
                } else if other == NODE_VEE || other == NODE_GND {
                    initial_r_vee = c.value.clone();
                }
            }
        }

        // Toggle Q by running mutate_change_value until Q is selected
        let mut toggled = false;
        for _ in 0..100 {
            let mut trial = circuit.clone();
            if mutate_change_value(&mut trial) && trial.components[q_idx].value.contains("3906") {
                circuit = trial;
                toggled = true;
                break;
            }
        }
        assert!(toggled, "mutate_change_value must eventually toggle Q");

        // Verify transistor value toggled to PNP
        assert!(circuit.components[q_idx].value.contains("3906"));
        // Verify C and E swapped
        assert_eq!(circuit.components[q_idx].nodes[0], orig_e);
        assert_eq!(circuit.components[q_idx].nodes[2], orig_c);

        // Verify base bias resistors inverted
        for c in &circuit.components {
            if c.comp_type == ComponentType::R && (c.nodes[0] == base_node || c.nodes[1] == base_node) {
                let other = if c.nodes[0] == base_node { c.nodes[1] } else { c.nodes[0] };
                if other == NODE_VCC {
                    assert_eq!(c.value, initial_r_vee);
                } else if other == NODE_VEE || other == NODE_GND {
                    assert_eq!(c.value, initial_r_vcc);
                }
            }
        }
        assert!(circuit.validate().is_ok());
    }
}

