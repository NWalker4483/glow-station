use serialport::SerialPort;
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

///Template of readout: <Tsetpoint P I D Tmin Tmax Tmeasured OC PWM>
#[derive(Debug, Clone)]
pub struct TecReadout {
    ///[°C] – target temperature value. The device checks output temperature via
    ///thermistor and manages output current to reach temperature setpoint.
    pub t_set: f32,
    ///PID proportional coefficient (in range from 0.0 to 20.0)
    pub p: f32,
    /// PID integral coefficient (in range from 0.0 to 20.0)
    pub i: f32,
    /// PID derivative coefficient (in range from 0.0 to 20.0)
    pub d: f32,
    /// [°C] - lower threshold which is checked by the driver (OC)
    pub t_min: f32,
    /// [°C] - upper threshold which is checked by the driver (OC)
    pub t_max: f32,
    /// [°C] - instantaneous value of measured temperature
    pub t_measured: f32,
    /// OC - status of the open collector (0 – disconnected from GND or 1 – connected to GND)
    pub oc: bool,
    ///[%] - Negative values indicate cooling phase and positive ones indicate heating phase. The absolute value of the parameter informs about intensity.
    pub pwm: f32,
}

fn parse_readout(response: &str) -> Result<TecReadout, Box<dyn std::error::Error>> {
    let sections: Vec<&str> = response.trim().split('=').collect();

    if sections.len() < 9 {
        return Err("Not enough sections in response".into());
    }

    // Helper function to filter and parse a section
    let parse_section = |section: &str| -> Result<f32, Box<dyn std::error::Error>> {
        let filtered: String = section
            .chars()
            .filter(|c| !c.is_alphabetic() && !c.is_whitespace())
            .collect();
        Ok(filtered.parse::<f32>()?)
    };

    // Parse individual sections (skip first section)
    let t_set = parse_section(sections[1])?;
    let P = parse_section(sections[2])?;
    let I = parse_section(sections[3])?;
    let D = parse_section(sections[4])?;

    // Parse section 5 (t_min...t_max)
    let temp_range_filtered: String = sections[5]
        .chars()
        .filter(|c| !c.is_alphabetic() && !c.is_whitespace())
        .collect();
    let temp_parts: Vec<&str> = temp_range_filtered.split("...").collect();
    if temp_parts.len() != 2 {
        return Err("Invalid temperature range format".into());
    }
    let t_min = temp_parts[0].parse::<f32>()?;
    let t_max = temp_parts[1].parse::<f32>()?;

    let t_measured = parse_section(sections[6])?;

    // Parse OC as boolean
    let oc_filtered: String = sections[7]
        .chars()
        .filter(|c| !c.is_alphabetic() && !c.is_whitespace())
        .collect();
    let OC = match oc_filtered.parse::<u8>()? {
        0 => false,
        1 => true,
        _ => return Err("Invalid OC value".into()),
    };

    let PWM = parse_section(sections[8])?;

    Ok(TecReadout {
        t_set,
        p: P,
        i: I,
        d: D,
        t_min,
        t_max,
        t_measured,
        oc: OC,
        pwm: PWM,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure the serial port
    let port_name = "/dev/serial0"; // Change this to your actual port

    // Open the serial port with detailed configuration
    let mut port = serialport::new(port_name, 38400)
        .timeout(Duration::from_millis(1000))
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .open()?;

    println!("Connected to {}", port_name);

    // Send a single character command
    let command: u8 = b'o'; // Example: send 'o' character
    port.write_all(&[command])?;
    port.flush()?;

    println!("Sent command: '{}'", command as char);

    // Read the response line
    let mut reader = BufReader::new(&mut port);
    let mut response = String::new();
    reader.read_line(&mut response)?;

    // Print the first response
    println!("First Response: {}", response.trim());

    // Read the second response
    response.clear();
    reader.read_line(&mut response)?;

    // Parse the second response using the parse_readout function
    match parse_readout(&response) {
        Ok(readout) => {
            println!("Parsed TEC Readout:");
            println!("{:#?}", readout);
        }
        Err(e) => {
            println!("Failed to parse readout: {}", e);
            println!("Raw response: {}", response.trim());
        }
    }

    Ok(())
}
