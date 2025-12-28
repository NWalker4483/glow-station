use log::{debug, info, trace, warn};
use serialport::TTYPort;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
//*
//UART parameters:
// ● 38400 bits / second
// ● 8 data bits
// ● even parity: No parity
// ● stop bits: 1
// ● flow control: No
// Serial Protocol
// Protocol allows users to set configuration parameters of the device or acquire
// measured data using commands specified below. Each command is a set of ASCII
// characters which may be followed by <CR> and/or <LF>.
//
///Command template: <Tsetpoint P I D Tmin Tmax>
#[derive(Debug, Clone)]
pub struct TecConfig {
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
}

///Template of readout: <Tsetpoint P I D Tmin Tmax Tmeasured OC PWM>
#[derive(Debug, Clone)]
pub struct TecReadout {
    ///[°C] – target temperature value. The device checks output temperature via
    ///thermistor and manages output current to reach temperature setpoint.
    pub t_set: f32,
    ///PID proportional coefficient (in range from 0.0 to 20.0)
    pub P: f32,
    /// PID integral coefficient (in range from 0.0 to 20.0)
    pub I: f32,
    /// PID derivative coefficient (in range from 0.0 to 20.0)
    pub D: f32,
    /// [°C] - lower threshold which is checked by the driver (OC)
    pub t_min: f32,
    /// [°C] - upper threshold which is checked by the driver (OC)
    pub t_max: f32,
    /// [°C] - instantaneous value of measured temperature
    pub t_measured: f32,
    /// OC - status of the open collector (0 – disconnected from GND or 1 – connected to GND)
    pub OC: bool,
    ///[%] - Negative values indicate cooling phase and positive ones indicate heating phase. The absolute value of the parameter informs about intensity.
    pub PWM: f32,
}

///Single byte commands
///These are single character commands without additional parameters:
/// o – print single readout
/// R – turn ON cyclic print (print every second).
/// r – turn OFF cyclic print
/// A - switch ON TEC supply
/// a - switch OFF TEC supply
pub struct TecController {
    port: TTYPort,
}

impl TecController {
    pub fn new(port_name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let port = serialport::new(port_name, 38400)
            .timeout(Duration::from_millis(1000))
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .open_native()?;

        Ok(TecController { port })
    }

    /// Send a command to the device and read acknowledgment response.
    /// All commands return an acknowledgment in the format `<command>`.
    /// Logs sent commands and received responses for debugging.
    fn send_command(&mut self, command: &str) -> Result<String, Box<dyn std::error::Error>> {
        debug!("Sending command: '{}'", command);
        self.port.write_all(command.as_bytes())?;
        self.port.flush()?;

        let mut reader = BufReader::new(&mut self.port);
        let mut response = String::new();

        match reader.read_line(&mut response) {
            Ok(0) => return Err("EOF - no response received".into()),
            Ok(_) => {
                let trimmed_response = response.trim();
                debug!("Received acknowledgment: '{}'", trimmed_response);

                // Validate acknowledgment format
                let expected_ack = format!("<{}>", command);
                if trimmed_response != expected_ack {
                    eprintln!(
                        "Warning: Unexpected acknowledgment format. Expected '{}', got '{}'",
                        expected_ack, trimmed_response
                    );
                }

                Ok(trimmed_response.to_string())
            }
            Err(e) => {
                eprintln!("Error reading from serial port: {}", e);
                Err(e.into())
            }
        }
    }

    /// Get a single temperature and status readout from the device (single byte command 'o').
    /// This command returns acknowledgment followed by data line with format:
    /// `Tz=+25.00 P= 5.00 I= 2.00 D= 1.00 T=  0...+50 Tr=+25.01 OC=0 PW=+ 25`
    pub fn get_single_readout(&mut self) -> Result<TecReadout, Box<dyn std::error::Error>> {
        // Send command and get acknowledgment
        let _ack = self.send_command("o")?;

        // Read the actual data line
        let mut reader = BufReader::new(&mut self.port);
        let mut data_response = String::new();

        match reader.read_line(&mut data_response) {
            Ok(0) => return Err("EOF - no data response received".into()),
            Ok(_) => {
                let trimmed_data = data_response.trim();
                debug!("Received data: '{}'", trimmed_data);
                self.parse_readout(trimmed_data)
            }
            Err(e) => {
                eprintln!("Error reading data from serial port: {}", e);
                Err(e.into())
            }
        }
    }

