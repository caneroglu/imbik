use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// Single frequency point in AC small-signal response
#[derive(Debug, Clone, PartialEq)]
pub struct AcPoint {
    pub freq: f64,
    pub mag_db: f64,
    pub phase_deg: f64,
}

/// Single time point in transient response
#[derive(Debug, Clone, PartialEq)]
pub struct TranPoint {
    pub time: f64,
    pub v_out: f64,
    pub v_in: Option<f64>,
}

/// Single frequency point in probe AC response (gain and input impedance)
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeAcPoint {
    pub freq: f64,
    pub gain: f64,
    pub zin: f64,
}

/// Structured simulation output
#[derive(Debug, Clone, Default)]
pub struct SimulationResult {
    /// DC Operating Point node voltages (e.g. "out" -> 2.5)
    pub dc_nodes: HashMap<String, f64>,
    /// AC frequency response data (if .ac was run)
    pub ac_response: Option<Vec<AcPoint>>,
    /// Probe AC response (gain and zin)
    pub probe_ac: Option<Vec<ProbeAcPoint>>,
    /// Transient waveform data (if .tran was run)
    pub tran_response: Option<Vec<TranPoint>>,
    /// Small-signal transient waveform (if tran_small.txt was produced)
    pub tran_small: Option<Vec<TranPoint>>,
    /// Large-signal transient waveform (if tran_large.txt was produced)
    pub tran_large: Option<Vec<TranPoint>>,
    /// Zero-input transient waveform (if tran_zero.txt was produced)
    pub tran_zero: Option<Vec<TranPoint>>,
}

#[derive(Debug)]
pub enum SpiceError {
    Timeout(Duration),
    SubprocessFailed(String),
    ParseError(String),
    IoError(std::io::Error),
}

impl std::fmt::Display for SpiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpiceError::Timeout(d) => write!(f, "ngspice simulation timed out after {:?}", d),
            SpiceError::SubprocessFailed(msg) => write!(f, "ngspice failed: {}", msg),
            SpiceError::ParseError(msg) => write!(f, "Failed to parse simulation output: {}", msg),
            SpiceError::IoError(e) => write!(f, "I/O error during simulation: {}", e),
        }
    }
}

impl std::error::Error for SpiceError {}

impl From<std::io::Error> for SpiceError {
    fn from(err: std::io::Error) -> Self {
        SpiceError::IoError(err)
    }
}

/// Run a SPICE netlist string through ngspice in batch mode with a timeout.
pub fn run_simulation(netlist: &str, timeout: Duration) -> Result<SimulationResult, SpiceError> {
    let temp_dir = tempfile::Builder::new()
        .prefix("imbik_sim_")
        .tempdir()?;

    let circuit_path = temp_dir.path().join("circuit.cir");
    let log_path = temp_dir.path().join("ngspice.log");
    let ac_data_path = temp_dir.path().join("ac_out.txt");
    let tran_data_path = temp_dir.path().join("tran_out.txt");
    let tran_small_path = temp_dir.path().join("tran_small.txt");
    let tran_large_path = temp_dir.path().join("tran_large.txt");
    let tran_zero_path = temp_dir.path().join("tran_zero.txt");
    let probe_ac_path = temp_dir.path().join("probe_ac.txt");

    fs::write(&circuit_path, netlist)?;

    // Copy any models from ./models if directory exists
    let models_dir = Path::new("models");
    if models_dir.exists() && models_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(models_dir) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                if file_path.is_file() {
                    if let Some(file_name) = file_path.file_name() {
                        let _ = fs::copy(&file_path, temp_dir.path().join(file_name));
                    }
                }
            }
        }
    }

    // Spawn ngspice in batch mode
    let mut child = Command::new("ngspice")
        .arg("-b")
        .arg("-o")
        .arg(&log_path)
        .arg(&circuit_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .current_dir(temp_dir.path())
        .spawn()?;

    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => {
                let log_content = fs::read_to_string(&log_path).unwrap_or_default();

                // Check for simulation error indicators in exit status or log content
                if !status.success() || log_content.contains("Error:") || log_content.contains("Simulation interrupted") {
                    let err_summary = extract_error_summary(&log_content);
                    return Err(SpiceError::SubprocessFailed(err_summary));
                }
                break;
            }
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SpiceError::Timeout(timeout));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    let log_content = fs::read_to_string(&log_path).unwrap_or_default();
    let dc_nodes = parse_dc_operating_point(&log_content);

    let ac_response = if ac_data_path.exists() {
        Some(parse_ac_data(&ac_data_path)?)
    } else {
        None
    };

    let tran_response = if tran_data_path.exists() {
        Some(parse_tran_data(&tran_data_path)?)
    } else {
        None
    };

    let tran_small = if tran_small_path.exists() {
        Some(parse_tran_data(&tran_small_path)?)
    } else {
        None
    };

    let tran_large = if tran_large_path.exists() {
        Some(parse_tran_data(&tran_large_path)?)
    } else {
        None
    };

    let tran_zero = if tran_zero_path.exists() {
        Some(parse_tran_data(&tran_zero_path)?)
    } else {
        None
    };

    let probe_ac = if probe_ac_path.exists() {
        Some(parse_probe_ac_data(&probe_ac_path)?)
    } else {
        None
    };

    Ok(SimulationResult {
        dc_nodes,
        ac_response,
        probe_ac,
        tran_response,
        tran_small,
        tran_large,
        tran_zero,
    })
}

