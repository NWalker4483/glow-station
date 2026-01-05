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
/// Guard that ensures the camera process is killed when dropped
struct VideoProcessGuard {
    process: Child,
}

impl VideoProcessGuard {
    fn new(process: Child) -> Self {
        VideoProcessGuard { process }
    }
}

impl Drop for VideoProcessGuard {
    fn drop(&mut self) {
        println!("Stopping video recording...");
        // let _ = self.process.kill();

            let pid = Pid::from_raw(self.process.id() as i32);

    println!("Child process spawned with PID: {}", pid);

    // 2. Wait for a moment to ensure the process is running
    thread::sleep(Duration::from_secs(1));

    // 3. Send SIGINT (Signal 2) to the process
    println!("Sending SIGINT (Signal 2) to the child process...");
    nix::sys::signal::kill(pid, Some(Signal::SIGINT)).unwrap();

    println!("Signal sent. Waiting for child to terminate...");

    // 4. Wait for the child process to finish after receiving the signal
    let status = self.process.wait().unwrap();
    println!("Child process exited with status: {}", status);


    }
}

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

pub struct Experiment {
    tec: Arc<Mutex<TecController>>,
    fan: Fan,
    params: Parameters,
    experiment_dir: String,
}

impl Experiment {
    pub fn new(tec_controller: Arc<Mutex<TecController>>, fan: Fan, params: Parameters) -> Self {
        Experiment {
            tec: tec_controller,
            fan: fan,
            params,
            experiment_dir: String::new(),
        }
    }

    

    fn create_experiment_directory(&mut self) -> std::io::Result<()> {
        // Create experiments base directory if it doesn't exist
        fs::create_dir_all("experiments")?;
        
        // Create timestamped subdirectory for this experiment
        let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
        self.experiment_dir = format!("experiments/experiment_{}", timestamp);
        fs::create_dir_all(&self.experiment_dir)?;
        
        println!("Created experiment directory: {}", self.experiment_dir);
        Ok(())
    }

   
    fn save_parameters(&self) -> std::io::Result<()> {
        let params_path = format!("{}/parameters.yaml", self.experiment_dir);
        
        let yaml_string = serde_yaml::to_string(&self.params)
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
        println!("Waiting for temperature to reach {:.1}°C...", target_temp);
        let start_time = SystemTime::now();
        let max_wait = Duration::from_secs_f32(self.params.max_wait_time);

        loop {
            if start_time.elapsed().unwrap() > max_wait {
                return Err(format!(
                    "Timeout waiting for temperature to reach {:.1}°C",
                    target_temp
                ));
            }

            match self.tec.lock() {
                Ok(mut controller) => match controller.get_single_readout() {
                    Ok(readout) => {
                        let temp_diff = (readout.t_measured - target_temp).abs();
                        if temp_diff <= self.params.temperature_tolerance {
                            println!(
                                "Temperature reached: {:.1}°C (target: {:.1}°C)",
                                readout.t_measured, target_temp
                            );
                            return Ok(());
                        }
                        println!(
                            "Current: {:.1}°C, Target: {:.1}°C, Diff: {:.1}°C",
                            readout.t_measured, target_temp, temp_diff
                        );
                    }
                    Err(e) => eprintln!("Failed to read temperature: {}", e),
                },
                Err(e) => eprintln!("Failed to lock TEC controller: {}", e),
            }

            thread::sleep(Duration::from_millis(1000));
        }
    }

    fn start_video_recording(&self) -> std::io::Result<VideoProcessGuard> {
        println!("Starting video capture...");

        let video_filename = format!("{}/video.h264", self.experiment_dir);
        let pts_filename = format!("{}/timestamps.txt", self.experiment_dir);

        // Set timeout to  to ensure it captures the full experiment
        // We'll kill it manually when done
        let process = Command::new("rpicam-vid")
            .args([
                "-o",
                &video_filename,
                "-t",
                "0", 
                "--save-pts",
                &pts_filename,
                "--flush",
                "--nopreview",
                "--mode","1920:1080:10:P"
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        
        Ok(VideoProcessGuard::new(process))
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
        self.create_experiment_directory()?;

        // Save parameters to YAML
        if let Err(e) = self.save_parameters() {
            eprintln!("Failed to save parameters: {}", e);
            return Err(Box::new(e));
        }

        // Initialize log file
        if let Err(e) = self.initialize_log_file() {
            eprintln!("Failed to initialize log file: {}", e);
            return Err(Box::new(e));
        }

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
        self.fan.on_full();

        // Set initial rest temperature and wait for stabilization
        {
            let mut controller = self.tec.lock().unwrap();
            controller.set_t(self.params.rest_temp);
        }
        self.wait_for_temperature(self.params.rest_temp)?;

        // Start video recording
        let _video_guard = self.start_video_recording()?;

        // Start temperature logging thread
        let _logging_thread = self.start_temperature_logging();

        // Pre-record phase at rest temperature
        println!(
            "Phase 1: Pre-recording at rest temperature for {:.1}s",
            self.params.prerecord_time
        );
        thread::sleep(Duration::from_secs_f32(self.params.prerecord_time));

        // Change to snap temperature
        println!(
            "Phase 2: Changing to snap temperature {:.1}°C",
            self.params.snap_temp
        );
        {
            let mut controller = self.tec.lock().unwrap();
            controller.set_t(self.params.snap_temp);
        }
        self.wait_for_temperature(self.params.snap_temp)?;

        // Hold at snap temperature
        println!(
            "Phase 3: Holding at snap temperature for {:.1}s",
            self.params.snap_hold_time
        );
        thread::sleep(Duration::from_secs_f32(self.params.snap_hold_time));

        // Return to rest temperature
        println!(
            "Phase 4: Returning to rest temperature {:.1}°C",
            self.params.rest_temp
        );
        {
            let mut controller = self.tec.lock().unwrap();
            controller.set_t(self.params.rest_temp);
        }
        // Note: We don't wait for temperature to stabilize here as we want to capture the cooling

        // Post-record phase
        println!(
            "Phase 5: Post-recording for {:.1}s",
            self.params.postrecord_time
        );
        thread::sleep(Duration::from_secs_f32(self.params.postrecord_time));

        // Video process will be automatically stopped when _video_guard goes out of scop
        self.fan.off();
        // Disable TEC
        {
            let mut controller = self.tec.lock().unwrap();
            println!("Disabling TEC...");
            match controller.disable() {
                Ok(response) => println!("TEC disabled: {}", response),
                Err(e) => eprintln!("Failed to disable TEC: {}", e),
            }
        }

        println!("Experiment completed!");
        println!("Results saved to: {}", self.experiment_dir);
        println!("  - parameters.yaml");
        println!("  - video.h264");
        println!("  - timestamps.txt");
        println!("  - temperature_log.csv");

        Ok(())
    }
}