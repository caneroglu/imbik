use crate::circuit::{Circuit, ComponentType, NODE_GND, NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE};
use crate::engine::CheckpointData;
use crate::fitness::{euclidean_distance, BehaviorDescriptor};
use crate::spice::NoiseSummary;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rarity {
    Common,    // Nearest-neighbor distance < 0.30
    Rare,      // Nearest-neighbor distance >= 0.30
    Epic,      // Nearest-neighbor distance >= 0.60 AND Monte Carlo deviation < 6.0 dB
    Legendary, // Physically verified on breadboard via oscilloscope ingest
}

impl Rarity {
    pub fn label(&self) -> &'static str {
        match self {
            Rarity::Common => "COMMON",
            Rarity::Rare => "RARE",
            Rarity::Epic => "EPIC",
            Rarity::Legendary => "LEGENDARY",
        }
    }

    pub fn colored_label(&self) -> String {
        match self {
            Rarity::Common => format!("\x1b[90m[{:<4}]\x1b[0m", self.label()),
            Rarity::Rare => format!("\x1b[1;36m[{:<4}]\x1b[0m", self.label()),
            Rarity::Epic => format!("\x1b[1;35m[{:<4}]\x1b[0m", self.label()),
            Rarity::Legendary => format!("\x1b[1;33m[{:<9}]\x1b[0m", self.label()),
        }
    }
}

