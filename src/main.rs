pub mod bench;
pub mod circuit;
pub mod constraints;
pub mod engine;
pub mod fitness;
pub mod loot;
pub mod mutate;
pub mod parts;
pub mod preset;
pub mod realism;
pub mod spice;

use clap::{Parser, Subcommand};
use engine::{EvolutionConfig, EvolutionEngine};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Parser, Debug)]
#[command(
    name = "imbik",
    version = "0.1.0",
    about = "Autonomous Analog Circuit Hunter Engine",
    after_help = "Examples:\n  imbik list\n  imbik part list\n  imbik part test TL072\n  imbik part add OPA1612.LIB\n  imbik show best\n  imbik draw 0 circuit.svg\n  imbik bench 0\n  imbik evolve --preset buffer 50\n  imbik ingest scope.csv 0"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Display colorized loot table of discovered circuits
    List {
        /// Optional path to checkpoint JSON file
        checkpoint: Option<PathBuf>,
    },

    /// Manage & inspect analog component catalog, vendor SPICE models, and benchmarks
    Part {
        #[command(subcommand)]
        action: PartCommands,
    },

    /// Manage & inspect mission presets and fitness probes
    Preset {
        #[command(subcommand)]
        action: PresetCommands,
    },

    /// Inspect details, netlist & pin map of a circuit
    Show {
        /// Circuit ID number, or "best" / "latest"
        #[arg(default_value = "best")]
        id: String,

        /// Optional path to checkpoint JSON file
        checkpoint: Option<PathBuf>,
    },

    /// Generate publication-quality AoE SchemDraw schematic
    Draw {
        /// Circuit ID number, or "best"
        #[arg(default_value = "best")]
        id: String,

        /// Destination SVG path (default: bench/circuit_XX/schematic.svg)
        out_svg: Option<PathBuf>,

        /// Optional path to checkpoint JSON file
        checkpoint: Option<PathBuf>,
    },

    /// Export breadboard BOM, protocol, CSV & schematic
    Bench {
        /// Circuit ID number, or "best"
        #[arg(default_value = "best")]
        id: String,

        /// Output directory (default: bench/circuit_XX)
        out_dir: Option<PathBuf>,

        /// Optional path to checkpoint JSON file
        checkpoint: Option<PathBuf>,
    },

    /// Ingest oscilloscope capture & verify physical reality
    Ingest {
        /// Path to oscilloscope CSV capture file
        scope_csv: PathBuf,

        /// Target circuit ID (default: 0)
        #[arg(default_value_t = 0)]
        circuit_id: usize,

        /// Optional path to checkpoint JSON file
        checkpoint: Option<PathBuf>,
    },

    /// Launch mission-driven or novelty discovery engine
    Evolve {
        /// Target mission preset name (e.g. 'buffer', 'gyrator') or TOML file path
        #[arg(short, long)]
        preset: Option<String>,

        /// Population size
        #[arg(long, default_value_t = 20)]
        pop: usize,

        /// RNG seed (decimal or 0xHEX format)
        #[arg(long)]
        seed: Option<String>,

        /// Maximum generations to run
        #[arg(default_value_t = 50)]
        generations: usize,
    },
}

#[derive(Subcommand, Debug)]
enum PartCommands {
    /// List all registered parts and their physical characteristics
    List {
        /// Optional kind filter: 'opamp', 'bjt', 'diode'
        #[arg(short, long)]
        kind: Option<String>,
    },
    /// Inspect details and SPICE macromodel of a component
    Show {
        /// Part name (e.g. 'TL072', 'NE5532', '2N3904')
        name: String,
    },
    /// Run automated 50ms SPICE physical characterization bench (Zin, noise, rail margin, GBW)
    Test {
        /// Part name (e.g. 'TL072', 'NE5532')
        name: String,
    },
    /// Ingest a vendor SPICE model (.lib, .sub, .mod, .cir) OR component TOML into catalog
    Add {
        /// Path to vendor SPICE or component TOML file
        file: PathBuf,
    },
    /// Print documented TOML configuration template for defining custom components
    Template {
        /// Component kind: 'opamp', 'bjt', or 'diode' (default: 'opamp')
        #[arg(short, long, default_value = "opamp")]
        kind: String,
    },
}