    /// Set the TEC controller configuration parameters.
    /// Command format: `<Tsetpoint P I D Tmin Tmax>`
    /// Returns acknowledgment containing the configuration that was set.
    /// Validates that the returned configuration matches what was sent.
    pub fn set_configuration(
        &mut self,
        config: &TecConfig,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let command = format!(
            "<{} {} {} {} {} {}>",
            config.t_set, config.p, config.i, config.d, config.t_min, config.t_max
        );

        debug!("Sending configuration: '{}'", command);
        self.port.write_all(command.as_bytes())?;
        self.port.flush()?;
        // self.write_str(&command)?;

        // Configuration commands return a complex acknowledgment with config data
        let mut reader = BufReader::new(&mut self.port);
        let mut response = String::new();

        match reader.read_line(&mut response) {
            Ok(0) => return Err("EOF - no configuration response received".into()),
            Ok(_) => {
                let trimmed_response = response.trim();
                debug!(
                    "Received configuration acknowledgment: '{}'",
                    trimmed_response
                );

                // Try to parse the response as a configuration acknowledgment
                // Expected format should be similar to the command we sent
                if trimmed_response.starts_with('<') && trimmed_response.ends_with('>') {
                    // Parse the values and validate they match what we sent
                    if let Ok(returned_config) = self.parse_config_acknowledgment(trimmed_response)
                    {
                        self.validate_config_match(config, &returned_config)?;
                        debug!("Configuration validated successfully");
                    } else {
                        eprintln!("Warning: Could not parse configuration acknowledgment");
                    }
                } else {
                    eprintln!(
                        "Warning: Unexpected configuration acknowledgment format: '{}'",
                        trimmed_response
                    );
                }

                Ok(trimmed_response.to_string())
            }
            Err(e) => {
                eprintln!("Error reading configuration response: {}", e);
                Err(e.into())
            }
        }
    }

    /// Enable the TEC supply
    pub fn enable(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        self.send_command("A")
    }

    /// Disable the TEC supply
    pub fn disable(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        self.send_command("a")
    }