/// Render 11D Behavior Descriptor into a compact 11-character UTF-8 sparkline bar
pub fn render_sparkline(d: &BehaviorDescriptor) -> String {
    let blocks = [' ', ' ', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let mut s = String::with_capacity(11);
    for &val in d {
        let clamped = val.clamp(0.0, 1.0);
        let idx = (clamped * 8.0).round() as usize;
        s.push(blocks[idx.min(8)]);
    }
    s
}

/// Derive a human-readable, colorful character tag from the 11D descriptor and optional noise profile
pub fn describe_character(d: &BehaviorDescriptor) -> String {
    describe_character_with_noise(d, None)
}

/// Derive a human-readable character tag taking both 11D dynamics and analog noise into account
pub fn describe_character_with_noise(d: &BehaviorDescriptor, noise: Option<&NoiseSummary>) -> String {
    if let Some(ns) = noise {
        let en_nv = ns.inoise_spot_1k;
        if en_nv > 0.0 && en_nv <= 3.5 && d[9] <= 0.02 {
            return format!("🤫 Ultra Düşük Gürültü ({:.1} nV/√Hz)", en_nv);
        }
    }

    if d[9] > 0.02 {
        let f_khz = 10f64.powf(d[10] * 5.0) / 1000.0;
        if f_khz > 20.0 {
            format!("⚠️ Parazitik Çınlama ({:.1} kHz)", f_khz)
        } else {
            format!("⚡ Otonom Osilatör ({:.1} kHz)", f_khz)
        }
    } else if d[7] > 0.08 {
        "🔥 Sert Kırpma / Fuzz (H5 Baskın)".to_string()
    } else if d[5] > 0.04 && d[5] > d[6] * 1.25 {
        "🎸 Asimetrik / Çift Harmonik (Warm Tube)".to_string()
    } else if d[6] > 0.04 && d[6] > d[5] * 1.25 {
        "⚡ Simetrik Kırpma (Overdrive)".to_string()
    } else if d[8] > 0.20 {
        format!("🗜️ Dinamik Kompresör ({:.0}%)", d[8] * 100.0)
    } else if d[0] > 0.5 && d[3] > 0.25 {
        format!("🔊 Rezonanslı Filtre (Peak Q +{:.1}dB)", d[3] * 20.0)
    } else if d[0] > 0.5 {
        let fc = 10f64.powf(d[1] * 5.0);
        format!("🎛️ Aktif Filtre (fc ≈ {:.0} Hz)", fc)
    } else if let Some(ns) = noise {
        let en_nv = ns.inoise_spot_1k;
        if en_nv > 0.0 && en_nv <= 8.0 {
            format!("🎧 Düşük Gürültülü Kat ({:.1} nV/√Hz)", en_nv)
        } else if d[0] < 0.5 && d[5] < 0.005 && d[4] < 0.01 {
            "〰️ Geniş Bant Lineer Kat".to_string()
        } else {
            "🎚️ Analog Dalga Şekillendirici".to_string()
        }
    } else if d[0] < 0.5 && d[5] < 0.005 && d[4] < 0.01 {
        "〰️ Geniş Bant Lineer Kat".to_string()
    } else {
        "🎚️ Analog Dalga Şekillendirici".to_string()
    }
}

/// Summarize circuit components in compact string (e.g. "4R 2C 1D 1X (8)")
pub fn summarize_components(c: &Circuit) -> String {
    let mut r = 0;
    let mut cap = 0;
    let mut d = 0;
    let mut q = 0;
    let mut x = 0;
    for comp in &c.components {
        match comp.comp_type {
            ComponentType::R => r += 1,
            ComponentType::C => cap += 1,
            ComponentType::D => d += 1,
            ComponentType::Q => q += 1,
            ComponentType::X => x += 1,
            _ => {}
        }
    }
    format!("{}R {}C {}D {}Q {}X ({:>2})", r, cap, d, q, x, c.components.len())
}

/// Determine circuit rarity from its nearest-neighbor distance, Monte Carlo environmental deviation,
/// and behavioral functionality.
pub fn evaluate_rarity(nn_dist: f64, mc_dev_db: Option<f64>, desc: &BehaviorDescriptor) -> Rarity {
    // 1. Parasitic Instability Check:
    let has_osc = desc[9] > 0.02;
    let f_osc_hz = if has_osc {
        10f64.powf(desc[10] * 5.0)
    } else {
        0.0
    };
    let is_ultrasonic_parasite = has_osc && f_osc_hz > 20_000.0;

    // 2. Functionality Checks:
    let has_filter = desc[0] > 0.5;
    let fc_hz = if has_filter { 10f64.powf(desc[1] * 5.0) } else { 0.0 };
    let audio_filter = has_filter && ((20.0..=20_000.0).contains(&fc_hz) || desc[3] > 0.20);

    let has_harmonics = (desc[5] > 0.03) || (desc[6] > 0.03) || (desc[7] > 0.03) || (desc[8] > 0.15);
    let audio_osc = has_osc && (20.0..=20_000.0).contains(&f_osc_hz);

    let is_functional = (audio_filter || has_harmonics || audio_osc) && !is_ultrasonic_parasite;

    // 3. Environmental Robustness (Monte Carlo component variation & breadboard stray tolerance):
    let mc_val = mc_dev_db.unwrap_or(99.0);

    // 4. LEGENDARY: Literature-grade analog breakthrough!
    let is_rich_char = desc[5] > 0.05 || desc[8] > 0.20 || desc[3] > 0.25 || desc[7] > 0.08;
    if is_functional && nn_dist >= 0.70 && mc_val <= 1.5 && is_rich_char {
        return Rarity::Legendary;
    }

    // 5. EPIC: Truly distinctive, environmentally robust, and strictly functional
    if is_functional && nn_dist >= 0.50 && mc_val <= 2.5 {
        return Rarity::Epic;
    }

    // 6. RARE: Noticeable novelty and acceptable stability
    if is_functional && nn_dist >= 0.35 && mc_val <= 4.0 {
        return Rarity::Rare;
    }

    // 7. COMMON: Classic topology clone, high tolerance spread, or weak character
    Rarity::Common
}

/// Resolve the uv executable path from environment or default PATH
pub fn get_uv_cmd() -> String {
    if let Ok(p) = std::env::var("UV_PATH")
        && !p.trim().is_empty() {
            return p.trim().to_string();
        }
    if let Ok(p) = std::env::var("IMBIK_UV")
        && !p.trim().is_empty() {
            return p.trim().to_string();
        }
    "uv".to_string()
}

/// Evaluate circuit rarity with hardware verification awareness
pub fn evaluate_rarity_with_hw(
    nn_dist: f64,
    mc_dev_db: Option<f64>,
    descriptor: &BehaviorDescriptor,
    is_hardware_verified: bool,
) -> Rarity {
    if is_hardware_verified {
        return Rarity::Legendary;
    }
    evaluate_rarity(nn_dist, mc_dev_db, descriptor)
}

#[derive(Clone)]
pub struct ArchiveItemView {
    pub id: usize,
    pub circuit: Circuit,
    pub descriptor: BehaviorDescriptor,
    pub nn_dist: f64,
    pub mc_dev_db: Option<f64>,
    pub noise_summary: Option<NoiseSummary>,
    pub rarity: Rarity,
    pub generation: usize,
    pub fitness: Option<f64>,
    pub obj_summary: Option<String>,
    pub is_hardware_verified: bool,
}

/// Load and analyze all entries in a checkpoint file without leaking memory
pub fn load_archive_views(checkpoint_path: &Path) -> Result<Vec<ArchiveItemView>, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(checkpoint_path)?;
    let checkpoint: CheckpointData = serde_json::from_str(&content)?;

    let n = checkpoint.archive.entries.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    // Pre-calculate nearest neighbor distance for every archive entry
    let mut nn_distances = Vec::with_capacity(n);
    for i in 0..n {
        let mut min_dist = f64::INFINITY;
        for j in 0..n {
            if i != j {
                let dist = euclidean_distance(
                    &checkpoint.archive.entries[i].descriptor,
                    &checkpoint.archive.entries[j].descriptor,
                );
                if dist < min_dist {
                    min_dist = dist;
                }
            }
        }
        if min_dist.is_infinite() {
            min_dist = 1.0;
        }
        nn_distances.push(min_dist);
    }

    let mut views = Vec::with_capacity(n);
    for (i, entry) in checkpoint.archive.entries.into_iter().enumerate() {
        let nn = nn_distances[i];
        let mc = entry.mc_dev_db;
        let is_hw = entry.is_hardware_verified;
        let rarity = evaluate_rarity_with_hw(nn, mc, &entry.descriptor, is_hw);
        let generation = entry.generation;
        let fitness = entry.fitness;
        let obj_summary = entry.obj_summary;
        let noise_summary = entry.noise_summary;

        views.push(ArchiveItemView {
            id: i,
            circuit: entry.circuit,
            descriptor: entry.descriptor,
            nn_dist: nn,
            mc_dev_db: mc,
            noise_summary,
            rarity,
            generation,
            fitness,
            obj_summary,
            is_hardware_verified: is_hw,
        });
    }

    Ok(views)
}