#[derive(Subcommand, Debug)]
enum PresetCommands {
    /// List all available mission presets
    List,
    /// Inspect details and probe specifications of a mission preset
    Show {
        /// Preset name (e.g. 'buffer', 'low_noise_preamp') or path to TOML
        name: String,
    },
    /// Validate a preset TOML structure and verify component models with live SPICE smoke tests
    Check {
        /// Preset name (e.g. 'buffer', 'sallen_key_10k') or path to TOML
        name: String,
        /// Skip live SPICE benchmark tests (schema and catalog checks only)
        #[arg(long)]
        no_spice: bool,
    },
    /// Print documented TOML configuration template for custom missions & probe targets
    Template,
}

fn find_default_checkpoint() -> PathBuf {
    let mut candidates = vec![
        PathBuf::from("checkpoints/checkpoint_final.json"),
        PathBuf::from("../checkpoints/checkpoint_final.json"),
        PathBuf::from("../../checkpoints/checkpoint_final.json"),
        PathBuf::from("target/test_checkpoints/checkpoint_final.json"),
        PathBuf::from("checkpoints/checkpoint_gen_50.json"),
        PathBuf::from("../checkpoints/checkpoint_gen_50.json"),
        PathBuf::from("target/test_checkpoints/checkpoint_gen_50.json"),
    ];

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            candidates.push(exe_dir.join("checkpoints/checkpoint_final.json"));
            candidates.push(exe_dir.join("../checkpoints/checkpoint_final.json"));
            candidates.push(exe_dir.join("../../checkpoints/checkpoint_final.json"));
        }
    }

    for c in &candidates {
        if c.exists() {
            return c.clone();
        }
    }

    PathBuf::from("checkpoints/checkpoint_final.json")
}

