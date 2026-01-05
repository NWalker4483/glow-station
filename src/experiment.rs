use crate::camera::Camera;
use crate::fan::Fan;
use crate::tec::*;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use nix::unistd::Pid;
use nix::sys::signal::{self, Signal};
use indicatif::{ProgressBar, ProgressStyle};

#[derive(Serialize, Deserialize)]
pub struct Parameters {
    pub rest_temp: f32,
    pub snap_temp: f32,
    pub snap_hold_time: f32,        // in seconds
    pub prerecord_time: f32,        // in seconds
    pub postrecord_time: f32,       // in seconds
    pub temperature_tolerance: f32, // tolerance for reaching target temp
    pub max_wait_time: f32,         // max time to wait for temperature stabilization
}

impl Default for Parameters {
    fn default() -> Self {
        Parameters {
            rest_temp: 25.0,
            snap_temp: 35.0,
            snap_hold_time: 5.0,
            prerecord_time: 5.0,
            postrecord_time: 10.0,
            temperature_tolerance: 0.5,
            max_wait_time: 30.0,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct PhaseTiming {
    pub phase_name: String,
    pub start_time_ms: u64,
    pub end_time_ms: u64,
    pub duration_s: f64,
}

pub struct Experiment {
    tec: Arc<Mutex<TecController>>,
    fan: Fan,
    params: Parameters,
    experiment_dir: String,
    phase_timings: Vec<PhaseTiming>,
}

impl Experiment {
    pub fn new(tec_controller: Arc<Mutex<TecController>>, fan: Fan, params: Parameters) -> Self {
        Experiment {
            tec: tec_controller,
            fan,
            params,
            experiment_dir: String::new(),
            phase_timings: Vec::new(),
        }
    }

    fn record_phase_timing(&mut self, phase_name: String, start_time_ms: u64, end_time_ms: u64) {
        let duration_s = (end_time_ms - start_time_ms) as f64 / 1000.0;
        self.phase_timings.push(PhaseTiming {
            phase_name,
            start_time_ms,
            end_time_ms,
            duration_s,
        });
    }

    fn save_phase_timings(&self) -> std::io::Result<()> {
        let timings_path = format!("{}/phase_timings.yaml", self.experiment_dir);
        
        let yaml_string = serde_yaml::to_string(&self.phase_timings)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&timings_path)?;
        file.write_all(yaml_string.as_bytes())?;
        println!("Phase timings saved to: {}", timings_path);
        Ok(())
    }

    fn initialize_log_file(&self) -> std::io::Result<()> {
        let header = "timestamp_ms,T_setpoint,P,I,D,T_min,T_max,T_measured,OC,PWM\n";
        let log_path = format!("{}/temperature_log.csv", self.experiment_dir);
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_path)?;
        file.write_all(header.as_bytes())?;
        Ok(())
    }

    fn wait_for_temperature(&self, target_temp: f32) -> Result<(), String> {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} [{elapsed_precise}] {msg}")
                .unwrap(),
        );
        pb.set_message(format!("Waiting for temperature to reach {:.1}°C...", target_temp));

        let start_time = SystemTime::now();
        let max_wait = Duration::from_secs_f32(self.params.max_wait_time);