/// Print formatted, colorful loot table for the archive
pub fn list_archive(checkpoint_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut items = load_archive_views(checkpoint_path)?;
    if items.is_empty() {
        println!("Archive in {:?} is empty.", checkpoint_path);
        return Ok(());
    }

    // Sort by nearest neighbor distance (highest novelty / rarest first)
    items.sort_by(|a, b| b.nn_dist.partial_cmp(&a.nn_dist).unwrap_or(std::cmp::Ordering::Equal));

    println!("\n========================================================================================================================================");
    println!("  🏆 İMBİK ANALOG LOOT TABLE — ARCHIVE DISCOVERIES (Total: {})", items.len());
    println!("  Source: {:?}", checkpoint_path);
    println!("========================================================================================================================================");
    println!(
        " {:<4} | {:<5} | {:<8} | {:<10} | {:<12} | {:<15} | {:<11} | {:<7} | {:<6} | {:<28}",
        "ID", "GEN", "FITNESS", "RARITY", "SPARK (11D)", "PARTS", "NOISE (1k)", "NN DIST", "MC DEV", "CHARACTER SIGNATURE"
    );
    println!("----------------------------------------------------------------------------------------------------------------------------------------");

    let mut legendary_count = 0;
    let mut epic_count = 0;
    let mut rare_count = 0;
    let mut common_count = 0;

    for item in &items {
        match item.rarity {
            Rarity::Legendary => legendary_count += 1,
            Rarity::Epic => epic_count += 1,
            Rarity::Rare => rare_count += 1,
            Rarity::Common => common_count += 1,
        }

        let spark = render_sparkline(&item.descriptor);
        let comps = summarize_components(&item.circuit);
        let char_tag = describe_character_with_noise(&item.descriptor, item.noise_summary.as_ref());
        let mc_str = match item.mc_dev_db {
            Some(dev) => format!("{:.1}dB", dev),
            None => "--".to_string(),
        };
        let fit_str = match item.fitness {
            Some(f) => format!("{:.4}", f),
            None => "--".to_string(),
        };
        let gen_str = if item.generation > 0 {
            format!("G{:>2}", item.generation)
        } else {
            "--".to_string()
        };
        let noise_str = match item.noise_summary.as_ref() {
            Some(ns) => {
                let nv = ns.inoise_spot_1k;
                if nv < 1000.0 {
                    format!("{:.1}nV/√Hz", nv)
                } else {
                    format!("{:.1}µV/√Hz", nv / 1000.0)
                }
            }
            None => "--".to_string(),
        };

        println!(
            " #{:<3} | {:<5} | {:<8} | {:<19} | \x1b[33m{:<12}\x1b[0m | {:<15} | {:<11} | {:.4}  | {:<6} | {}",
            item.id,
            gen_str,
            fit_str,
            item.rarity.colored_label(),
            spark,
            comps,
            noise_str,
            item.nn_dist,
            mc_str,
            char_tag
        );
    }

    println!("----------------------------------------------------------------------------------------------------------------------------------------");
    println!(
        " Summary: \x1b[1;33m{} LEGENDARY\x1b[0m, \x1b[1;35m{} EPIC\x1b[0m, \x1b[1;36m{} RARE\x1b[0m, \x1b[90m{} COMMON\x1b[0m.",
        legendary_count, epic_count, rare_count, common_count
    );
    println!(" Tips: Use `imbik show <id>` to inspect details, or `imbik bench <id>` to export breadboard package.");
    println!("========================================================================================================================================\n");

    Ok(())
}

