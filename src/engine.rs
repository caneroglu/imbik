use crate::circuit::{Circuit, Component, NODE_GND, NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE};
use crate::constraints::check;
use crate::fitness::{
    evaluate_noise, evaluate_preset, extract_ac_features, extract_behavior_descriptor,
    extract_nonlinear_features, extract_oscillation_features, to_characterization_netlist,
    BehaviorDescriptor, NoveltyArchive,
};
use crate::loot::{describe_character, evaluate_rarity};
use crate::mutate::mutate;
use crate::preset::{ObjectiveVector, Preset};
use crate::realism::evaluate_monte_carlo;
use crate::spice::run_simulation;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Configuration parameters for the evolutionary loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionConfig {
    pub population_size: usize,
    pub max_generations: usize,
    pub novelty_threshold: f64,
    pub min_novelty_dist: f64,
    pub k_neighbors: usize,
    pub checkpoint_interval: usize,
    pub checkpoint_dir: PathBuf,
    pub stray_cap_pf: f64,
    pub sim_timeout_secs: u64,
    pub monte_carlo_runs: usize,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub preset: Option<Preset>,
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        EvolutionConfig {
            population_size: 20,
            max_generations: 50,
            novelty_threshold: 0.5,
            min_novelty_dist: 0.25,
            k_neighbors: 3,
            checkpoint_interval: 50,
            checkpoint_dir: PathBuf::from("checkpoints"),
            stray_cap_pf: 22.0,
            sim_timeout_secs: 2,
            monte_carlo_runs: 20,
            seed: None,
            preset: None,
        }
    }
}

/// Statistics collected per generation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationStats {
    pub generation: usize,
    pub valid_topologies: usize,
    pub constraint_passed: usize,
    pub archive_size: usize,
    pub max_novelty: f64,
    pub avg_novelty: f64,
    pub duration_ms: u128,
}

/// Checkpoint format serialized to disk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointData {
    pub generation: usize,
    pub config: EvolutionConfig,
    pub stats: GenerationStats,
    pub archive: NoveltyArchive,
    pub population: Vec<Circuit>,
}

/// Individual candidate evaluation result
#[allow(dead_code)]
#[derive(Clone)]
struct CandidateResult {
    circuit: Circuit,
    descriptor: Option<BehaviorDescriptor>,
    objectives: Option<ObjectiveVector>,
    fitness_score: f64,
    reject_reason: Option<String>,
}

pub struct EvolutionEngine {
    pub config: EvolutionConfig,
    pub archive: NoveltyArchive,
    pub population: Vec<Circuit>,
    pub stats_history: Vec<GenerationStats>,
}

impl EvolutionEngine {
    pub fn new(config: EvolutionConfig) -> Self {
        EvolutionEngine {
            config,
            archive: NoveltyArchive::new(),
            population: Vec::new(),
            stats_history: Vec::new(),
        }
    }

    /// Mutate circuit respecting configured preset component models
    pub fn mutate_circuit(&self, circuit: &Circuit) -> Circuit {
        let models = self.config.preset.as_ref().map(|p| &p.models);
        crate::mutate::mutate_with_models(circuit, models)
    }

