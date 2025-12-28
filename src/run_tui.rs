// Example main.rs for standalone TEC monitor binary
// This can be used if you want to create a separate binary for the TUI

use std::env;
use std::process;

mod tec; // Your existing TEC controller module
mod tui;       // The TUI module

use tui::run_tui;

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() != 2 {
        eprintln!("TEC Controller Monitor");
        eprintln!("Usage: {} <serial_port>", args[0]);
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  /dev/ttyUSB0   ");
        eprintln!("   COM3");
        eprintln!();
        eprintln!("Controls:");
        eprintln!("  Tab       - Switch between panes");
        eprintln!("  ↑↓        - Navigate parameters");
        eprintln!("  Enter     - Edit parameter or toggle TEC");
        eprintln!("  'a'       - Apply configuration");
        eprintln!("  'q'       - Quit");
        process::exit(1);
    }
    
    let port_name = &args[1];
    
    println!("Starting TEC Controller Monitor on port: {}", port_name);
    println!("Press 'q' to quit when running...");
    
    if let Err(e) = run_tui(port_name) {
        eprintln!("Error running TUI: {}", e);
        process::exit(1);
    }
}