        loop {
            pb.tick();
            
            if start_time.elapsed().unwrap() > max_wait {
                pb.finish_with_message(format!("❌ Timeout waiting for {:.1}°C", target_temp));
                return Err(format!(
                    "Timeout waiting for temperature to reach {:.1}°C",
                    target_temp
                ));
            }

            match self.tec.lock() {
                Ok(mut controller) => match controller.get_single_readout() {
                    Ok(readout) => {
                        let temp_diff = (readout.t_measured - target_temp).abs();
                        pb.set_message(format!(
                            "Current: {:.1}°C | Target: {:.1}°C | Diff: {:.1}°C",
                            readout.t_measured, target_temp, temp_diff
                        ));

                        if temp_diff <= self.params.temperature_tolerance {
                            pb.finish_with_message(format!(
                                "✓ Temperature reached: {:.1}°C (target: {:.1}°C)",
                                readout.t_measured, target_temp
                            ));
                            return Ok(());
                        }
                    }
                    Err(e) => eprintln!("Failed to read temperature: {}", e),
                },
                Err(e) => eprintln!("Failed to lock TEC controller: {}", e),
            }

            thread::sleep(Duration::from_millis(1000));
        }
    }

    fn start_temperature_logging(&self) -> thread::JoinHandle<()> {
        let tec_clone = Arc::clone(&self.tec);
        let log_path = format!("{}/temperature_log.csv", self.experiment_dir);

        thread::spawn(move || {
            println!("Starting temperature logging...");
            let log_interval = Duration::from_millis(100); // Log every 100ms

            loop {
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;

                match tec_clone.lock() {
                    Ok(mut controller) => match controller.get_single_readout() {
                        Ok(readout) => {
                            let log_entry = format!(
                                "{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{},{:.1}\n",
                                timestamp,
                                readout.t_set,
                                readout.p,
                                readout.i,
                                readout.d,
                                readout.t_min,
                                readout.t_max,
                                readout.t_measured,
                                if readout.oc { 1 } else { 0 },
                                readout.pwm
                            );

                            match OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&log_path)
                            {
                                Ok(mut file) => {
                                    if let Err(e) = file.write_all(log_entry.as_bytes()) {
                                        eprintln!("Failed to write to log file: {}", e);
                                    }
                                }
                                Err(e) => eprintln!("Failed to open log file: {}", e),
                            }
                        }
                        Err(e) => eprintln!("Failed to read TEC data: {}", e),
                    },
                    Err(e) => eprintln!("Failed to lock TEC controller: {}", e),
                }

                thread::sleep(log_interval);
            }
        })
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        println!("Starting experiment...");
        println!("Parameters:");
        println!("  Rest temperature: {:.1}°C", self.params.rest_temp);
        println!("  Snap temperature: {:.1}°C", self.params.snap_temp);
        println!("  Pre-record time: {:.1}s", self.params.prerecord_time);
        println!("  Snap hold time: {:.1}s", self.params.snap_hold_time);
        println!("  Post-record time: {:.1}s", self.params.postrecord_time);

        // Create experiment directory
        self.experiment_dir = create_experiment_directory()?;

        // Save parameters to YAML
        save_parameters(&self.experiment_dir, &self.params)?;

        // Initialize log file
        self.initialize_log_file()?;

        // Configure and enable TEC
        {
            let mut controller = self.tec.lock().unwrap();

            // Configure TEC with appropriate PID values
            let config = TecConfig {
                t_set: self.params.rest_temp,
                ..controller.current_config
            };

            println!("Configuring TEC...");
            match controller.set_configuration(&config) {
                Ok(response) => println!("TEC configured: {}", response),
                Err(e) => {
                    eprintln!("Failed to configure TEC: {}", e);
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("TEC configuration failed: {}", e),
                    )));
                }
            }

            println!("Enabling TEC...");
            match controller.enable() {
                Ok(response) => println!("TEC enabled: {}", response),
                Err(e) => {
                    eprintln!("Failed to enable TEC: {}", e);
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("TEC enable failed: {}", e),
                    )));
                }
            }
        }
        // self.fan.on_full();

        // Phase 0: Initial temperature stabilization
        let phase_start = get_timestamp_ms();
        {
            let mut controller = self.tec.lock().unwrap();
            controller.set_t(self.params.rest_temp);
        }
        self.wait_for_temperature(self.params.rest_temp)?;
        let phase_end = get_timestamp_ms();
        self.record_phase_timing("Initial stabilization".to_string(), phase_start, phase_end);

        // Initialize camera
        let mut camera = Camera::new(&self.experiment_dir);
        camera.start()?;

        // Start temperature logging thread
        let _logging_thread = self.start_temperature_logging();

        // Phase 1: Pre-record at rest temperature
        let phase_start = get_timestamp_ms();
        let pb = create_phase_progress_bar(self.params.prerecord_time, "Phase 1: Pre-recording at rest temperature");
        sleep_with_progress(&pb, self.params.prerecord_time);
        pb.finish_with_message("✓ Phase 1 complete");
        let phase_end = get_timestamp_ms();
        self.record_phase_timing("Pre-record at rest temp".to_string(), phase_start, phase_end);

        // Phase 2: Change to snap temperature
        let phase_start = get_timestamp_ms();
        println!("Phase 2: Changing to snap temperature {:.1}°C", self.params.snap_temp);
        {
            let mut controller = self.tec.lock().unwrap();
            controller.set_t(self.params.snap_temp);
        }
        self.wait_for_temperature(self.params.snap_temp)?;
        let phase_end = get_timestamp_ms();
        self.record_phase_timing("Heat to snap temp".to_string(), phase_start, phase_end);

        // Phase 3: Hold at snap temperature
        let phase_start = get_timestamp_ms();
        let pb = create_phase_progress_bar(self.params.snap_hold_time, "Phase 3: Holding at snap temperature");
        sleep_with_progress(&pb, self.params.snap_hold_time);
        pb.finish_with_message("✓ Phase 3 complete");
        let phase_end = get_timestamp_ms();
        self.record_phase_timing("Hold at snap temp".to_string(), phase_start, phase_end);

        // Phase 4: Return to rest temperature
        let phase_start = get_timestamp_ms();
        println!("Phase 4: Returning to rest temperature {:.1}°C", self.params.rest_temp);
        {
            let mut controller = self.tec.lock().unwrap();
            controller.set_t(self.params.rest_temp);
        }
        // Note: We don't wait for temperature to stabilize here as we want to capture the cooling
        let phase_end = get_timestamp_ms();
        self.record_phase_timing("Initiate cooling".to_string(), phase_start, phase_end);

        // Phase 5: Post-record
        let phase_start = get_timestamp_ms();
        let pb = create_phase_progress_bar(self.params.postrecord_time, "Phase 5: Post-recording");
        sleep_with_progress(&pb, self.params.postrecord_time);
        pb.finish_with_message("✓ Phase 5 complete");
        let phase_end = get_timestamp_ms();
        self.record_phase_timing("Post-record".to_string(), phase_start, phase_end);

        // Stop camera
        camera.stop()?;

        // self.fan.off();

        // Disable TEC
        {
            let mut controller = self.tec.lock().unwrap();
            println!("Disabling TEC...");
            match controller.disable() {
                Ok(response) => println!("TEC disabled: {}", response),
                Err(e) => eprintln!("Failed to disable TEC: {}", e),
            }
        }

        // Save phase timings
        self.save_phase_timings()?;

        println!("\n✓ Experiment completed!");
        println!("Results saved to: {}", self.experiment_dir);
        println!("  - parameters.yaml");
        println!("  - phase_timings.yaml");
        println!("  - video.h264");
        println!("  - timestamps.txt");
        println!("  - temperature_log.csv");

        Ok(())
    }
}