fn resolve_circuit_id(id_str: &str, views: &[loot::ArchiveItemView]) -> Result<usize, String> {
    if id_str == "best" || id_str == "latest" {
        views
            .iter()
            .max_by(|a, b| {
                let fit_a = a.fitness.unwrap_or(a.nn_dist);
                let fit_b = b.fitness.unwrap_or(b.nn_dist);
                fit_a
                    .partial_cmp(&fit_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|v| v.id)
            .ok_or_else(|| "Archive is empty".to_string())
    } else {
        id_str.parse::<usize>().map_err(|_| {
            format!(
                "Invalid circuit ID: '{}'. Expected integer or 'best'",
                id_str
            )
        })
    }
}

fn parse_seed_str(s: &str) -> Result<u64, String> {
    let s_trim = s.trim();
    if s_trim.starts_with("0x") || s_trim.starts_with("0X") {
        u64::from_str_radix(&s_trim[2..], 16)
            .map_err(|e| format!("Invalid hex seed '{}': {}", s_trim, e))
    } else {
        s_trim
            .parse::<u64>()
            .map_err(|e| format!("Invalid u64 seed '{}': {}", s_trim, e))
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let cli = Cli::parse();

    let is_template = matches!(
        cli.command,
        Some(Commands::Part { action: PartCommands::Template { .. } })
            | Some(Commands::Preset { action: PresetCommands::Template })
    );

    if !is_template {
        println!("============================================================");
        println!("  İmbik v0.1.0 - Autonomous Analog Circuit Hunter Engine   ");
        println!("============================================================");
    }

    let command = match cli.command {
        Some(cmd) => cmd,
        None => {
            let default_cp = find_default_checkpoint();
            if default_cp.exists() {
                println!("Found existing discovery archive at: {:?}", default_cp);
                println!("Displaying loot table:\n");
                if let Err(e) = loot::list_archive(&default_cp) {
                    eprintln!("Error reading archive: {}", e);
                }
            } else {
                use clap::CommandFactory;
                let _ = Cli::command().print_help();
                println!();
            }
            return;
        }
    };

    match command {
        Commands::List { checkpoint } => {
            let cp_path = checkpoint.unwrap_or_else(find_default_checkpoint);

            if !cp_path.exists() {
                eprintln!("Error: Checkpoint file not found: {:?}", cp_path);
                std::process::exit(1);
            }

            if let Err(e) = loot::list_archive(&cp_path) {
                eprintln!("Error displaying archive: {}", e);
                std::process::exit(1);
            }
        }

        Commands::Part { action } => {
            let mut catalog = parts::PartCatalog::load_with_overrides();

            match action {
                PartCommands::List { kind } => {
                    println!("\n========================================================================================================================");
                    println!("  🧩 İMBİK ANALOG COMPONENT CATALOG (Total: {})", catalog.parts.len());
                    println!("========================================================================================================================");
                    println!(
                        "{:<10} | {:<10} | {:<22} | {:<12} | {:<12} | {:<12} | {:<10} | {:<14}",
                        "NAME", "KIND", "INPUT STAGE", "NOISE(1k)", "ZIN(1k)", "RRIO / Vsat", "GBW (MHz)", "SUPPLY LIMITS"
                    );
                    println!("{:-<120}", "");

                    let mut all_parts: Vec<_> = catalog.parts.values().collect();
                    all_parts.sort_by_key(|p| (p.kind as usize, p.name.clone()));

                    for p in all_parts {
                        if let Some(ref k) = kind {
                            let k_lower = k.to_lowercase();
                            let match_kind = match p.kind {
                                parts::PartKind::OpAmp => k_lower.contains("op"),
                                parts::PartKind::BjtNpn | parts::PartKind::BjtPnp => {
                                    k_lower.contains("bjt") || k_lower.contains("transistor")
                                }
                                parts::PartKind::Diode => k_lower.contains("diode"),
                                _ => false,
                            };
                            if !match_kind {
                                continue;
                            }
                        }

                        let stage_str = p.specs.input_stage.map(|s| s.as_str()).unwrap_or("-");
                        let noise_str = p
                            .specs
                            .noise_spot_1k
                            .map(|n| format!("{:.1} nV/√Hz", n))
                            .unwrap_or_else(|| "-".to_string());
                        let zin_str = p
                            .specs
                            .zin_1k
                            .map(|z| {
                                if z >= 1.0e12 {
                                    format!("{:.0} TΩ", z / 1.0e12)
                                } else if z >= 1.0e9 {
                                    format!("{:.0} GΩ", z / 1.0e9)
                                } else if z >= 1.0e6 {
                                    format!("{:.1} MΩ", z / 1.0e6)
                                } else if z >= 1.0e3 {
                                    format!("{:.0} kΩ", z / 1.0e3)
                                } else {
                                    format!("{:.0} Ω", z)
                                }
                            })
                            .unwrap_or_else(|| "-".to_string());

                        let rrio_str = if p.specs.is_rrio == Some(true) {
                            "★ RRIO".to_string()
                        } else if let Some(margin) = p.specs.rail_margin_v {
                            format!("{:.2} V drop", margin)
                        } else {
                            "-".to_string()
                        };

                        let gbw_str = p
                            .specs
                            .gbw_mhz
                            .map(|g| format!("{:.1} MHz", g))
                            .unwrap_or_else(|| "-".to_string());
                        let supply_str = match (p.specs.v_supply_min, p.specs.v_supply_max) {
                            (Some(min), Some(max)) => format!("{:.0}V - {:.0}V", min, max),
                            (None, Some(max)) => format!("max {:.0}V", max),
                            _ => "-".to_string(),
                        };

                        println!(
                            "{:<10} | {:<10} | {:<22} | {:<12} | {:<12} | {:<12} | {:<10} | {:<14}",
                            p.name,
                            p.kind.as_str(),
                            stage_str,
                            noise_str,
                            zin_str,
                            rrio_str,
                            gbw_str,
                            supply_str
                        );
                    }
                    println!("{:-<120}", "");
                    println!("Tips: Use `imbik part show <name>` to inspect SPICE model or `imbik part test <name>` to benchmark.\n");
                }

                PartCommands::Show { name } => {
                    let part = match catalog.get(&name) {
                        Some(p) => p,
                        None => {
                            eprintln!("Error: Component '{}' not found in catalog.", name);
                            std::process::exit(1);
                        }
                    };

                    println!("\n=================================================================================");
                    println!("  🔍 COMPONENT INSPECTOR: {}", part.name);
                    println!("=================================================================================");
                    println!("- **Name**:          {}", part.name);
                    println!("- **Kind**:          {}", part.kind.as_str());
                    println!("- **Description**:   {}", part.description);
                    println!(
                        "- **Origin**:        {}",
                        if part.is_builtin {
                            "Built-in System Library"
                        } else {
                            "Custom / User Imported"
                        }
                    );
                    println!("- **Pin Mapping**:   {:?}", part.pin_order);
                    if let Some(st) = part.specs.input_stage {
                        println!("- **Input Stage**:   {}", st.as_str());
                    }
                    if let Some(n) = part.specs.noise_spot_1k {
                        println!("- **Voltage Noise**: {:.2} nV/√Hz @ 1kHz", n);
                    }
                    if let Some(fc) = part.specs.noise_corner_freq {
                        println!("- **1/f Corner**:    {:.1} Hz", fc);
                    }
                    if let Some(z) = part.specs.zin_1k {
                        println!("- **Input Impedance**: {:.0} Ω", z);
                    }
                    if let Some(g) = part.specs.gbw_mhz {
                        println!("- **Unity GBW**:     {:.2} MHz", g);
                    }
                    if let Some(r) = part.specs.rail_margin_v {
                        println!(
                            "- **Rail Margin**:   {:.2} V (RRIO: {:?})",
                            r,
                            part.specs.is_rrio.unwrap_or(false)
                        );
                    }

                    println!("\n## SPICE Model Definition");
                    println!("```spice\n{}\n```", part.spice_text.trim());
                    println!("=================================================================================\n");
                }

                PartCommands::Test { name } => {
                    println!("\n=================================================================================");
                    println!("  🔬 LIVE SPICE COMPONENT BENCHMARK: {}", name);
                    println!("=================================================================================");
                    println!("Running automated 50ms SPICE physical characterization bench on ngspice...");

                    let part = match catalog.get(&name) {
                        Some(p) => p,
                        None => {
                            eprintln!("Error: Component '{}' not found in catalog.", name);
                            std::process::exit(1);
                        }
                    };

                    if part.kind == parts::PartKind::OpAmp {
                        match catalog.benchmark_opamp(&name) {
                            Ok(specs) => {
                                println!("\n✅ SPICE Benchmark Completed Successfully!");
                                println!(
                                    "  • Input Stage Type:     {}",
                                    specs.input_stage.map(|s| s.as_str()).unwrap_or("Unknown")
                                );
                                println!(
                                    "  • Input Impedance @1k:  {:.1} MΩ",
                                    specs.zin_1k.unwrap_or(0.0) / 1.0e6
                                );
                                println!(
                                    "  • Voltage Noise @1k:    {:.2} nV/√Hz",
                                    specs.noise_spot_1k.unwrap_or(0.0)
                                );
                                if let Some(fc) = specs.noise_corner_freq {
                                    println!("  • 1/f Flicker Corner:   {:.1} Hz", fc);
                                }
                                println!(
                                    "  • Rail Saturation Drop: {:.2} V (RRIO: {})",
                                    specs.rail_margin_v.unwrap_or(0.0),
                                    if specs.is_rrio == Some(true) {
                                        "YES (★ RRIO)"
                                    } else {
                                        "NO (Standard Drop)"
                                    }
                                );
                                println!(
                                    "  • Unity Gain Bandwidth: {:.2} MHz",
                                    specs.gbw_mhz.unwrap_or(0.0)
                                );
                                println!("=================================================================================\n");
                            }
                            Err(e) => {
                                eprintln!("Benchmark failed: {}", e);
                                std::process::exit(1);
                            }
                        }
                    } else {
                        println!(
                            "Component '{}' is a {}. Direct model text is valid.",
                            part.name,
                            part.kind.as_str()
                        );
                    }
                }

                PartCommands::Add { file } => {
                    if !file.exists() {
                        eprintln!("Error: File not found: {:?}", file);
                        std::process::exit(1);
                    }

                    println!("\n=================================================================================");
                    println!("  📥 IMPORTING VENDOR SPICE MODEL: {:?}", file);
                    println!("=================================================================================");

                    match catalog.ingest_vendor_file(&file) {
                        Ok(names) => {
                            println!("Extracted {} model(s): {:?}", names.len(), names);
                            for name in &names {
                                if let Some(part) = catalog.get(name) {
                                    println!("  • Ingested '{}' as {}", part.name, part.kind.as_str());
                                    if part.kind == parts::PartKind::OpAmp {
                                        print!("    Benchmarking '{}' in SPICE... ", part.name);
                                        match catalog.benchmark_opamp(name) {
                                            Ok(measured_specs) => {
                                                println!(
                                                    "DONE! (Noise: {:.2} nV/√Hz, Zin: {:.1} MΩ, GBW: {:.1} MHz)",
                                                    measured_specs.noise_spot_1k.unwrap_or(0.0),
                                                    measured_specs.zin_1k.unwrap_or(0.0) / 1.0e6,
                                                    measured_specs.gbw_mhz.unwrap_or(0.0)
                                                );
                                                // Save to parts/{name}.toml
                                                let mut updated_part = part.clone();
                                                updated_part.specs = measured_specs;
                                                let parts_dir = PathBuf::from("parts");
                                                let _ = fs::create_dir_all(&parts_dir);
                                                let out_path = parts_dir.join(format!("{}.toml", name.to_lowercase()));
                                                if let Ok(toml_str) = toml::to_string_pretty(&updated_part) {
                                                    let _ = fs::write(&out_path, toml_str);
                                                    println!("    Saved part definition to {:?}", out_path);
                                                }
                                            }
                                            Err(e) => {
                                                println!("Warning: Benchmark failed ({}), registering raw model.", e);
                                            }
                                        }
                                    }
                                }
                            }
                            println!("\n🎉 Successfully imported into İmbik component catalog!");
                        }
                        Err(e) => {
                            eprintln!("Error ingesting SPICE file: {}", e);
                            std::process::exit(1);
                        }
                    }
                }

                PartCommands::Template { kind } => {
                    let toml_str = parts::PartCatalog::template_toml(&kind);
                    print!("{}", toml_str);
                }
            }
        }

        Commands::Preset { action } => match action {
            PresetCommands::List => {
                let presets = preset::Preset::list_available();
                println!("\n========================================================================================================");
                println!("  🎯 İMBİK MISSION PRESETS & TARGET CATALOG (Total: {})", presets.len());
                println!("========================================================================================================");
                println!("{:<20} | {:<8} | {:<60}", "NAME", "PROBES", "DESCRIPTION");
                println!("{:-<104}", "");
                for (name, desc, probe_cnt) in presets {
                    println!("{:<20} | {:<8} | {:<60}", name, probe_cnt, desc);
                }
                println!("{:-<104}", "");
                println!("Tips: Use `imbik preset show <name>` to inspect or `imbik preset template` for TOML format.\n");
            }
            PresetCommands::Show { name } => {
                let p = match preset::Preset::load_or_builtin(&name) {
                    Ok(pr) => pr,
                    Err(e) => {
                        eprintln!("Error loading preset '{}': {}", name, e);
                        std::process::exit(1);
                    }
                };

                println!("\n=================================================================================");
                println!("  🎯 MISSION PRESET: {}", p.name);
                println!("=================================================================================");
                println!("- Description:             {}", p.description);
                println!("- Checksum:                {}", p.checksum());
                println!("- Feasibility Max DC:      {:.2} V", p.feasibility_max_dc);
                println!("- Feasibility Rail Margin: {:.2} V", p.feasibility_rail_margin);
                println!("- Allow Op-Amps:           {}", p.allow_opamps);
                println!("- Component Models:");
                println!("    • Op-Amp:  {} (Default/Configured)", p.models.opamp);
                println!("    • BJT NPN: {}", p.models.bjt_npn);
                println!("    • BJT PNP: {}", p.models.bjt_pnp);
                println!("    • Diode:   {}", p.models.diode);
                println!("\nTarget Fitness Probes (Total: {}):", p.probes.len());
                for (i, pr) in p.probes.iter().enumerate() {
                    let req_str = if pr.is_required { " [REQUIRED]" } else { "" };
                    println!(
                        "  {}. {:<14} : {:?} {:?} want={:.2}, soft={:.2}, wt={:.1}{}",
                        i + 1,
                        pr.name,
                        pr.probe_type,
                        pr.kind,
                        pr.want,
                        pr.soft,
                        pr.weight,
                        req_str
                    );
                    println!(
                        "     Condition: f={:.0}Hz, Vin={:.3}V, Rload={:.0}Ω, Rsrc={:.0}Ω",
                        pr.condition.freq, pr.condition.vin, pr.condition.r_load, pr.condition.r_source
                    );
                }
                println!("=================================================================================\n");
            }
            PresetCommands::Check { name, no_spice } => {
                let p = match preset::Preset::load_or_builtin(&name) {
                    Ok(pr) => pr,
                    Err(e) => {
                        eprintln!("Error loading preset '{}': {}", name, e);
                        std::process::exit(1);
                    }
                };

                let catalog = parts::PartCatalog::load_with_overrides();
                let report = p.validate_and_check(&catalog, !no_spice);
                println!("{}", report.format_diagnostic());

                if !report.is_valid {
                    std::process::exit(1);
                }
            }
            PresetCommands::Template => {
                let toml_str = preset::Preset::template_toml();
                print!("{}", toml_str);
            }
        },

        Commands::Show { id, checkpoint } => {
            let cp_path = checkpoint.unwrap_or_else(find_default_checkpoint);

            let views = match loot::load_archive_views(&cp_path) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("Error reading archive: {}", e);
                    std::process::exit(1);
                }
            };

            if views.is_empty() {
                eprintln!("Archive in {:?} is empty.", cp_path);
                std::process::exit(1);
            }

            let circuit_id = match resolve_circuit_id(&id, &views) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            };

            if let Err(e) = loot::show_circuit(&cp_path, circuit_id) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }

        Commands::Bench {
            id,
            out_dir,
            checkpoint,
        } => {
            let cp_path = checkpoint.unwrap_or_else(find_default_checkpoint);

            let views = match loot::load_archive_views(&cp_path) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("Error reading archive: {}", e);
                    std::process::exit(1);
                }
            };

            let circuit_id = match resolve_circuit_id(&id, &views) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            };

            if let Err(e) =
                loot::export_bench_from_checkpoint(&cp_path, circuit_id, out_dir.as_deref())
            {
                eprintln!("Error exporting bench package: {}", e);
                std::process::exit(1);
            }
        }

        Commands::Draw {
            id,
            out_svg,
            checkpoint,
        } => {
            let cp_path = checkpoint.unwrap_or_else(find_default_checkpoint);

            let views = match loot::load_archive_views(&cp_path) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("Error reading archive: {}", e);
                    std::process::exit(1);
                }
            };

            let circuit_id = match resolve_circuit_id(&id, &views) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            };

            if let Err(e) = loot::draw_circuit_schematic(&cp_path, circuit_id, out_svg.as_deref()) {
                eprintln!("Error generating schematic: {}", e);
                std::process::exit(1);
            }
        }

        Commands::Ingest {
            scope_csv,
            circuit_id,
            checkpoint,
        } => {
            if !scope_csv.exists() {
                eprintln!(
                    "Error: Oscilloscope capture file not found: {:?}",
                    scope_csv
                );
                std::process::exit(1);
            }

            let cp_path = checkpoint.unwrap_or_else(find_default_checkpoint);

            if !cp_path.exists() {
                eprintln!("Error: Checkpoint file not found: {:?}", cp_path);
                std::process::exit(1);
            }

            // Find reference.csv either in bench/circuit_{id}/reference.csv or generate it
            let candidate_ref =
                PathBuf::from(format!("bench/circuit_{:02}/reference.csv", circuit_id));
            let _temp_guard = tempfile::Builder::new()
                .prefix("imbik_ingest_bench_")
                .tempdir()
                .ok();
            let ref_csv_path = if candidate_ref.exists() {
                candidate_ref
            } else if let Some(ref td) = _temp_guard {
                let _ = loot::export_bench_from_checkpoint(&cp_path, circuit_id, Some(td.path()));
                td.path().join("reference.csv")
            } else {
                eprintln!("Error creating temp dir for reference extraction");
                std::process::exit(1);
            };

            if !ref_csv_path.exists() {
                eprintln!(
                    "Error: Reference simulation data not found at {:?}",
                    ref_csv_path
                );
                std::process::exit(1);
            }

            println!("\n============================================================");
            println!("  🔬 Physical Hardware Verification & Oscilloscope Ingest    ");
            println!("============================================================");
            println!("Scope Capture:     {:?}", scope_csv);
            println!("SPICE Reference:   {:?}", ref_csv_path);
            println!("Target Circuit ID: #{}", circuit_id);

            match bench::ingest_scope_data(&scope_csv, &ref_csv_path) {
                Ok(res) => {
                    println!("\n{}", res.details);
                    if res.is_verified {
                        println!(
                            "\n🏆 SUCCESS: Measured oscilloscope waveforms match SPICE reality with r = {:.4}!",
                            res.pearson_r
                        );
                        println!(
                            "🎉 Circuit #{} is now HARDWARE VERIFIED! Rarity upgraded to LEGENDARY!",
                            circuit_id
                        );

                        // Update checkpoint with is_hardware_verified = true
                        if let Ok(content) = std::fs::read_to_string(&cp_path)
                            && let Ok(mut cp) =
                                serde_json::from_str::<engine::CheckpointData>(&content)
                                && let Some(entry) = cp.archive.entries.get_mut(circuit_id) {
                                    entry.is_hardware_verified = true;
                                    if let Ok(updated_json) = serde_json::to_string_pretty(&cp) {
                                        let _ = std::fs::write(&cp_path, updated_json);
                                        println!(
                                            "Saved updated hardware-verified status to {:?}",
                                            cp_path
                                        );
                                    }
                                }
                    } else {
                        println!(
                            "\n⚠️ WARNING: Deviation between physical hardware and SPICE simulation!"
                        );
                        println!(
                            "   Pearson r: {:.4} (need >= 0.85), NRMSE: {:.4} (need <= 0.35)",
                            res.pearson_r, res.nrmse
                        );
                        println!(
                            "   Check breadboard component tolerances, rail voltages, and grounding."
                        );
                    }
                }
                Err(e) => {
                    eprintln!("Ingest analysis failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Evolve {
            preset,
            pop,
            seed,
            generations,
        } => {
            let mut config = EvolutionConfig::default();
            config.population_size = pop;
            config.max_generations = generations;

            if let Some(preset_name) = preset {
                match preset::Preset::load_or_builtin(&preset_name) {
                    Ok(p) => {
                        let catalog = parts::PartCatalog::load_with_overrides();
                        let report = p.validate_and_check(&catalog, true);
                        if !report.is_valid {
                            eprintln!("\n❌ PRESET / MODEL VALIDATION FAILED: Cannot start evolution.");
                            for issue in &report.issues {
                                if issue.severity == preset::ValidationSeverity::Error {
                                    eprintln!("  • [ERROR] {}: {}", issue.component, issue.message);
                                    if let Some(ref sug) = issue.suggestion {
                                        eprintln!("    ↳ Suggestion: {}", sug);
                                    }
                                }
                            }
                            println!();
                            std::process::exit(1);
                        }

                        let warnings: Vec<_> = report
                            .issues
                            .iter()
                            .filter(|i| i.severity == preset::ValidationSeverity::Warning)
                            .collect();
                        if !warnings.is_empty() {
                            println!("\n⚠️ Preset Validation Warnings:");
                            for w in warnings {
                                println!("  • [WARN] {}: {}", w.component, w.message);
                                if let Some(ref sug) = w.suggestion {
                                    println!("    ↳ Suggestion: {}", sug);
                                }
                            }
                            println!();
                        }

                        config.preset = Some(p);
                    }
                    Err(err) => {
                        eprintln!("Error loading preset: {}", err);
                        std::process::exit(1);
                    }
                }
            }

            if let Some(s_str) = seed {
                match parse_seed_str(&s_str) {
                    Ok(s) => config.seed = Some(s),
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        std::process::exit(1);
                    }
                }
            }

            let running = Arc::new(AtomicBool::new(true));
            let r = running.clone();

            if let Err(e) = ctrlc::set_handler(move || {
                println!(
                    "\n[Ctrl-C detected] Halting evolution gracefully after current generation..."
                );
                r.store(false, Ordering::SeqCst);
            }) {
                eprintln!("Warning: Failed to set Ctrl-C handler: {}", e);
            }

            let mut engine = EvolutionEngine::new(config);
            match engine.run(running) {
                Ok(checkpoint) => {
                    println!("\n============================================================");
                    println!(
                        "  Evolution Run Finished! Archive Size: {} topologies",
                        checkpoint.archive.len()
                    );
                    println!("============================================================");
                }
                Err(e) => {
                    eprintln!("Engine execution error: {}", e);
                }
            }
        }
    }
}
