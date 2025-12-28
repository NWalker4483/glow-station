use std::fs::{self, OpenOptions};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
mod tec;
use tec::*;
/// Time after recording start that temperature change steated
/// time recording after target temp reached
/// hold temp time
///
/// illuminator brightness
///
/// handle illuminator and fan
///
fn get_current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn log_temperature_data(tec_controller: &Arc<Mutex<TecController>>) {
    let timestamp = get_current_timestamp();

    match tec_controller.lock() {
        Ok(mut controller) => match controller.get_single_readout() {
            Ok(readout) => {
                let log_entry = format!(
                    "{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{},{:.1}\n",
                    timestamp,
                    readout.t_set,
                    readout.P,
                    readout.I,
                    readout.D,
                    readout.t_min,
                    readout.t_max,
                    readout.t_measured,
                    if readout.OC { 1 } else { 0 },
                    readout.PWM
                );

                match OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("temperature_log.csv")
                {
                    Ok(mut file) => {
                        // if let Err(e) = file.write_all(log_entry.as_bytes()) {
                        //     eprintln!("Failed to write to log file: {}", e);
                        // }
                    }
                    Err(e) => eprintln!("Failed to open log file: {}", e),
                }
            }
            Err(e) => eprintln!("Failed to read TEC data: {}", e),
        },
        Err(e) => eprintln!("Failed to lock TEC controller: {}", e),
    }
}

fn initialize_log_file() -> std::io::Result<()> {
    let header = "timestamp_ms,T_setpoint,P,I,D,T_min,T_max,T_measured,OC,PWM\n";
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("temperature_log.csv")?;
    // file.write_all(header.as_bytes())?;
    Ok(())
}

fn main() -> std::io::Result<()> {
    println!("Initializing TEC Controller...");

    // Initialize TEC controller - adjust port name as needed (/dev/ttyUSB0, /dev/ttyACM0, etc.)
    let tec_controller = match TecController::new("/dev/serial0") {
        Ok(controller) => Arc::new(Mutex::new(controller)),
        Err(e) => {
            eprintln!("Failed to initialize TEC controller: {}", e);
            eprintln!("Make sure the device is connected and the port is correct.");
            eprintln!("Try ports like: /dev/ttyUSB0, /dev/ttyACM0, /dev/ttyAMA0");
            return Ok(());
        }
    };

    // Initialize CSV log file with headers
    if let Err(e) = initialize_log_file() {
        eprintln!("Failed to initialize log file: {}", e);
        return Err(e);
    }

    // Configure TEC (adjust these values as needed for your application)
    let config = TecConfig {
        t_set: 25.0, // Target temperature in °C
        p: 1.0,      // Proportional gain
        i: 0.1,      // Integral gain
        d: 0.05,     // Derivative gain
        t_min: 10.0, // Minimum temperature threshold
        t_max: 40.0, // Maximum temperature threshold
    };

    // Set up TEC controller
    {
        let mut controller = tec_controller.lock().unwrap();

        println!("Configuring TEC...");
        match controller.set_configuration(&config) {
            Ok(response) => println!("TEC configured: {}", response),
            Err(e) => eprintln!("Failed to configure TEC: {}", e),
        }

        println!("Enabling TEC...");
        match controller.enable() {
            Ok(response) => println!("TEC enabled: {}", response),
            Err(e) => eprintln!("Failed to enable TEC: {}", e),
        }
    }

    // Start rpicam-vid
    println!("Starting video capture...");
    let mut child = Command::new("rpicam-vid")
        .args([
            "-o",
            "video.h264",
            "-t",
            "10000",
            "--save-pts",
            "timestamps.txt",
            "--flush",
        ])
        .spawn()?;

    // Clone the Arc for use in the monitoring thread
    let tec_controller_clone = Arc::clone(&tec_controller);

    // Monitor PTS file for new timestamps
    let monitor_thread = thread::spawn(move || {
        let mut last_size = 0;
        println!("Starting temperature monitoring...");

        loop {
            if let Ok(metadata) = fs::metadata("timestamps.txt") {
                let current_size = metadata.len();
                if current_size > last_size {
                    // New frame timestamp available - log temperature data
                    log_temperature_data(&tec_controller_clone);
                    last_size = current_size;
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
    });

    println!("Recording video and logging temperature data...");
    println!("Temperature data will be saved to: temperature_log.csv");

    // Wait for video capture to complete
    let result = child.wait()?;

    // Disable TEC when done
    {
        let mut controller = tec_controller.lock().unwrap();
        println!("Disabling TEC...");
        match controller.disable() {
            Ok(response) => println!("TEC disabled: {}", response),
            Err(e) => eprintln!("Failed to disable TEC: {}", e),
        }
    }

    println!("Video capture completed with status: {}", result);
    println!("Temperature log saved to: temperature_log.csv");

    Ok(())
}