// Utility functions

/// Create a timestamped experiment directory
fn create_experiment_directory() -> std::io::Result<String> {
    // Create experiments base directory if it doesn't exist
    fs::create_dir_all("experiments")?;
    
    // Create timestamped subdirectory for this experiment
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let experiment_dir = format!("experiments/experiment_{}", timestamp);
    fs::create_dir_all(&experiment_dir)?;
    
    println!("Created experiment directory: {}", experiment_dir);
    Ok(experiment_dir)
}

/// Save experiment parameters to YAML file
fn save_parameters(experiment_dir: &str, params: &Parameters) -> std::io::Result<()> {
    let params_path = format!("{}/parameters.yaml", experiment_dir);
    
    let yaml_string = serde_yaml::to_string(params)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&params_path)?;
    file.write_all(yaml_string.as_bytes())?;
    println!("Parameters saved to: {}", params_path);
    Ok(())
}

/// Get current timestamp in milliseconds
fn get_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Create a progress bar for a timed phase
fn create_phase_progress_bar(duration_s: f32, message: &str) -> ProgressBar {
    let pb = ProgressBar::new((duration_s * 10.0) as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg}\n[{elapsed_precise}] [{bar:40.cyan/blue}] {percent}% ({eta})")
            .unwrap()
            .progress_chars("=>-"),
    );
    pb.set_message(message.to_string());
    pb
}

/// Sleep with progress bar updates
fn sleep_with_progress(pb: &ProgressBar, duration_s: f32) {
    let steps = (duration_s * 10.0) as u64;
    for _ in 0..steps {
        thread::sleep(Duration::from_millis(100));
        pb.inc(1);
    }
}