    /// Seed the population with templates appropriate for novelty search or preset mission
    pub fn seed_standard_population(&mut self) {
        self.population.clear();

        if let Some(ref preset) = self.config.preset {
            if !preset.allow_opamps {
                // Pure Discrete Mission Seeding (BJTs, Resistors, Capacitors, Diodes - NO Op-Amps)
                self.population = crate::mutate::seed_discrete_population(self.config.population_size);
                return;
            }

            let opamp_model = &preset.models.opamp;

            // Op-Amp Preset Mission Seeding: Op-Amp follower seed + buffered follower with input resistor + 2-stage ladder follower
            let mut op_seed = Circuit::new();
            op_seed.add_component(
                Component::new('X', 1, vec![NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], opamp_model).unwrap(),
            );
            let _ = op_seed.validate();

            let mut op_res_seed = Circuit::new();
            op_res_seed.add_component(Component::new('R', 1, vec![NODE_IN, 10], "10k").unwrap());
            op_res_seed.add_component(
                Component::new('X', 1, vec![10, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], opamp_model).unwrap(),
            );
            let _ = op_res_seed.validate();

            let mut op_res2_seed = Circuit::new();
            op_res2_seed.add_component(Component::new('R', 1, vec![NODE_IN, 10], "10k").unwrap());
            op_res2_seed.add_component(Component::new('R', 2, vec![10, 20], "10k").unwrap());
            op_res2_seed.add_component(
                Component::new('X', 1, vec![20, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], opamp_model).unwrap(),
            );
            let _ = op_res2_seed.validate();

            self.population.push(op_seed.clone());
            self.population.push(op_res_seed.clone());
            self.population.push(op_res2_seed.clone());

            let seeds = [&op_seed, &op_res_seed, &op_res2_seed];
            while self.population.len() < self.config.population_size {
                let base = seeds[self.population.len() % seeds.len()];
                self.population.push(self.mutate_circuit(base));
            }
            return;
        }

        // Default Novelty Search Template 1: Standard 1kHz Sallen-Key Lowpass
        let mut sk = Circuit::new();
        sk.add_component(Component::new('R', 1, vec![NODE_IN, 10], "10k").unwrap());
        sk.add_component(Component::new('R', 2, vec![10, 20], "10k").unwrap());
        sk.add_component(Component::new('C', 1, vec![10, NODE_OUT], "15.9nF").unwrap());
        sk.add_component(Component::new('C', 2, vec![20, NODE_GND], "15.9nF").unwrap());
        sk.add_component(
            Component::new('X', 1, vec![20, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );
        let _ = sk.validate();

        // Template 2: Diode Clipper
        let mut dc = Circuit::new();
        dc.add_component(Component::new('R', 1, vec![NODE_IN, 10], "1k").unwrap());
        dc.add_component(Component::new('D', 1, vec![10, NODE_GND], "1N4148").unwrap());
        dc.add_component(
            Component::new('X', 1, vec![10, NODE_OUT, NODE_VCC, NODE_VEE, NODE_OUT], "TL072").unwrap(),
        );
        let _ = dc.validate();

        // Fill population with variations of seeds
        while self.population.len() < self.config.population_size {
            let base = if self.population.len().is_multiple_of(2) {
                sk.clone()
            } else {
                dc.clone()
            };
            let mutant = self.mutate_circuit(&base);
            self.population.push(mutant);
        }
    }

    /// Run the autonomous evolutionary exploration loop
    pub fn run(
        &mut self,
        running_flag: Arc<AtomicBool>,
    ) -> Result<CheckpointData, Box<dyn std::error::Error>> {
        let seed = self.config.seed.unwrap_or_else(|| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            now ^ (std::process::id() as u64)
        });
        fastrand::seed(seed);

        if self.population.is_empty() {
            self.seed_standard_population();
        }

        fs::create_dir_all(&self.config.checkpoint_dir)?;

        let timeout = Duration::from_secs(self.config.sim_timeout_secs);

        if let Some(ref preset) = self.config.preset {
            let preset_hash = preset.checksum();
            println!(
                "Starting İmbik Preset Mission: '{}' [Checksum: {}] | Seed: 0x{:016x} | Pop: {}, Max Gens: {}, Discrete: {}",
                preset.name, preset_hash, seed, self.config.population_size, self.config.max_generations, !preset.allow_opamps
            );
            log::info!(
                "Preset mission '{}' started (seed: 0x{:016x}, pop: {}, gens: {})",
                preset.name,
                seed,
                self.config.population_size,
                self.config.max_generations
            );
            println!("Target Probes:");
            for p in &preset.probes {
                println!(
                    "  - {:<12}: want={:.2}, soft={:.2}, weight={:.1}, condition={{f={:.0}Hz, vin={:.2}V, rload={:.0}Ω}}",
                    p.name, p.want, p.soft, p.weight, p.condition.freq, p.condition.vin, p.condition.r_load
                );
            }
            println!();
        } else {
            println!(
                "Starting İmbik Evolution Loop: Population = {}, Max Generations = {}, Novelty Threshold = {:.2} | Seed: 0x{:016x}",
                self.config.population_size, self.config.max_generations, self.config.novelty_threshold, seed
            );
            log::info!(
                "Novelty search started (seed: 0x{:016x}, pop: {}, gens: {}, threshold: {:.2})",
                seed,
                self.config.population_size,
                self.config.max_generations,
                self.config.novelty_threshold
            );
        }

        for gen_idx in 1..=self.config.max_generations {
            if !running_flag.load(Ordering::SeqCst) {
                println!("\nGraceful shutdown signal received at generation {}. Saving checkpoint...", gen_idx);
                log::info!("Graceful shutdown signal received at generation {}", gen_idx);
                break;
            }

            let stats = if let Some(ref preset) = self.config.preset.clone() {
                self.step_preset_generation(gen_idx, preset, timeout)
            } else {
                self.step_novelty_generation(gen_idx, timeout)
            };

            self.stats_history.push(stats.clone());

            // Periodic Checkpoint Serialization
            if gen_idx % self.config.checkpoint_interval == 0 || gen_idx == self.config.max_generations {
                let checkpoint_file = self
                    .config
                    .checkpoint_dir
                    .join(format!("checkpoint_gen_{}.json", gen_idx));
                self.save_checkpoint(&checkpoint_file, &stats)?;
            }
        }

        // Final checkpoint upon loop completion
        let final_path = self.config.checkpoint_dir.join("checkpoint_final.json");
        let last_stats = self.stats_history.last().cloned().unwrap_or_default();
        let checkpoint = self.save_checkpoint(&final_path, &last_stats)?;

        Ok(checkpoint)
    }

    /// Single generation step in Preset Mission Mode
    fn step_preset_generation(
        &mut self,
        gen_idx: usize,
        preset: &Preset,
        timeout: Duration,
    ) -> GenerationStats {
        let gen_start = Instant::now();

        // 1. Create pool of parents + mutant offspring (Elitist selection)
        let mut pool = self.population.clone();
        pool.extend(self.population.iter().map(mutate));

        let stray_pf = self.config.stray_cap_pf;
        let preset_clone = preset.clone();
        let total_eval = pool.len();

        // 2. Parallel Evaluation with Feasibility Gate & Pluggable Probes
        let eval_results: Vec<CandidateResult> = pool
            .into_par_iter()
            .map(|circuit| {
                if let Err(e) = circuit.validate() {
                    return CandidateResult {
                        circuit,
                        descriptor: None,
                        objectives: None,
                        fitness_score: 0.0,
                        reject_reason: Some(format!("Validate: {:?}", e)),
                    };
                }

                match evaluate_preset(&circuit, &preset_clone, stray_pf, timeout) {
                    Ok(obj) => {
                        let sc = obj.scalarized();
                        CandidateResult {
                            circuit,
                            descriptor: None,
                            objectives: Some(obj),
                            fitness_score: sc,
                            reject_reason: None,
                        }
                    }
                    Err(e) => CandidateResult {
                        circuit,
                        descriptor: None,
                        objectives: None,
                        fitness_score: 0.0,
                        reject_reason: Some(format!("{}", e)),
                    },
                }
            })
            .collect();

        let val_rejected = eval_results
            .iter()
            .filter(|r| r.reject_reason.as_ref().map(|s| s.starts_with("Validate")).unwrap_or(false))
            .count();

        let gate_rejected = eval_results
            .iter()
            .filter(|r| r.reject_reason.as_ref().map(|s| !s.starts_with("Validate")).unwrap_or(false))
            .count();

        let mut scored: Vec<CandidateResult> = eval_results
            .into_iter()
            .filter(|r| r.objectives.is_some())
            .collect();

        let scored_count = scored.len();

        // Occam's Razor Lexicographic Sort (Silva & Almeida / Luke & Panait epsilon-lexicographic parsimony):
        // 1. Fitness Bucket: Discretized with resolution OCCAM_EPSILON (0.5% = 0.005)
        // 2. Component Count (BOM): Within the same performance bucket, simpler circuits win!
        // 3. Fine Fitness: If same BOM, fine continuous score decides
        // 4. DC Offset: Minimize DC offset
        const OCCAM_EPSILON: f64 = 0.005; // 0.5% physical indifference margin

        let to_bucket = |f: f64| -> i64 {
            if f.is_nan() || f.is_infinite() {
                -1
            } else {
                (f / OCCAM_EPSILON).floor() as i64
            }
        };

        scored.sort_by(|a, b| {
            let bucket_a = to_bucket(a.fitness_score);
            let bucket_b = to_bucket(b.fitness_score);
            let bucket_ord = bucket_b.cmp(&bucket_a); // Descending: higher fitness bucket first
            if bucket_ord != std::cmp::Ordering::Equal {
                return bucket_ord;
            }

            // Within same performance bucket: Occam's Razor (fewer components wins!)
            let comp_ord = a.circuit.components.len().cmp(&b.circuit.components.len());
            if comp_ord != std::cmp::Ordering::Equal {
                return comp_ord;
            }

            // Same component count: fallback to fine continuous fitness
            let f_ord = b.fitness_score.partial_cmp(&a.fitness_score).unwrap_or(std::cmp::Ordering::Equal);
            if f_ord != std::cmp::Ordering::Equal {
                return f_ord;
            }

            let dc_a = a.objectives.as_ref().and_then(|o| o.values.get(2)).copied().unwrap_or(0.0);
            let dc_b = b.objectives.as_ref().and_then(|o| o.values.get(2)).copied().unwrap_or(0.0);
            dc_a.abs().partial_cmp(&dc_b.abs()).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Diversity Selection: Filter duplicates to prevent clone stagnation
        let mut next_pop = Vec::new();
        let mut seen_topologies = std::collections::HashSet::new();

        for r in &scored {
            if next_pop.len() < self.config.population_size {
                let topo_sig = r.circuit.to_netlist("SIG");
                if seen_topologies.insert(topo_sig) {
                    next_pop.push(r.circuit.clone());
                }
            }
        }

        let unique_topologies = seen_topologies.len();
        let duration_ms = gen_start.elapsed().as_millis();
        let mut best_fitness = 0.0;

        if let Some(best) = scored.first() {
            best_fitness = best.fitness_score;
            let best_obj_summary = best.objectives.as_ref().map(|o| o.summary()).unwrap_or_default();
            println!(
                "[Gen {:>2}] Eval: {:>2} | Rej: {:>2} (Gate: {:>2}, Val: {:>2}) | Scored: {:>2} | Uniq: {:>2} | Best: {:.6} | Time: {}ms | {}",
                gen_idx, total_eval, gate_rejected + val_rejected, gate_rejected, val_rejected, scored_count, unique_topologies, best.fitness_score, duration_ms, best_obj_summary
            );
            log::debug!(
                "Gen {} preset: best_fit={:.6}, scored={}/{}, uniq={}",
                gen_idx,
                best.fitness_score,
                scored_count,
                total_eval,
                unique_topologies
            );

            // Archive candidates:
            // 1. Champion is always considered first
            // 2. Additional slots select diverse high-performing candidates maximizing novelty
            let mut archived_in_gen = 0;
            let mut archive_candidates: Vec<&CandidateResult> = Vec::new();
            archive_candidates.push(best);

            // Identify high-performing candidates (>= 50% of champion fitness) with unique topologies
            let mut seen_archive_topos = std::collections::HashSet::new();
            seen_archive_topos.insert(best.circuit.to_netlist("SIG"));

            let min_viable_fitness = (best.fitness_score * 0.5).max(0.2);
            let mut diverse_pool = Vec::new();

            for cand in scored.iter().skip(1) {
                if cand.fitness_score < min_viable_fitness {
                    break;
                }
                let sig = cand.circuit.to_netlist("SIG");
                if seen_archive_topos.insert(sig) {
                    diverse_pool.push(cand);
                    if diverse_pool.len() >= 6 {
                        break;
                    }
                }
            }

            // Score diverse pool by novelty distance to current archive
            let mut novelty_scored: Vec<(&CandidateResult, f64)> = Vec::new();
            for cand in diverse_pool {
                if let Ok(desc) = extract_behavior_descriptor(&cand.circuit, stray_pf, timeout) {
                    let nn_dist = self.archive.min_distance(&desc);
                    novelty_scored.push((cand, nn_dist));
                }
            }

            // Sort by novelty distance descending (most novel first)
            novelty_scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            for (cand, _) in novelty_scored.into_iter().take(2) {
                archive_candidates.push(cand);
            }

            for cand in archive_candidates {
                if cand.fitness_score <= 0.0 || archived_in_gen >= 3 {
                    break;
                }
                let cand_summary = cand.objectives.as_ref().map(|o| o.summary()).unwrap_or_default();
                match extract_behavior_descriptor(&cand.circuit, stray_pf, timeout) {
                    Ok(desc) => {
                        let nn_dist = self.archive.min_distance(&desc);
                        let best_archived_fit = self.archive.entries.iter().filter_map(|e| e.fitness).fold(0.0, f64::max);
                        let is_breakthrough = cand.fitness_score > best_archived_fit;
                        let is_duplicate = !self.archive.is_empty()
                            && nn_dist < self.config.min_novelty_dist
                            && !is_breakthrough;

                        if !is_duplicate {
                            let mc_dev_db = evaluate_monte_carlo(
                                &cand.circuit,
                                self.config.monte_carlo_runs,
                                0.05,
                                stray_pf,
                                timeout,
                            )
                            .ok()
                            .map(|rep| rep.max_deviation_db);

                            let r_source = preset.probes.iter()
                                .find(|p| p.condition.r_source > 0.0)
                                .map(|p| p.condition.r_source)
                                .unwrap_or(600.0);
                            let r_load = preset.probes.iter()
                                .find(|p| p.condition.r_load > 0.0)
                                .map(|p| p.condition.r_load)
                                .unwrap_or(10000.0);

                            let noise_summary = evaluate_noise(&cand.circuit, r_source, r_load, stray_pf, timeout).ok();

                            let effective_min_dist = if is_breakthrough { 0.0 } else { self.config.min_novelty_dist };
                            let added = self.archive.maybe_add_with_meta(
                                cand.circuit.clone(),
                                desc,
                                0.0,
                                self.config.k_neighbors,
                                effective_min_dist,
                                mc_dev_db,
                                gen_idx,
                                Some(cand.fitness_score),
                                Some(cand_summary),
                                noise_summary.clone(),
                            );

                            if added {
                                archived_in_gen += 1;
                                let nn_report = if nn_dist.is_finite() { nn_dist } else { 1.0 };
                                let rarity = evaluate_rarity(nn_report, mc_dev_db, &desc);
                                let mc_str = mc_dev_db
                                    .map(|v| format!("{:.1}dB", v))
                                    .unwrap_or_else(|| "--".to_string());
                                let noise_str = match &noise_summary {
                                    Some(ns) => format!("{:.1} nV/√Hz", ns.inoise_spot_1k),
                                    None => "--".to_string(),
                                };
                                println!(
                                    "  {} DROP  #{} | fit={:.4} | noise={:<11} | nn={:.3} mc={} | {}",
                                    rarity.colored_label(),
                                    self.archive.len() - 1,
                                    cand.fitness_score,
                                    noise_str,
                                    nn_report,
                                    mc_str,
                                    describe_character(&desc)
                                );
                                log::info!(
                                    "Discovery drop #{} [{:?}] fit={:.4}, noise={}, mc={}",
                                    self.archive.len() - 1,
                                    rarity,
                                    cand.fitness_score,
                                    noise_str,
                                    mc_str
                                );
                            }
                        }
                    }
                    Err(e) => {
                        if archived_in_gen == 0 && std::ptr::eq(cand, best) {
                            eprintln!(
                                "  [Gen {}] Champion characterization failed, not archived: {}",
                                gen_idx, e
                            );
                            log::warn!("Gen {} champion characterization failed: {}", gen_idx, e);
                        }
                    }
                }
            }
        } else {
            println!(
                "[Gen {:>2}] Eval: {:>2} | Rej: {:>2} (Gate: {:>2}, Val: {:>2}) | Scored:  0 | Feasibility Gate dropped all candidates | Time: {}ms",
                gen_idx, total_eval, gate_rejected + val_rejected, gate_rejected, val_rejected, duration_ms
            );
            log::warn!("Gen {} feasibility gate dropped all candidates", gen_idx);
        }

        // Fill remaining population slots with fresh mutations of surviving parents (diversity injection)
        let mut fill_idx = 0;
        while next_pop.len() < self.config.population_size {
            if !next_pop.is_empty() {
                let parent = &next_pop[fill_idx % next_pop.len()];
                next_pop.push(self.mutate_circuit(parent));
                fill_idx += 1;
            } else if !self.population.is_empty() {
                let parent = &self.population[fill_idx % self.population.len()];
                next_pop.push(self.mutate_circuit(parent));
                fill_idx += 1;
            } else {
                let fresh = crate::mutate::seed_discrete_population(1);
                next_pop.push(fresh[0].clone());
            }
        }

        self.population = next_pop;

        GenerationStats {
            generation: gen_idx,
            valid_topologies: self.config.population_size,
            constraint_passed: scored_count,
            archive_size: self.archive.len(),
            max_novelty: best_fitness,
            avg_novelty: if scored_count > 0 { best_fitness } else { 0.0 },
            duration_ms,
        }
    }

    /// Single generation step in Novelty Search Mode
    fn step_novelty_generation(&mut self, gen_idx: usize, timeout: Duration) -> GenerationStats {
        let gen_start = Instant::now();

        // 1. Create mutant offspring from previous population
        let offspring: Vec<Circuit> = self
            .population
            .iter()
            .map(mutate)
            .collect();

        // 2. Parallel Evaluation via Rayon across all CPU cores
        let stray_pf = self.config.stray_cap_pf;
        let eval_results: Vec<CandidateResult> = offspring
            .into_par_iter()
            .map(|circuit| {
                // Fast graph topology validation (0ms)
                if let Err(e) = circuit.validate() {
                    return CandidateResult {
                        circuit,
                        descriptor: None,
                        objectives: None,
                        fitness_score: 0.0,
                        reject_reason: Some(format!("Validate: {:?}", e)),
                    };
                }

                // Single-pass characterization simulation
                let netlist = to_characterization_netlist(&circuit, "Gen Eval", stray_pf);
                let sim_res = match run_simulation(&netlist, timeout) {
                    Ok(res) => res,
                    Err(e) => {
                        return CandidateResult {
                            circuit,
                            descriptor: None,
                            objectives: None,
                            fitness_score: 0.0,
                            reject_reason: Some(format!("SPICE: {}", e)),
                        };
                    }
                };

                // S3 Constraint check (fail fast on rails, dead signal, DC offset, phase jump)
                if let Err(reject) = check(&sim_res, circuit.vcc, circuit.vee) {
                    return CandidateResult {
                        circuit,
                        descriptor: None,
                        objectives: None,
                        fitness_score: 0.0,
                        reject_reason: Some(format!("Constraint: {:?}", reject)),
                    };
                }

                let ac_data = match sim_res.ac_response {
                    Some(d) => d,
                    None => {
                        return CandidateResult {
                            circuit,
                            descriptor: None,
                            objectives: None,
                            fitness_score: 0.0,
                            reject_reason: Some("Missing AC".to_string()),
                        };
                    }
                };
                let tran_s = match sim_res.tran_small {
                    Some(d) => d,
                    None => {
                        return CandidateResult {
                            circuit,
                            descriptor: None,
                            objectives: None,
                            fitness_score: 0.0,
                            reject_reason: Some("Missing TranSmall".to_string()),
                        };
                    }
                };
                let tran_l = match sim_res.tran_large {
                    Some(d) => d,
                    None => {
                        return CandidateResult {
                            circuit,
                            descriptor: None,
                            objectives: None,
                            fitness_score: 0.0,
                            reject_reason: Some("Missing TranLarge".to_string()),
                        };
                    }
                };
                let tran_z = sim_res.tran_zero.unwrap_or_default();

                let (has_filt, ac_c, ac_s, ac_q) = extract_ac_features(&ac_data);
                let (asym, h2, h3, h5, comp) = extract_nonlinear_features(&tran_s, &tran_l);
                let (osc_r, osc_f) = extract_oscillation_features(&tran_z);

                let desc: BehaviorDescriptor =
                    [has_filt, ac_c, ac_s, ac_q, asym, h2, h3, h5, comp, osc_r, osc_f];

                CandidateResult {
                    circuit,
                    descriptor: Some(desc),
                    objectives: None,
                    fitness_score: 0.0,
                    reject_reason: None,
                }
            })
            .collect();

        // 3. Sequential Novelty Evaluation and Archive Updates
        let mut passed_candidates = Vec::new();
        let mut novelty_scores = Vec::new();

        for res in eval_results {
            if let Some(desc) = res.descriptor {
                let score = self.archive.novelty_score(&desc, self.config.k_neighbors);
                novelty_scores.push(score);

                let nearest_dist = self.archive.min_distance(&desc);

                // Candidate qualifies for the Novelty Archive if:
                // 1. Distance to nearest neighbor >= min_novelty_dist (prevents near-duplicate pollution)
                // 2. Average k-NN score >= novelty_threshold (or initial seed fill)
                if (score >= self.config.novelty_threshold && nearest_dist >= self.config.min_novelty_dist)
                    || self.archive.len() < self.config.k_neighbors
                {
                    // Monte Carlo Robustness Gate: Run runs ONLY for novel discoveries
                    let mc_report = evaluate_monte_carlo(
                        &res.circuit,
                        self.config.monte_carlo_runs,
                        0.05,
                        stray_pf,
                        timeout,
                    );

                    let dev_db = match &mc_report {
                        Ok(rep) => {
                            println!(
                                "  [Gen {} Novelty Discovery] Score: {:.4}, Dev: {:.2}dB, Runs: {}/{}",
                                gen_idx, score, rep.max_deviation_db, rep.runs_passed, rep.total_runs
                            );
                            log::info!(
                                "Gen {} novelty discovery: score={:.4}, dev={:.2}dB",
                                gen_idx,
                                score,
                                rep.max_deviation_db
                            );
                            Some(rep.max_deviation_db)
                        }
                        Err(_) => None,
                    };

                    let noise_summary = evaluate_noise(&res.circuit, 600.0, 10000.0, stray_pf, timeout).ok();

                    self.archive.maybe_add_with_meta(
                        res.circuit.clone(),
                        desc,
                        self.config.novelty_threshold,
                        self.config.k_neighbors,
                        self.config.min_novelty_dist,
                        dev_db,
                        gen_idx,
                        None,
                        None,
                        noise_summary,
                    );
                }

                passed_candidates.push((res.circuit, score));
            }
        }

        // 4. Survival Selection: Keep the highest-novelty candidates
        passed_candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut next_pop = Vec::new();
        for (c, _) in passed_candidates {
            if next_pop.len() < self.config.population_size {
                next_pop.push(c);
            }
        }

        // Fill any vacancies with mutants from current population or archive
        let mut fill_idx = 0;
        while next_pop.len() < self.config.population_size {
            if !self.archive.entries.is_empty() {
                let seed = &self.archive.entries[fill_idx % self.archive.entries.len()].circuit;
                next_pop.push(self.mutate_circuit(seed));
            } else if !self.population.is_empty() {
                let seed = &self.population[fill_idx % self.population.len()];
                next_pop.push(self.mutate_circuit(seed));
            } else {
                break;
            }
            fill_idx += 1;
        }

        self.population = next_pop;

        let max_nov = novelty_scores
            .iter()
            .cloned()
            .fold(0.0, f64::max);
        let avg_nov = if !novelty_scores.is_empty() {
            novelty_scores.iter().sum::<f64>() / (novelty_scores.len() as f64)
        } else {
            0.0
        };

        let duration_ms = gen_start.elapsed().as_millis();

        let stats = GenerationStats {
            generation: gen_idx,
            valid_topologies: self.config.population_size,
            constraint_passed: novelty_scores.len(),
            archive_size: self.archive.len(),
            max_novelty: max_nov,
            avg_novelty: avg_nov,
            duration_ms,
        };

        println!(
            "Gen {:>3} | Passed: {:>2}/{} | Archive: {:>3} | Max Nov: {:.4} | Avg Nov: {:.4} | Time: {}ms",
            gen_idx,
            stats.constraint_passed,
            self.config.population_size,
            stats.archive_size,
            stats.max_novelty,
            stats.avg_novelty,
            stats.duration_ms
        );
        log::debug!(
            "Gen {} novelty: passed={}/{}, archive={}, max_nov={:.4}, avg_nov={:.4}, time={}ms",
            gen_idx,
            stats.constraint_passed,
            self.config.population_size,
            stats.archive_size,
            stats.max_novelty,
            stats.avg_novelty,
            stats.duration_ms
        );

        stats
    }

    /// Serialize current state to a JSON checkpoint file
    pub fn save_checkpoint(
        &self,
        path: &Path,
        stats: &GenerationStats,
    ) -> Result<CheckpointData, Box<dyn std::error::Error>> {
        let checkpoint = CheckpointData {
            generation: stats.generation,
            config: self.config.clone(),
            stats: stats.clone(),
            archive: self.archive.clone(),
            population: self.population.clone(),
        };

        let json = serde_json::to_string_pretty(&checkpoint)?;
        fs::write(path, json)?;
        println!("Checkpoint saved successfully to: {:?}", path);

        Ok(checkpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Acceptance Criterion:
    /// Run a 50-generation evolutionary exploration run.
    /// 1. At least 1 novel circuit must be discovered and admitted to the archive.
    /// 2. The checkpoint JSON must be successfully written to disk.
    /// 3. The checkpoint JSON must be successfully deserialized back, and all circuits must pass validate().
    #[test]
    fn test_50_generation_evolution_and_checkpoint() {
        let test_dir = PathBuf::from("target/test_checkpoints");
        let _ = fs::remove_dir_all(&test_dir);
        fs::create_dir_all(&test_dir).unwrap();

        let config = EvolutionConfig {
            population_size: 8, // Compact population for blazing fast test execution
            max_generations: 50,
            novelty_threshold: 0.35,
            min_novelty_dist: 0.20,
            k_neighbors: 3,
            checkpoint_interval: 25,
            checkpoint_dir: test_dir.clone(),
            stray_cap_pf: 22.0,
            sim_timeout_secs: 2,
            monte_carlo_runs: 5,
            seed: None,
            preset: None,
        };

        let mut engine = EvolutionEngine::new(config);
        let running = Arc::new(AtomicBool::new(true));

        let checkpoint = engine.run(running).expect("Evolution loop failed");

        // Verification 1: Archive has accumulated discoveries
        println!("Test completed. Final Archive Size: {}", checkpoint.archive.len());
        assert!(
            checkpoint.archive.len() >= 1,
            "Archive must contain at least 1 discovered circuit, got {}",
            checkpoint.archive.len()
        );

        // Verification 2: Checkpoint file exists on disk
        let final_file = test_dir.join("checkpoint_final.json");
        assert!(final_file.exists(), "Final checkpoint JSON must exist on disk");

        // Verification 3: File evidence - read from disk and deserialize
        let content = fs::read_to_string(&final_file).expect("Failed to read checkpoint JSON");
        let loaded: CheckpointData =
            serde_json::from_str(&content).expect("Failed to deserialize checkpoint JSON");

        assert_eq!(loaded.generation, 50);
        assert!(!loaded.archive.is_empty());

        // Validate circuits loaded from checkpoint
        for entry in &loaded.archive.entries {
            entry
                .circuit
                .validate()
                .expect("Archived circuit must be topologically valid");
        }

        println!("Verification successful: 50 generations executed, checkpoint verified on disk.");
    }

    #[test]
    fn test_archive_distribution_analysis() {
        let final_file = PathBuf::from("target/test_checkpoints/checkpoint_final.json");
        if !final_file.exists() {
            println!("checkpoint_final.json does not exist, skipping analysis test");
            return;
        }

        let content = match fs::read_to_string(&final_file) {
            Ok(c) => c,
            Err(_) => {
                println!("checkpoint_final.json could not be read, skipping analysis test");
                return;
            }
        };
        let loaded: CheckpointData = match serde_json::from_str(&content) {
            Ok(d) => d,
            Err(_) => {
                println!("checkpoint_final.json could not be parsed, skipping analysis test");
                return;
            }
        };

        let entries = &loaded.archive.entries;
        let n = entries.len();
        println!("\n================ ARCHIVE DIVERSITY AUDIT ================");
        println!("Total Archive Discoveries: {}", n);

        let mut has_filter_count = 0;
        let mut no_filter_count = 0;

        let mut nonzero_h2_count = 0;
        let mut nonzero_h3_count = 0;
        let mut nonzero_h5_count = 0;
        let mut nonzero_asym_count = 0;
        let mut nonzero_comp_count = 0;
        let mut nonzero_osc_count = 0;
        let mut any_nonlinear_count = 0;

        let mut diode_counts = std::collections::HashMap::new();
        let mut opamp_counts = std::collections::HashMap::new();
        let mut total_comps = Vec::new();

        for entry in entries {
            let d = &entry.descriptor;
            if d[0] > 0.5 {
                has_filter_count += 1;
            } else {
                no_filter_count += 1;
            }

            let mut is_nl = false;
            if d[5] > 0.001 {
                nonzero_h2_count += 1;
                is_nl = true;
            }
            if d[6] > 0.001 {
                nonzero_h3_count += 1;
                is_nl = true;
            }
            if d[7] > 0.001 {
                nonzero_h5_count += 1;
                is_nl = true;
            }
            if d[4] > 0.02 {
                nonzero_asym_count += 1;
                is_nl = true;
            }
            if d[8] > 0.02 {
                nonzero_comp_count += 1;
                is_nl = true;
            }
            if d[9] > 0.02 {
                nonzero_osc_count += 1;
                is_nl = true;
            }
            if is_nl {
                any_nonlinear_count += 1;
            }

            // Topology audit
            let n_diodes = entry.circuit.components.iter().filter(|c| c.comp_type == crate::circuit::ComponentType::D).count();
            let n_opamps = entry.circuit.components.iter().filter(|c| c.comp_type == crate::circuit::ComponentType::X).count();
            *diode_counts.entry(n_diodes).or_insert(0) += 1;
            *opamp_counts.entry(n_opamps).or_insert(0) += 1;
            total_comps.push(entry.circuit.components.len());
        }

        println!("\n1. Filter vs Broadband/Clipper:");
        println!("   has_filter == 1 (True Filter): {} ({:.1}%)", has_filter_count, (has_filter_count as f64 / n as f64) * 100.0);
        println!("   has_filter == 0 (Broadband/Non-filter): {} ({:.1}%)", no_filter_count, (no_filter_count as f64 / n as f64) * 100.0);

        println!("\n2. Non-linear & Dynamic Dimension Counts:");
        println!("   Non-zero H2 (>0.001):          {} ({:.1}%)", nonzero_h2_count, (nonzero_h2_count as f64 / n as f64) * 100.0);
        println!("   Non-zero H3 (>0.001):          {} ({:.1}%)", nonzero_h3_count, (nonzero_h3_count as f64 / n as f64) * 100.0);
        println!("   Non-zero H5 (>0.001):          {} ({:.1}%)", nonzero_h5_count, (nonzero_h5_count as f64 / n as f64) * 100.0);
        println!("   Asymmetry Delta (>0.02):      {} ({:.1}%)", nonzero_asym_count, (nonzero_asym_count as f64 / n as f64) * 100.0);
        println!("   Gain Compression (>0.02):     {} ({:.1}%)", nonzero_comp_count, (nonzero_comp_count as f64 / n as f64) * 100.0);
        println!("   Autonomous Oscillation (>0.02):{} ({:.1}%)", nonzero_osc_count, (nonzero_osc_count as f64 / n as f64) * 100.0);
        println!("   TOTAL with Non-linear Flavor:  {} ({:.1}%)", any_nonlinear_count, (any_nonlinear_count as f64 / n as f64) * 100.0);

        println!("\n3. Topology & Component Diversity:");
        println!("   Diode counts:   {:?}", diode_counts);
        println!("   OpAmp counts:   {:?}", opamp_counts);
        let avg_comps: f64 = total_comps.iter().sum::<usize>() as f64 / n as f64;
        let min_comps = total_comps.iter().min().unwrap_or(&0);
        let max_comps = total_comps.iter().max().unwrap_or(&0);
        println!("   Components: min={}, max={}, avg={:.1}", min_comps, max_comps, avg_comps);

        // Pairwise Distance Matrix
        let mut all_pairwise_dists = Vec::new();
        let mut nn_dists = Vec::new();

        for i in 0..n {
            let mut min_other_dist = f64::INFINITY;
            for j in 0..n {
                if i != j {
                    let dist = crate::fitness::euclidean_distance(&entries[i].descriptor, &entries[j].descriptor);
                    if j > i {
                        all_pairwise_dists.push(dist);
                    }
                    if dist < min_other_dist {
                        min_other_dist = dist;
                    }
                }
            }
            if min_other_dist.is_finite() {
                nn_dists.push(min_other_dist);
            }
        }

        all_pairwise_dists.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        nn_dists.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let avg_pairwise: f64 = all_pairwise_dists.iter().sum::<f64>() / all_pairwise_dists.len().max(1) as f64;
        let median_pairwise = all_pairwise_dists[all_pairwise_dists.len() / 2];
        let min_pairwise = all_pairwise_dists.first().copied().unwrap_or(0.0);
        let max_pairwise = all_pairwise_dists.last().copied().unwrap_or(0.0);

        let avg_nn: f64 = nn_dists.iter().sum::<f64>() / nn_dists.len().max(1) as f64;
        let min_nn = nn_dists.first().copied().unwrap_or(0.0);
        let max_nn = nn_dists.last().copied().unwrap_or(0.0);

        println!("\n4. Distance & Sparsity Metrics:");
        println!("   Pairwise Distances: min={:.4}, max={:.4}, avg={:.4}, median={:.4}", min_pairwise, max_pairwise, avg_pairwise, median_pairwise);
        println!("   Nearest-Neighbor (k=1) Distances: min={:.4}, max={:.4}, avg={:.4}", min_nn, max_nn, avg_nn);

        println!("\n5. Close Pairs Audit (distance < 0.05):");
        let mut close_pairs_count = 0;
        for i in 0..n {
            for j in (i + 1)..n {
                let dist = crate::fitness::euclidean_distance(&entries[i].descriptor, &entries[j].descriptor);
                if dist < 0.05 {
                    close_pairs_count += 1;
                    if close_pairs_count <= 5 {
                        println!("   Pair ({}, {}): dist={:.6}", i, j, dist);
                        println!("     Desc {}: {:?}", i, entries[i].descriptor);
                        println!("     Desc {}: {:?}", j, entries[j].descriptor);
                    }
                }
            }
        }
        println!("   Total pairs with dist < 0.05: {}", close_pairs_count);

        // Calculate size of non-redundant archive with min_dist >= 0.30 and 0.50
        for &eps in &[0.20, 0.30, 0.35, 0.50] {
            let mut pruned: Vec<&crate::fitness::ArchiveEntry> = Vec::new();
            for entry in entries {
                let is_far_enough = pruned.iter().all(|existing| {
                    crate::fitness::euclidean_distance(&entry.descriptor, &existing.descriptor) >= eps
                });
                if is_far_enough {
                    pruned.push(entry);
                }
            }
            println!("   Pruned Archive Size with min_dist >= {:.2}: {} / {}", eps, pruned.len(), n);
        }
        println!("=========================================================\n");
    }

    #[test]
    fn test_low_noise_preamp_mission_execution() {
        let preset = Preset::load_or_builtin("low_noise_preamp").expect("load low_noise_preamp");
        let mut config = EvolutionConfig::default();
        config.population_size = 10;
        config.max_generations = 2;
        config.preset = Some(preset);
        config.seed = Some(0x18d2b4bb78d2a4c8);

        let mut engine = EvolutionEngine::new(config);
        let running = Arc::new(AtomicBool::new(true));
        let res = engine.run(running);
        assert!(res.is_ok(), "Low-noise preamp mission execution should succeed");
    }
}

