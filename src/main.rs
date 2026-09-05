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

use engine::{EvolutionConfig, EvolutionEngine};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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

fn print_help() {
    println!("Usage:");
    println!("  imbik list [checkpoint.json]               - Display colorized loot table of discovered circuits");
    println!("  imbik show <id> [checkpoint.json]          - Inspect details, netlist & pin map of a circuit");
    println!("  imbik draw <id> [out.svg] [checkpoint]     - Generate publication-quality AoE SchemDraw schematic");
    println!("  imbik bench <id> [out_dir] [checkpoint]    - Export breadboard BOM, protocol, CSV & schematic");
    println!("  imbik ingest <scope.csv> [id] [checkpoint] - Ingest oscilloscope capture & verify physical reality");
    println!("  imbik evolve [--preset <name>] [gens]      - Launch mission-driven or novelty discovery engine");
    println!("  imbik help                                 - Show this help menu\n");
}

fn main() {
    println!("============================================================");
    println!("  İmbik v0.1.0 - Autonomous Analog Circuit Hunter Engine   ");
    println!("============================================================");

    let args: Vec<String> = env::args().collect();

    if args.len() <= 1 {
        let default_cp = find_default_checkpoint();
        if default_cp.exists() {
            println!("Found existing discovery archive at: {:?}", default_cp);
            println!("Displaying loot table:\n");
            if let Err(e) = loot::list_archive(&default_cp) {
                eprintln!("Error reading archive: {}", e);
            }
        } else {
            print_help();
        }
        return;
    }

    match args[1].as_str() {
        "list" => {
            let cp_path = if args.len() >= 3 {
                PathBuf::from(&args[2])
            } else {
                find_default_checkpoint()
            };

            if !cp_path.exists() {
                eprintln!("Error: Checkpoint file not found: {:?}", cp_path);
                std::process::exit(1);
            }

            if let Err(e) = loot::list_archive(&cp_path) {
                eprintln!("Error displaying archive: {}", e);
                std::process::exit(1);
            }
        }

        "show" => {
            let cp_path = if args.len() >= 4 {
                PathBuf::from(&args[3])
            } else {
                find_default_checkpoint()
            };

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

            let id: usize = if args.len() < 3 || args[2] == "best" || args[2] == "latest" {
                // Find candidate with highest fitness or highest NN distance
                views
                    .iter()
                    .max_by(|a, b| {
                        let fit_a = a.fitness.unwrap_or(a.nn_dist);
                        let fit_b = b.fitness.unwrap_or(b.nn_dist);
                        fit_a.partial_cmp(&fit_b).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|v| v.id)
                    .unwrap_or(0)
            } else {
                match args[2].parse() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!("Error: Invalid circuit ID: {}", args[2]);
                        std::process::exit(1);
                    }
                }
            };

            if let Err(e) = loot::show_circuit(&cp_path, id) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }

        "bench" => {
            let cp_path = if args.len() >= 5 {
                PathBuf::from(&args[4])
            } else {
                find_default_checkpoint()
            };

            let id: usize = if args.len() < 3 || args[2] == "best" {
                let views = loot::load_archive_views(&cp_path).unwrap_or_default();
                views
                    .iter()
                    .max_by(|a, b| {
                        let fit_a = a.fitness.unwrap_or(a.nn_dist);
                        let fit_b = b.fitness.unwrap_or(b.nn_dist);
                        fit_a.partial_cmp(&fit_b).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|v| v.id)
                    .unwrap_or(0)
            } else {
                match args[2].parse() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!("Error: Invalid circuit ID: {}", args[2]);
                        std::process::exit(1);
                    }
                }
            };

            let out_dir = args.get(3).map(|s| Path::new(s));

            if let Err(e) = loot::export_bench_from_checkpoint(&cp_path, id, out_dir) {
                eprintln!("Error exporting bench package: {}", e);
                std::process::exit(1);
            }
        }

        "draw" => {
            let cp_path = if args.len() >= 5 {
                PathBuf::from(&args[4])
            } else {
                find_default_checkpoint()
            };

            let id: usize = if args.len() < 3 || args[2] == "best" {
                let views = loot::load_archive_views(&cp_path).unwrap_or_default();
                views
                    .iter()
                    .max_by(|a, b| {
                        let fit_a = a.fitness.unwrap_or(a.nn_dist);
                        let fit_b = b.fitness.unwrap_or(b.nn_dist);
                        fit_a.partial_cmp(&fit_b).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|v| v.id)
                    .unwrap_or(0)
            } else {
                match args[2].parse() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!("Error: Invalid circuit ID: {}", args[2]);
                        std::process::exit(1);
                    }
                }
            };

            let out_svg = args.get(3).map(|s| Path::new(s));

            if let Err(e) = loot::draw_circuit_schematic(&cp_path, id, out_svg) {
                eprintln!("Error generating schematic: {}", e);
                std::process::exit(1);
            }
        }

        "ingest" => {
            if args.len() < 3 {
                eprintln!("Usage: imbik ingest <scope_capture.csv> [circuit_id] [checkpoint_path]");
                std::process::exit(1);
            }

            let scope_csv_path = PathBuf::from(&args[2]);
            if !scope_csv_path.exists() {
                eprintln!("Error: Oscilloscope capture file not found: {:?}", scope_csv_path);
                std::process::exit(1);
            }

            let circuit_id: usize = if args.len() >= 4 {
                args[3].parse().unwrap_or(0)
            } else {
                0
            };

            let cp_path = if args.len() >= 5 {
                PathBuf::from(&args[4])
            } else {
                find_default_checkpoint()
            };

            if !cp_path.exists() {
                eprintln!("Error: Checkpoint file not found: {:?}", cp_path);
                std::process::exit(1);
            }

            // Find reference.csv either in bench/circuit_{id}/reference.csv or generate it
            let candidate_ref = PathBuf::from(format!("bench/circuit_{:02}/reference.csv", circuit_id));
            let _temp_guard = tempfile::Builder::new().prefix("imbik_ingest_bench_").tempdir().ok();
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
                eprintln!("Error: Reference simulation data not found at {:?}", ref_csv_path);
                std::process::exit(1);
            }

            println!("\n============================================================");
            println!("  🔬 Physical Hardware Verification & Oscilloscope Ingest    ");
            println!("============================================================");
            println!("Scope Capture:     {:?}", scope_csv_path);
            println!("SPICE Reference:   {:?}", ref_csv_path);
            println!("Target Circuit ID: #{}", circuit_id);

            match bench::ingest_scope_data(&scope_csv_path, &ref_csv_path) {
                Ok(res) => {
                    println!("\n{}", res.details);
                    if res.is_verified {
                        println!("\n🏆 SUCCESS: Measured oscilloscope waveforms match SPICE reality with r = {:.4}!", res.pearson_r);
                        println!("🎉 Circuit #{} is now HARDWARE VERIFIED! Rarity upgraded to LEGENDARY!", circuit_id);

                        // Update checkpoint with is_hardware_verified = true
                        if let Ok(content) = std::fs::read_to_string(&cp_path) {
                            if let Ok(mut cp) = serde_json::from_str::<engine::CheckpointData>(&content) {
                                if let Some(entry) = cp.archive.entries.get_mut(circuit_id) {
                                    entry.is_hardware_verified = true;
                                    if let Ok(updated_json) = serde_json::to_string_pretty(&cp) {
                                        let _ = std::fs::write(&cp_path, updated_json);
                                        println!("Saved updated hardware-verified status to {:?}", cp_path);
                                    }
                                }
                            }
                        }
                    } else {
                        println!("\n⚠️ WARNING: Deviation between physical hardware and SPICE simulation!");
                        println!("   Pearson r: {:.4} (need >= 0.85), NRMSE: {:.4} (need <= 0.35)", res.pearson_r, res.nrmse);
                        println!("   Check breadboard component tolerances, rail voltages, and grounding.");
                    }
                }
                Err(e) => {
                    eprintln!("Ingest analysis failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        "evolve" => {
            let mut config = EvolutionConfig::default();
            let mut idx = 2;
            while idx < args.len() {
                match args[idx].as_str() {
                    "--preset" | "-p" => {
                        if idx + 1 < args.len() {
                            idx += 1;
                            let preset_name = &args[idx];
                            match preset::Preset::load_or_builtin(preset_name) {
                                Ok(p) => config.preset = Some(p),
                                Err(err) => {
                                    eprintln!("Error loading preset: {}", err);
                                    std::process::exit(1);
                                }
                            }
                        } else {
                            eprintln!("Error: --preset requires a preset name or file path");
                            std::process::exit(1);
                        }
                    }
                    "--pop" => {
                        if idx + 1 < args.len() {
                            idx += 1;
                            if let Ok(pop) = args[idx].parse() {
                                config.population_size = pop;
                            }
                        }
                    }
                    "--seed" => {
                        if idx + 1 < args.len() {
                            idx += 1;
                            let s_str = &args[idx];
                            let parsed = if s_str.starts_with("0x") || s_str.starts_with("0X") {
                                u64::from_str_radix(&s_str[2..], 16)
                            } else {
                                s_str.parse::<u64>()
                            };
                            match parsed {
                                Ok(s) => config.seed = Some(s),
                                Err(_) => {
                                    eprintln!("Error: Invalid u64 seed: {}", s_str);
                                    std::process::exit(1);
                                }
                            }
                        }
                    }
                    val => {
                        if let Ok(gens) = val.parse() {
                            config.max_generations = gens;
                        }
                    }
                }
                idx += 1;
            }

            let running = Arc::new(AtomicBool::new(true));
            let r = running.clone();

            if let Err(e) = ctrlc::set_handler(move || {
                println!("\n[Ctrl-C detected] Halting evolution gracefully after current generation...");
                r.store(false, Ordering::SeqCst);
            }) {
                eprintln!("Warning: Failed to set Ctrl-C handler: {}", e);
            }

            let mut engine = EvolutionEngine::new(config);
            match engine.run(running) {
                Ok(checkpoint) => {
                    println!("\n============================================================");
                    println!("  Evolution Run Finished! Archive Size: {} topologies", checkpoint.archive.len());
                    println!("============================================================");
                }
                Err(e) => {
                    eprintln!("Engine execution error: {}", e);
                }
            }
        }

        "help" | "-h" | "--help" => {
            print_help();
        }

        other => {
            eprintln!("Unknown command: '{}'", other);
            print_help();
        }
    }
}
