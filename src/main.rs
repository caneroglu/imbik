pub mod bench;
pub mod circuit;
pub mod constraints;
pub mod engine;
pub mod fitness;
pub mod loot;
pub mod mutate;
pub mod preset;
pub mod realism;
pub mod spice;

use clap::{Parser, Subcommand};
use engine::{EvolutionConfig, EvolutionEngine};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Parser, Debug)]
#[command(
    name = "imbik",
    version = "0.1.0",
    about = "Autonomous Analog Circuit Hunter Engine",
    after_help = "Examples:\n  imbik list\n  imbik show best\n  imbik draw 0 circuit.svg\n  imbik bench 0\n  imbik evolve --preset buffer 50\n  imbik ingest scope.csv 0"
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

fn find_default_checkpoint() -> PathBuf {
    let candidates = [
        "checkpoints/checkpoint_final.json",
        "target/test_checkpoints/checkpoint_final.json",
        "checkpoints/checkpoint_gen_50.json",
        "target/test_checkpoints/checkpoint_gen_50.json",
    ];

    for c in &candidates {
        let p = Path::new(c);
        if p.exists() {
            return p.to_path_buf();
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

    println!("============================================================");
    println!("  İmbik v0.1.0 - Autonomous Analog Circuit Hunter Engine   ");
    println!("============================================================");

    let cli = Cli::parse();

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
                    Ok(p) => config.preset = Some(p),
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