/// Extract human-readable error lines from ngspice log
fn extract_error_summary(log: &str) -> String {
    let mut errors = Vec::new();
    for line in log.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Error:")
            || trimmed.contains("Simulation interrupted")
            || trimmed.contains("singular matrix")
            || trimmed.contains("timestep too small")
            || trimmed.contains("no convergence")
        {
            errors.push(trimmed);
        }
    }

    if errors.is_empty() {
        // Fallback: return last few lines of log
        log.lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        errors.join("; ")
    }
}

/// Parse DC operating point node voltages from `print allv` output
fn parse_dc_operating_point(log: &str) -> HashMap<String, f64> {
    let mut nodes = HashMap::new();
    for line in log.lines() {
        let trimmed = line.trim();
        if let Some((lhs, rhs)) = trimmed.split_once('=') {
            let node_name = lhs.trim().to_lowercase();
            let val_str = rhs.trim();
            // Exclude non-node rows like "No. of Data Rows" or temp info
            if node_name.contains("rows") || node_name.contains("temp") {
                continue;
            }
            if let Ok(val) = val_str.parse::<f64>() {
                // Insert raw node name (e.g. "v(2)" or "out")
                nodes.insert(node_name.clone(), val);
                // Also insert normalized inner name if formatted as v(...)
                if node_name.starts_with("v(") && node_name.ends_with(')') {
                    let inner = &node_name[2..node_name.len() - 1];
                    nodes.insert(inner.to_string(), val);
                }
            }
        }
    }
    nodes
}

/// Parse AC output generated by `wrdata ac_out.txt v(out)`
pub fn parse_ac_data(path: &Path) -> Result<Vec<AcPoint>, SpiceError> {
    let content = fs::read_to_string(path)?;
    let mut points = Vec::new();

    for (line_idx, line) in content.lines().enumerate() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        // Format 1: 3 columns (freq, real, imag) from wrdata v(out)
        if parts.len() == 3 {
            let freq = parts[0]
                .parse::<f64>()
                .map_err(|e| SpiceError::ParseError(format!("Line {}: freq parse error: {}", line_idx + 1, e)))?;
            let real = parts[1]
                .parse::<f64>()
                .map_err(|e| SpiceError::ParseError(format!("Line {}: real parse error: {}", line_idx + 1, e)))?;
            let imag = parts[2]
                .parse::<f64>()
                .map_err(|e| SpiceError::ParseError(format!("Line {}: imag parse error: {}", line_idx + 1, e)))?;

            let mag_sq = real * real + imag * imag;
            let mag_db = if mag_sq > 0.0 {
                10.0 * mag_sq.log10()
            } else {
                -200.0
            };
            let phase_deg = imag.atan2(real).to_degrees();

            points.push(AcPoint {
                freq,
                mag_db,
                phase_deg,
            });
        }
        // Format 2: 4 columns (freq, mag_db, freq, phase_deg) from wrdata vdb(out) vp(out)
        else if parts.len() == 4 {
            let freq = parts[0]
                .parse::<f64>()
                .map_err(|e| SpiceError::ParseError(format!("Line {}: freq parse error: {}", line_idx + 1, e)))?;
            let mag_db = parts[1]
                .parse::<f64>()
                .map_err(|e| SpiceError::ParseError(format!("Line {}: mag_db parse error: {}", line_idx + 1, e)))?;
            let phase_deg = parts[3]
                .parse::<f64>()
                .map_err(|e| SpiceError::ParseError(format!("Line {}: phase parse error: {}", line_idx + 1, e)))?;

            points.push(AcPoint {
                freq,
                mag_db,
                phase_deg,
            });
        } else {
            return Err(SpiceError::ParseError(format!(
                "Unexpected column count {} in AC data on line {}",
                parts.len(),
                line_idx + 1
            )));
        }
    }

    Ok(points)
}

