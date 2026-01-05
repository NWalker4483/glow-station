use std::sync::{Arc, Mutex};
mod experiment;
mod tec;
mod fan;
use experiment::{Experiment, Parameters};
use tec::*;

use crate::fan::Fan;
fn main() -> std::io::Result<()> {
    println!("Initializing TEC Controller...");

env_logger::init();
    // Initialize TEC controller - adjust port name as needed (/dev/ttyUSB0, /dev/ttyACM0, etc.)
    let tec_controller = match TecController::new("/dev/serial0") {
        Ok(controller) => Arc::new(Mutex::new(controller)),
        Err(e) => {
            eprintln!("Failed to initialize TEC controller: {}", e);
            return Ok(());
        }
    };

    // Set up experiment parameters
    let params = Parameters::default(); 
    let fan = Fan::new(0,0,25_000).unwrap();

    // Create and run experiment
    let mut experiment = Experiment::new(tec_controller, fan, params);

    match experiment.run() {
        Ok(()) => println!("Experiment completed successfully!"),
        Err(e) => eprintln!("Experiment failed: {}", e),
    }

    Ok(())
}
