use derive_builder::Builder;
use log::{debug, info, trace, warn};
use serialport::TTYPort;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct TecConfig {
    pub t_set: f32,
    pub p: f32,
    pub i: f32,
    pub d: f32,
    pub t_min: f32,
    pub t_max: f32,
}

impl Default for TecConfig {
    fn default() -> TecConfig {
        
        TecConfig {
            t_set: 20.0,
            p: 5.50,
            i: 2.50,
            d: 0.5,
            t_min: 0.0,
            t_max: 35.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TecReadout {
    pub t_set: f32,
    pub p: f32,
    pub i: f32,
    pub d: f32,
    pub t_min: f32,
    pub t_max: f32,
    pub t_measured: f32,
    pub oc: bool,
    pub pwm: f32,
}

pub struct TecController {
    port: TTYPort,
    pub current_config: TecConfig,
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
        let mut tec = TecController {
            port,
            current_config: TecConfig::default(),
        };
        TecController::disable(&mut tec);
        TecController::set_configuration(&mut tec, &TecConfig::default())?;
        Ok(tec)
    }

    pub fn set_t(&mut self, temp: f32) {
            let new_cfg = TecConfig {
                t_set: temp,
                ..self.current_config
            };
            self.set_configuration(&new_cfg).ok();
        
    }

    /// Clear any pending data in the input buffer
    fn clear_input_buffer(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut discard = vec![0u8; 1024];
        loop {
            match self.port.read(&mut discard) {
                Ok(0) => break,
                Ok(n) => {},
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Read a response line from the serial port, handling non-UTF-8 gracefully
    fn read_response(&mut self, timeout_ms: u64) -> Result<String, Box<dyn std::error::Error>> {
        let mut buffer = Vec::new();
        let start_time = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        
        loop {
            let mut byte = [0u8; 1];
            match self.port.read(&mut byte) {
                Ok(n) if n > 0 => {
                    buffer.push(byte[0]);
                    
                    // Check for line ending
                    if byte[0] == b'\n' {
                        break;
                    }
                    // If we get CR, check for LF
                    if byte[0] == b'\r' {
                        // Try to read next byte (might be LF)
                        let mut next_byte = [0u8; 1];
                        match self.port.read(&mut next_byte) {
                            Ok(1) if next_byte[0] == b'\n' => {
                                buffer.push(next_byte[0]);
                            }
                            Ok(1) => {
                                // Not a LF, but we got something - it's the start of next line
                                // Put it back would be ideal, but we can't, so just break
                            }
                            _ => {}
                        }
                        break;
                    }
                }
                Ok(_) => {
                    if start_time.elapsed() > timeout {
                        return Err("Timeout waiting for response".into());
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    if start_time.elapsed() > timeout {
                        return Err("Timeout waiting for response".into());
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(e.into()),
            }
        }
        
        // Convert to string, replacing invalid UTF-8 sequences
        let response = String::from_utf8_lossy(&buffer);
        let trimmed = response.trim().to_string();
        
        debug!("Decoded response: '{}'", trimmed);
        
        Ok(trimmed)
    }

    /// Send a simple command and read acknowledgment
    fn send_command(&mut self, command: &str) -> Result<String, Box<dyn std::error::Error>> {
        debug!("Sending command: '{}'", command);
        
        // Clear any stale data
        self.clear_input_buffer()?;
        
        // Send command
        self.port.write_all(command.as_bytes())?;
        self.port.flush()?;
        
        // Wait a bit for device to process
        thread::sleep(Duration::from_millis(50));
        
        // Read response
        let response = self.read_response(1000)?;
        
        // Validate acknowledgment format
        let expected_ack = format!("<{}>", command);
        if response != expected_ack {
            warn!(
                "Unexpected acknowledgment format. Expected '{}', got '{}'",
                expected_ack, response
            );
        }
        
        Ok(response)
    }

    pub fn get_single_readout(&mut self) -> Result<TecReadout, Box<dyn std::error::Error>> {
        // Send command and get acknowledgment
        let _ack = self.send_command("o")?;
        
        // Read the actual data line
        let data_response = self.read_response(1000)?;
        debug!("Received data: '{}'", data_response);
        
        self.parse_readout(&data_response)
    }

    pub fn set_configuration(
        &mut self,
        config: &TecConfig,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Clear any pending data
        self.clear_input_buffer()?;
       /// <10 15 2 1 0 35> 
        let command = format!(
            "<{} {} {} {} {} {}>",
            config.t_set, config.p, config.i, config.d, config.t_min, config.t_max
        );
        
        debug!("Sending configuration: '{}'", command);
        self.port.write_all(command.as_bytes())?;
        self.port.flush()?;
        
        // Give device more time to process configuration
        thread::sleep(Duration::from_millis(150));
        
        // Read response
        let response = self.read_response(2000)?;
        debug!("Received configuration acknowledgment: '{}'", response);
        
        // Parse and validate
        match self.parse_config_acknowledgment(&response) {
            Ok(returned_config) => {
                self.current_config = returned_config;
                debug!("Configuration validated successfully");
                Ok(response)
            }
            Err(e) => {
                warn!("Could not parse configuration acknowledgment: {}", e);
                Err(format!("Parse error: {} - Response was: '{}'", e, response).into())
            }
        }
    }

    pub fn enable(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        self.send_command("A")
    }

    pub fn disable(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        self.send_command("a")
    }

    fn parse_readout(&self, response: &str) -> Result<TecReadout, Box<dyn std::error::Error>> {
        let sections: Vec<&str> = response.trim().split('=').collect();
        if sections.len() < 9 {
            return Err(format!("Not enough sections in response. Got {} sections, expected at least 9", sections.len()).into());
        }

        let parse_section = |section: &str| -> Result<f32, Box<dyn std::error::Error>> {
            let filtered: String = section
                .chars()
                .filter(|c| !c.is_alphabetic() && !c.is_whitespace())
                .collect();
            Ok(filtered.parse::<f32>()?)
        };

        let t_set = parse_section(sections[1])?;
        let p = parse_section(sections[2])?;
        let i = parse_section(sections[3])?;
        let d = parse_section(sections[4])?;

        // Parse temperature range
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
        let oc = match oc_filtered.parse::<u8>()? {
            0 => false,
            1 => true,
            _ => return Err("Invalid OC value".into()),
        };

        let pwm = parse_section(sections[8])?;

        Ok(TecReadout {
            t_set,
            p,
            i,
            d,
            t_min,
            t_max,
            t_measured,
            oc,
            pwm,
        })
    }

    fn parse_config_acknowledgment(
        &self,
        response: &str,
    ) -> Result<TecConfig, Box<dyn std::error::Error>> {
        let mut config = TecConfig::default();
        
        for part in response.split_whitespace() {
            if let Some((key, value)) = part.split_once('=') {
                match value.parse::<f32>() {
                    Ok(val) => {
                        match key {
                            "eTzc" => config.t_set = val,
                            "eKp" => config.p = val,
                            "eKi" => config.i = val,
                            "eKd" => config.d = val,
                            "eTmin" => config.t_min = val,
                            "eTmax" => config.t_max = val,
                            _ => debug!("Unknown config parameter: {}", key),
                        }
                    }
                    Err(e) => {
                        warn!("Failed to parse value '{}' for key '{}': {}", value, key, e);
                    }
                }
            }
        }
        
        Ok(config)
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
    }

    #[test]
    fn test_enable_disable_tec() {
        let mut controller = TecController::new(TEST_PORT).expect("Failed to connect");
        
        let enable_result = controller.enable();
        assert!(
            enable_result.is_ok(),
            "Failed to enable TEC: {:?}",
            enable_result.err()
        );
        thread::sleep(Duration::from_millis(100));
        
        let disable_result = controller.disable();
        assert!(
            disable_result.is_ok(),
            "Failed to disable TEC: {:?}",
            disable_result.err()
        );
    }

    #[test]
    fn test_set_configuration() {
        let mut controller = TecController::new(TEST_PORT).expect("Failed to connect");
  
        
        let config_result = controller.set_configuration(&TecConfig::default());
        assert!(
            config_result.is_ok(),
            "Failed to set configuration: {:?}",
            config_result.err()
        );
    }

    #[test]
    fn test_set_configuration_2() {
        let mut controller = TecController::new(TEST_PORT).expect("Failed to connect");
        
  
        let config_result = controller.set_configuration(&TecConfig::default());
        assert!(
            config_result.is_ok(),
            "Failed to set configuration: {:?}",
            config_result.err()
        );
        
        let readout = controller.get_single_readout();
        assert!(
            readout.is_ok(),
            "Failed to get readout: {:?}",
            readout.err()
        );
        let readout = readout.unwrap();
        debug!("Readout: {:?}", readout);
    }
}