/// Parse transient output generated by `wrdata tran_out.txt v(out) [v(in)]`
pub fn parse_tran_data(path: &Path) -> Result<Vec<TranPoint>, SpiceError> {
    let content = fs::read_to_string(path)?;
    let mut points = Vec::new();

    for (line_idx, line) in content.lines().enumerate() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        if parts.len() == 2 {
            let time = parts[0]
                .parse::<f64>()
                .map_err(|e| SpiceError::ParseError(format!("Line {}: time parse error: {}", line_idx + 1, e)))?;
            let v_out = parts[1]
                .parse::<f64>()
                .map_err(|e| SpiceError::ParseError(format!("Line {}: v_out parse error: {}", line_idx + 1, e)))?;
            points.push(TranPoint {
                time,
                v_out,
                v_in: None,
            });
        } else if parts.len() == 4 {
            let time = parts[0]
                .parse::<f64>()
                .map_err(|e| SpiceError::ParseError(format!("Line {}: time parse error: {}", line_idx + 1, e)))?;
            let v_out = parts[1]
                .parse::<f64>()
                .map_err(|e| SpiceError::ParseError(format!("Line {}: v_out parse error: {}", line_idx + 1, e)))?;
            let v_in = parts[3]
                .parse::<f64>()
                .map_err(|e| SpiceError::ParseError(format!("Line {}: v_in parse error: {}", line_idx + 1, e)))?;
            points.push(TranPoint {
                time,
                v_out,
                v_in: Some(v_in),
            });
        } else {
            return Err(SpiceError::ParseError(format!(
                "Unexpected column count {} in transient data on line {}",
                parts.len(),
                line_idx + 1
            )));
        }
    }

    Ok(points)
}

/// Parse probe AC output generated by `wrdata probe_ac.txt gain zin`
pub fn parse_probe_ac_data(path: &Path) -> Result<Vec<ProbeAcPoint>, SpiceError> {
    let content = fs::read_to_string(path)?;
    let mut points = Vec::new();

    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let freq = parts[0].parse::<f64>().unwrap_or(0.0);
            let gain = parts[1].parse::<f64>().unwrap_or(0.0);
            let zin = parts[3].parse::<f64>().unwrap_or(0.0);
            points.push(ProbeAcPoint { freq, gain, zin });
        }
    }

    Ok(points)
}