/// Print comprehensive detail view of an individual archived circuit
pub fn show_circuit(checkpoint_path: &Path, circuit_id: usize) -> Result<(), Box<dyn std::error::Error>> {
    let items = load_archive_views(checkpoint_path)?;
    let item = items
        .iter()
        .find(|it| it.id == circuit_id)
        .ok_or_else(|| format!("Circuit ID #{} not found in archive (total: {})", circuit_id, items.len()))?;

    let d = &item.descriptor;

    println!("\n=================================================================================");
    if item.generation > 0 {
        println!("  🔍 CIRCUIT INSPECTOR — DISCOVERY #{} (Discovered in Generation {})", item.id, item.generation);
    } else {
        println!("  🔍 CIRCUIT INSPECTOR — DISCOVERY #{}", item.id);
    }
    println!("=================================================================================");
    if let Some(fit) = item.fitness {
        println!("- **Fitness Score**: {:.6}", fit);
    }
    if let Some(ref summary) = item.obj_summary {
        println!("- **Objective Breakdown**: {}", summary);
    }
    println!("- **Rarity Tier**: {}", item.rarity.colored_label());
    println!("- **Character Tag**: {}", describe_character_with_noise(d, item.noise_summary.as_ref()));
    println!("- **Nearest Neighbor Distance**: {:.4}", item.nn_dist);
    println!(
        "- **Worst-Case Monte Carlo Deviation**: {}",
        item.mc_dev_db
            .map(|v| format!("{:.2} dB", v))
            .unwrap_or_else(|| "N/A".to_string())
    );
    if let Some(ref ns) = item.noise_summary {
        println!("\n## 🔬 Analog Noise Performance (SPICE .NOISE)");
        println!("- **Input-Referred Density (1 kHz)**:   {:.2} nV/√Hz", ns.inoise_spot_1k);
        println!("- **Integrated Audio Input Noise**:     {:.2} µV RMS (20 Hz - 20 kHz)", ns.inoise_total_rms);
        println!("- **Integrated Audio Output Noise**:    {:.2} µV RMS (20 Hz - 20 kHz)", ns.onoise_total_rms);
        if let Some(nf) = ns.noise_figure_db {
            println!("- **Noise Figure (NF)**:                {:.2} dB", nf);
        }
        if let Some(fc) = ns.corner_freq {
            println!("- **1/f Flicker Corner**:               {:.1} Hz", fc);
        }
        if ns.inoise_spot_1k < 3.5 {
            println!("- **Noise Quality Tier**:               \x1b[1;32m★ STUDIO ULTRA-LOW NOISE\x1b[0m (Sub-Johnson 600Ω Floor)");
        } else if ns.inoise_spot_1k < 10.0 {
            println!("- **Noise Quality Tier**:               \x1b[1;36m★ LOW NOISE PRO AUDIO\x1b[0m");
        } else if ns.inoise_spot_1k < 50.0 {
            println!("- **Noise Quality Tier**:               \x1b[33mSTANDARD ANALOG STAGE\x1b[0m");
        } else {
            println!("- **Noise Quality Tier**:               \x1b[90mHIGH NOISE\x1b[0m");
        }
    }
    println!("- **Total Components**: {}", item.circuit.components.len());
    println!("- **Supply Rails**: ±{:.1}V (VCC=+{:.1}V, VEE=-{:.1}V, GND=0V)", item.circuit.vcc, item.circuit.vcc, item.circuit.vee.abs());

    println!("\n## 11-Dimensional Behavior Descriptor Vector");
    println!("  Sparkline: \x1b[33m{}\x1b[0m\n", render_sparkline(d));
    println!(" | Index | Dimension Name            | Value    | Bar      | Physical Interpretation");
    println!(" | :---: | :------------------------ | :------: | :------: | :----------------------");

    let dim_names = [
        ("D0: ac_has_filter", d[0], if d[0] > 0.5 { "Filtre Aktif (1.0)" } else { "Geniş Bant / Kırpıcı (0.0)" }),
        ("D1: ac_cutoff_norm", d[1], &format!("fc ≈ {:.1} Hz", 10f64.powf(d[1] * 5.0))),
        ("D2: ac_rolloff_norm", d[2], &format!("Eğim ≈ {:.1} dB/dec", d[2] * -40.0)),
        ("D3: ac_peak_q_norm", d[3], &format!("Rezonans Artışı +{:.1} dB", d[3] * 20.0)),
        ("D4: tran_asym_delta", d[4], &format!("Asimetri Farkı: {:.3}", d[4])),
        ("D5: tran_h2_ratio", d[5], &format!("H2 / H1: {:.4} ({:.1}%)", d[5], d[5] * 100.0)),
        ("D6: tran_h3_ratio", d[6], &format!("H3 / H1: {:.4} ({:.1}%)", d[6], d[6] * 100.0)),
        ("D7: tran_h5_ratio", d[7], &format!("H5 / H1: {:.4} ({:.1}%)", d[7], d[7] * 100.0)),
        ("D8: tran_compression", d[8], &format!("Dinamik Kazanç Düşüşü: {:.1}%", d[8] * 100.0)),
        ("D9: tran_osc_rms", d[9], if d[9] > 0.02 { "Otonom Salınım Var" } else { "Kararlı (0.0)" }),
        ("D10: tran_osc_freq", d[10], &format!("Salınım Frekansı: {:.1} kHz", 10f64.powf(d[10] * 5.0) / 1000.0)),
    ];

    let blocks = [' ', ' ', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    for (idx, (name, val, interp)) in dim_names.iter().enumerate() {
        let clamped = val.clamp(0.0, 1.0);
        let b_idx = (clamped * 8.0).round() as usize;
        let bar_str = format!("{}{}", blocks[b_idx.min(8)], " ".repeat(7));
        println!(" | {:<5} | {:<24} | {:<8.4} | {:<8} | {}", idx, name, val, bar_str, interp);
    }

    println!("\n## Component Netlist");
    for comp in &item.circuit.components {
        let prefix = comp.comp_type.to_char();
        let nodes_str = comp
            .nodes
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("  • {}{:<3} {:<8} [Nodes: {}]", prefix, comp.id, comp.value, nodes_str);
    }

    println!("\n## Breadboard Node Connection Map");
    let mut node_map: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for comp in &item.circuit.components {
        let prefix = comp.comp_type.to_char();
        match comp.comp_type {
            ComponentType::R | ComponentType::C => {
                if comp.nodes.len() >= 2 {
                    node_map.entry(comp.nodes[0]).or_default().push(format!("{}{}[1] ({})", prefix, comp.id, comp.value));
                    node_map.entry(comp.nodes[1]).or_default().push(format!("{}{}[2] ({})", prefix, comp.id, comp.value));
                }
            }
            ComponentType::D => {
                if comp.nodes.len() >= 2 {
                    node_map.entry(comp.nodes[0]).or_default().push(format!("D{}[Anode]", comp.id));
                    node_map.entry(comp.nodes[1]).or_default().push(format!("D{}[Cathode | Ring]", comp.id));
                }
            }
            ComponentType::Q => {
                if comp.nodes.len() >= 3 {
                    node_map.entry(comp.nodes[0]).or_default().push(format!("Q{}[Collector] ({})", comp.id, comp.value));
                    node_map.entry(comp.nodes[1]).or_default().push(format!("Q{}[Base] ({})", comp.id, comp.value));
                    node_map.entry(comp.nodes[2]).or_default().push(format!("Q{}[Emitter] ({})", comp.id, comp.value));
                }
            }
            ComponentType::X
                if comp.nodes.len() >= 5 => {
                    node_map.entry(comp.nodes[0]).or_default().push(format!("X{}[IN+]", comp.id));
                    node_map.entry(comp.nodes[1]).or_default().push(format!("X{}[IN-]", comp.id));
                    node_map.entry(comp.nodes[2]).or_default().push(format!("X{}[VCC Pin 8]", comp.id));
                    node_map.entry(comp.nodes[3]).or_default().push(format!("X{}[VEE Pin 4]", comp.id));
                    node_map.entry(comp.nodes[4]).or_default().push(format!("X{}[OUT Pin 1]", comp.id));
                }
            _ => {}
        }
    }

    for (node, pins) in &node_map {
        let role = match *node {
            NODE_GND => "GND (0V)",
            NODE_IN => "INPUT",
            NODE_OUT => "OUTPUT",
            NODE_VCC => "VCC (+9V)",
            NODE_VEE => "VEE (-9V)",
            _ => "Tie Point",
        };
        println!("  - Node {:>2} ({:<10}): {}", node, role, pins.join(", "));
    }

    println!("\nReady for breadboard export! Run:");
    println!("  imbik bench {} [out_dir]", item.id);
    println!("=================================================================================\n");

    Ok(())
}

/// Export breadboard package for an archived circuit
pub fn export_bench_from_checkpoint(
    checkpoint_path: &Path,
    circuit_id: usize,
    out_dir: Option<&Path>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let items = load_archive_views(checkpoint_path)?;
    let item = items
        .iter()
        .find(|it| it.id == circuit_id)
        .ok_or_else(|| format!("Circuit ID #{} not found in archive", circuit_id))?;

    let default_dir = PathBuf::from(format!("bench/circuit_{:02}", circuit_id));
    let target_dir = out_dir.unwrap_or(&default_dir);

    let report = crate::bench::generate_bench_package(
        &item.circuit,
        &item.descriptor,
        target_dir,
        Duration::from_secs(3),
    )?;

    println!("\n✅ Bench Package successfully exported to: {:?}", report.output_dir);
    println!("  - BOM:           {:?}", report.bom_path);
    println!("  - Test Protocol: {:?}", report.protocol_path);
    println!("  - Reference CSV: {:?}", report.reference_csv_path);
    println!("  - Reference AC:  {:?}", report.reference_ac_path);
    println!("  - SPICE Netlist: {:?}", report.netlist_path);

    // Automatically render AoE SchemDraw schematic via uv or python fallback
    let svg_path = target_dir.join("schematic.svg");
    if let Err(e) = render_schematic_svg(checkpoint_path, circuit_id, &svg_path) {
        log::warn!("Schematic rendering skipped: {}", e);
    } else {
        println!("  - AoE Schematic: {:?}", svg_path);
    }

    Ok(report.output_dir)
}

/// Execute schematic rendering using multi-tier fallback:
/// 1. `uv run --with schemdraw python scripts/schematic.py ...` (via UV_PATH, IMBIK_UV, or PATH)
/// 2. Direct `python scripts/schematic.py ...`
/// 3. Direct `python3 scripts/schematic.py ...`
/// 4. Direct `py -3 scripts/schematic.py ...`
pub fn render_schematic_svg(
    checkpoint_path: &Path,
    circuit_id: usize,
    target_svg: &Path,
) -> Result<(), String> {
    if let Some(parent) = target_svg.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let cp_str = checkpoint_path.to_str().unwrap_or("");
    let id_str = circuit_id.to_string();
    let out_str = target_svg.to_str().unwrap_or("");

    // 1. Try uv with ephemeral schemdraw environment
    let uv_bin = get_uv_cmd();
    log::debug!("Attempting schematic rendering via uv binary: '{}'", uv_bin);
    let uv_result = std::process::Command::new(&uv_bin)
        .args([
            "run",
            "--with",
            "schemdraw",
            "python",
            "scripts/schematic.py",
            cp_str,
            &id_str,
            out_str,
        ])
        .status();

    if let Ok(st) = uv_result {
        if st.success() && target_svg.exists() {
            log::debug!("Schematic rendering succeeded via uv");
            return Ok(());
        }
        log::debug!("uv execution finished with non-zero exit code: {:?}", st.code());
    } else if let Err(ref e) = uv_result {
        log::debug!("uv launch failed ({:?}), falling back to direct python interpreters", e.kind());
    }

    // 2. Try direct python interpreters if uv is missing or failed
    let python_candidates = ["python", "python3", "py"];
    for py_cmd in &python_candidates {
        log::debug!("Attempting schematic rendering via direct interpreter '{}'", py_cmd);
        let mut cmd = std::process::Command::new(py_cmd);
        if *py_cmd == "py" {
            cmd.arg("-3");
        }
        cmd.args(["scripts/schematic.py", cp_str, &id_str, out_str]);

        if let Ok(st) = cmd.status()
            && st.success() && target_svg.exists() {
                log::debug!("Schematic rendering succeeded via '{}'", py_cmd);
                return Ok(());
            }
    }

    Err(format!(
        "Failed to render schematic for circuit #{}. Neither uv ('{}') nor system Python with 'schemdraw' could execute 'scripts/schematic.py'.\n\
         To enable schematic rendering:\n\
           • Install uv (recommended): https://astral.sh/uv (or 'cargo install uv')\n\
           • OR install schemdraw: 'pip install schemdraw'",
        circuit_id, uv_bin
    ))
}

/// Render standalone AoE SchemDraw schematic
pub fn draw_circuit_schematic(
    checkpoint_path: &Path,
    circuit_id: usize,
    out_svg: Option<&Path>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let default_svg = PathBuf::from(format!("bench/circuit_{:02}/schematic.svg", circuit_id));
    let target_svg = out_svg.unwrap_or(&default_svg);

    render_schematic_svg(checkpoint_path, circuit_id, target_svg)
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    println!("\n✅ AoE SchemDraw schematic successfully generated: {:?}", target_svg);
    Ok(target_svg.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rarity_and_character_tagging() {
        // Asymmetric Clipper
        let d_clipper = [0.0, 0.0, 0.0, 0.0, 0.53, 0.26, 0.06, 0.02, 0.35, 0.0, 0.0];
        let tag_c = describe_character(&d_clipper);
        assert!(tag_c.contains("Asimetrik") || tag_c.contains("Kırpma"));

        // Sparkline length must be exactly 11 characters
        let spark = render_sparkline(&d_clipper);
        assert_eq!(spark.chars().count(), 11);

        // Filter
        let d_filter = [1.0, 0.6, 0.7, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let tag_f = describe_character(&d_filter);
        assert!(tag_f.contains("Filtre"));

        // Rarity evaluation
        assert_eq!(evaluate_rarity(0.20, None, &d_clipper), Rarity::Common);
        assert_eq!(evaluate_rarity(0.40, Some(3.0), &d_clipper), Rarity::Rare);
        assert_eq!(evaluate_rarity(0.55, Some(2.0), &d_clipper), Rarity::Epic);
        assert_eq!(evaluate_rarity(0.55, Some(5.0), &d_clipper), Rarity::Common); // Too fragile for Epic
        assert_eq!(evaluate_rarity(0.75, Some(1.0), &d_clipper), Rarity::Legendary);

        // Hardware verification promotes even common circuits to Legendary tier
        assert_eq!(evaluate_rarity_with_hw(0.10, None, &d_clipper, true), Rarity::Legendary);
        assert_eq!(evaluate_rarity_with_hw(0.10, None, &d_clipper, false), Rarity::Common);
    }

    #[test]
    fn test_list_and_show_on_checkpoint() {
        let cp_path = PathBuf::from("target/test_checkpoints/checkpoint_final.json");
        if !cp_path.exists() {
            return;
        }

        let views = load_archive_views(&cp_path).expect("Failed to load archive views");
        assert!(!views.is_empty());

        list_archive(&cp_path).expect("list_archive failed");
        show_circuit(&cp_path, 0).expect("show_circuit failed");
    }
}