    fn parse_readout(& self, response: &str) -> Result<TecReadout, Box<dyn std::error::Error>> {
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
            P,
            I,
            D,
            t_min,
            t_max,
            t_measured,
            OC,
            PWM,
        })
    }

    fn parse_config_acknowledgment(
        &self,
        response: &str,
    ) -> Result<TecConfig, Box<dyn std::error::Error>> {
        // Remove < and > brackets
        let content = response.trim_start_matches('<').trim_end_matches('>');
        let parts: Vec<&str> = content.split_whitespace().collect();

        if parts.len() != 6 {
            return Err(format!(
                "Invalid config format: expected 6 values, got {}",
                parts.len()
            )
            .into());
        }

        Ok(TecConfig {
            t_set: parts[0].parse()?,
            p: parts[1].parse()?,
            i: parts[2].parse()?,
            d: parts[3].parse()?,
            t_min: parts[4].parse()?,
            t_max: parts[5].parse()?,
        })
    }

    fn validate_config_match(
        &self,
        sent: &TecConfig,
        received: &TecConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let tolerance = 0.01f32; // Allow small floating point differences

        if (sent.t_set - received.t_set).abs() > tolerance {
            return Err(format!(
                "T_set mismatch: sent {}, received {}",
                sent.t_set, received.t_set
            )
            .into());
        }
        if (sent.p - received.p).abs() > tolerance {
            return Err(format!("P mismatch: sent {}, received {}", sent.p, received.p).into());
        }
        if (sent.i - received.i).abs() > tolerance {
            return Err(format!("I mismatch: sent {}, received {}", sent.i, received.i).into());
        }
        if (sent.d - received.d).abs() > tolerance {
            return Err(format!("D mismatch: sent {}, received {}", sent.d, received.d).into());
        }
        if (sent.t_min - received.t_min).abs() > tolerance {
            return Err(format!(
                "T_min mismatch: sent {}, received {}",
                sent.t_min, received.t_min
            )
            .into());
        }
        if (sent.t_max - received.t_max).abs() > tolerance {
            return Err(format!(
                "T_max mismatch: sent {}, received {}",
                sent.t_max, received.t_max
            )
            .into());
        }

        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    const TEST_PORT: &str = "/dev/serial0";

    #[test]
    fn test_controller_connection() {
        let controller = TecController::new(TEST_PORT);
        assert!(controller.is_ok(), "Failed to connect to {}", TEST_PORT);
    }

    #[test]
    fn test_get_single_readout() {
        let mut controller = TecController::new(TEST_PORT).expect("Failed to connect");

        let readout = controller.get_single_readout();
        assert!(
            readout.is_ok(),
            "Failed to get readout: {:?}",
            readout.err()
        );

        let readout = readout.unwrap();
        debug!("Readout: {:?}", readout);

        // Basic sanity checks
        //assert!(readout.P >= 0.0 && readout.P <= 20.0, "P coefficient out of range");
        //assert!(readout.I >= 0.0 && readout.I <= 20.0, "I coefficient out of range");
        //assert!(readout.D >= 0.0 && readout.D <= 20.0, "D coefficient out of range");
        //assert!(readout.PWM >= -100.0 && readout.PWM <= 100.0, "PWM out of expected range");
    }

    //#[test]
    //fn test_set_configuration() {
    //    let mut controller = TecController::new(TEST_PORT).expect("Failed to connect");
    //
    //    let config = TecConfig {
    //        T_set: 25.0,
    //        P: 5.0,
    //        I: 2.0,
    //        D: 1.0,
    //        T_min: 0.0,
    //        T_max: 50.0,
    //    };
    //
    //    let result = controller.set_configuration(&config);
    //    assert!(result.is_ok(), "Failed to set configuration: {:?}", result.err());
    //
    //    // Wait a moment for settings to take effect
    //    thread::sleep(Duration::from_millis(200));
    //
    //    // Verify configuration was applied by reading back
    //    let readout = controller.get_single_readout().expect("Failed to get readout after config");
    //    assert!((readout.T_set - config.T_set).abs() < 0.1, "T_set not applied correctly");
    //    assert!((readout.P - config.P).abs() < 0.1, "P coefficient not applied correctly");
    //    assert!((readout.I - config.I).abs() < 0.1, "I coefficient not applied correctly");
    //    assert!((readout.D - config.D).abs() < 0.1, "D coefficient not applied correctly");
    //}

    #[test]
    fn test_enable_disable_tec() {
        let mut controller = TecController::new(TEST_PORT).expect("Failed to connect");

        // Test enabling TEC
        let enable_result = controller.enable();
        assert!(
            enable_result.is_ok(),
            "Failed to enable TEC: {:?}",
            enable_result.err()
        );

        thread::sleep(Duration::from_millis(100));

        // Test disabling TEC
        let disable_result = controller.disable();
        assert!(
            disable_result.is_ok(),
            "Failed to disable TEC: {:?}",
            disable_result.err()
        );
    }

    #[test]
    fn test_multiple_readouts() {
        let mut controller = TecController::new(TEST_PORT).expect("Failed to connect");

        // Take multiple readouts to ensure stability
        for i in 0..5 {
            let readout = controller.get_single_readout();
            assert!(
                readout.is_ok(),
                "Failed on readout {}: {:?}",
                i,
                readout.err()
            );

            thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn test_cyclic_print_control() {
        let mut controller = TecController::new(TEST_PORT).expect("Failed to connect");

        // Test enabling cyclic print
        let enable_result = controller.enable_cyclic_print();
        assert!(
            enable_result.is_ok(),
            "Failed to enable cyclic print: {:?}",
            enable_result.err()
        );

        thread::sleep(Duration::from_millis(100));

        // Test disabling cyclic print
        let disable_result = controller.disable_cyclic_print();
        assert!(
            disable_result.is_ok(),
            "Failed to disable cyclic print: {:?}",
            disable_result.err()
        );
    }
}