/// Fast Feasibility Gate: Run ONLY `.op` in ngspice to inspect DC node voltages
pub fn run_dc_operating_point(netlist: &str, timeout: Duration) -> Result<HashMap<String, f64>, SpiceError> {
    let temp_dir = tempfile::Builder::new()
        .prefix("imbik_dc_")
        .tempdir()?;

    let circuit_path = temp_dir.path().join("circuit.cir");
    let log_path = temp_dir.path().join("ngspice.log");

    fs::write(&circuit_path, netlist)?;

    let models_dir = Path::new("models");
    if models_dir.exists() && models_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(models_dir) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                if file_path.is_file() {
                    if let Some(file_name) = file_path.file_name() {
                        let _ = fs::copy(&file_path, temp_dir.path().join(file_name));
                    }
                }
            }
        }
    }

    let mut child = Command::new("ngspice")
        .arg("-b")
        .arg("-o")
        .arg(&log_path)
        .arg(&circuit_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .current_dir(temp_dir.path())
        .spawn()?;

    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => {
                let log_content = fs::read_to_string(&log_path).unwrap_or_default();
                if !status.success() || log_content.contains("Error:") || log_content.contains("Simulation interrupted") {
                    return Err(SpiceError::SubprocessFailed(extract_error_summary(&log_content)));
                }
                break;
            }
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SpiceError::Timeout(timeout));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    let log_content = fs::read_to_string(&log_path).unwrap_or_default();
    let dc_nodes = parse_dc_operating_point(&log_content);
    Ok(dc_nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stage 1 Acceptance Criterion:
    /// Hand-written 1kΩ + 159nF RC lowpass netlist finds -3dB point within 1kHz ± 5%.
    #[test]
    fn test_rc_lowpass_cutoff() {
        let netlist = r#"* RC Lowpass Acceptance Test
V1 in 0 dc 0 ac 1
R1 in out 1k
C1 out 0 159nF
.control
ac dec 100 10 100k
wrdata ac_out.txt v(out)
quit
.endc
.end
"#;

        let res = run_simulation(netlist, Duration::from_secs(2)).expect("Simulation failed");
        let ac = res.ac_response.expect("AC response missing");
        assert!(!ac.is_empty(), "AC response points should not be empty");

        // Passband level at lowest frequency (~10 Hz) should be ~0 dB
        assert!(
            (ac[0].mag_db - 0.0).abs() < 0.1,
            "Passband magnitude at 10Hz should be ~0dB, got {:.2}dB",
            ac[0].mag_db
        );

        // Find the frequency where magnitude is closest to -3.01 dB
        let target_db = -3.0103;
        let closest = ac
            .iter()
            .min_by(|a, b| {
                (a.mag_db - target_db)
                    .abs()
                    .partial_cmp(&(b.mag_db - target_db).abs())
                    .unwrap()
            })
            .expect("Should find closest point");

        let cutoff_freq = closest.freq;
        let expected_fc = 1000.0;
        let margin = expected_fc * 0.05; // ±5% tolerance (950Hz - 1050Hz)

        println!(
            "Detected -3dB cutoff frequency: {:.2} Hz (magnitude: {:.2} dB, phase: {:.2}°)",
            cutoff_freq, closest.mag_db, closest.phase_deg
        );

        assert!(
            (cutoff_freq - expected_fc).abs() <= margin,
            "Cutoff freq {:.2} Hz is outside 1kHz ± 5% (margin: {:.2} Hz)",
            cutoff_freq,
            margin
        );
    }

    /// Verify DC Operating Point reading
    #[test]
    fn test_dc_voltage_divider() {
        let netlist = r#"* DC Voltage Divider Test
V1 in 0 5
R1 in mid 1k
R2 mid 0 1k
.control
op
print allv
quit
.endc
.end
"#;

        let res = run_simulation(netlist, Duration::from_secs(2)).expect("Simulation failed");
        let in_v = res.dc_nodes.get("in").copied().expect("Node 'in' missing");
        let mid_v = res.dc_nodes.get("mid").copied().expect("Node 'mid' missing");

        assert!((in_v - 5.0).abs() < 0.01, "Expected in=5V, got {}", in_v);
        assert!((mid_v - 2.5).abs() < 0.01, "Expected mid=2.5V, got {}", mid_v);
    }

    /// Verify Transient response parser
    #[test]
    fn test_transient_sine() {
        let netlist = r#"* Transient Sine Test
V1 in 0 sin(0 1 1k)
R1 in out 1k
C1 out 0 159nF
.control
tran 10u 2m
wrdata tran_out.txt v(out) v(in)
quit
.endc
.end
"#;

        let res = run_simulation(netlist, Duration::from_secs(2)).expect("Simulation failed");
        let tran = res.tran_response.expect("Transient response missing");
        assert!(tran.len() > 50, "Transient should contain data points");

        // Verify time advances
        assert!(tran[0].time <= tran[1].time);
        assert!(tran.last().unwrap().time >= 1.9e-3);
    }

    /// Verify ngspice syntax error handling
    #[test]
    fn test_syntax_error_handling() {
        let netlist = r#"* Broken Netlist
V1 in 0 5
R1 in out 1k
X1 out 0 NONEXISTENT_SUBCKT
.control
op
quit
.endc
.end
"#;

        let res = run_simulation(netlist, Duration::from_secs(2));
        assert!(res.is_err(), "Broken netlist should return Err");
        if let Err(SpiceError::SubprocessFailed(err_msg)) = res {
            println!("Captured expected error message: {}", err_msg);
        } else {
            panic!("Expected SubprocessFailed error");
        }
    }
}
