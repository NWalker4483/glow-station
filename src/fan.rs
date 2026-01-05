use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub struct Fan {
    pwm_chip: u32,
    pwm_channel: u32,
    period_ns: u32,
}

impl Fan {
    /// Create a new Fan controller
    /// 
    /// # Arguments
    /// * `pwm_chip` - PWM chip number (typically 0)
    /// * `pwm_channel` - PWM channel number (typically 0)
    /// * `frequency_hz` - Desired PWM frequency in Hz (e.g., 25000 for 25kHz)
    pub fn new(pwm_chip: u32, pwm_channel: u32, frequency_hz: u32) -> io::Result<Self> {
        let fan = Fan {
            pwm_chip,
            pwm_channel,
            period_ns: 1_000_000_000 / frequency_hz, // Convert Hz to nanoseconds
        };
        
        // // Export the PWM channel if not already exported
        // if !fan.is_exported() {
        //     fan.export()?;
        //     // Give the system time to create the sysfs files
        //     std::thread::sleep(std::time::Duration::from_millis(100));
        // }
        
        // // Set the period
        // fan.write_attribute("period", &fan.period_ns.to_string())?;
        
        // // Set initial duty cycle to 0 (fan off)
        // fan.write_attribute("duty_cycle", "0")?;
        
        // // Enable the PWM
        // fan.write_attribute("enable", "1")?;
        
        Ok(fan)
    }
    
    /// Set fan speed as a percentage (0-100)
    pub fn set_speed_percent(&self, percent: u8) -> io::Result<()> {
        let percent = percent.min(100); // Clamp to 100%
        let duty_cycle = (self.period_ns as u64 * percent as u64 / 100) as u32;
        self.write_attribute("duty_cycle", &duty_cycle.to_string())
    }
    
    /// Set fan speed with raw duty cycle value (0 to period_ns)
    pub fn set_duty_cycle(&self, duty_cycle_ns: u32) -> io::Result<()> {
        let duty_cycle = duty_cycle_ns.min(self.period_ns);
        self.write_attribute("duty_cycle", &duty_cycle.to_string())
    }
    
    /// Turn fan off
    pub fn off(&self) -> io::Result<()> {
        self.set_speed_percent(0)
    }
    
    /// Turn fan on at full speed
    pub fn on_full(&self) -> io::Result<()> {
        self.set_speed_percent(100)
    }
    
    /// Enable PWM output
    pub fn enable(&self) -> io::Result<()> {
        self.write_attribute("enable", "1")
    }
    
    /// Disable PWM output
    pub fn disable(&self) -> io::Result<()> {
        self.write_attribute("enable", "0")
    }
    
    // Helper methods
    
    fn pwm_path(&self) -> String {
        format!("/sys/class/pwm/pwmchip{}/pwm{}", self.pwm_chip, self.pwm_channel)
    }
    
    fn chip_path(&self) -> String {
        format!("/sys/class/pwm/pwmchip{}", self.pwm_chip)
    }
    
    fn is_exported(&self) -> bool {
        Path::new(&self.pwm_path()).exists()
    }
    
    fn export(&self) -> io::Result<()> {
        let export_path = format!("{}/export", self.chip_path());
        let mut file = OpenOptions::new().write(true).open(export_path)?;
        write!(file, "{}", self.pwm_channel)?;
        Ok(())
    }
    
    fn unexport(&self) -> io::Result<()> {
        if self.is_exported() {
            let unexport_path = format!("{}/unexport", self.chip_path());
            let mut file = OpenOptions::new().write(true).open(unexport_path)?;
            write!(file, "{}", self.pwm_channel)?;
        }
        Ok(())
    }
    
    fn write_attribute(&self, attribute: &str, value: &str) -> io::Result<()> {
        let path = format!("{}/{}", self.pwm_path(), attribute);
        let mut file = OpenOptions::new().write(true).open(&path)?;
        write!(file, "{}", value)?;
        Ok(())
    }
}

impl Drop for Fan {
    fn drop(&mut self) {
        // Clean shutdown: disable and unexport
        let _ = self.disable();
        let _ = self.unexport();
    }
}

// Example usage:
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    #[ignore] // Requires actual hardware
    fn test_fan_control() -> io::Result<()> {
        // Create fan controller at 25kHz
        let fan = Fan::new(0, 0, 25_000)?;
        
        // Test different speeds
        fan.set_speed_percent(25)?;
        std::thread::sleep(std::time::Duration::from_secs(2));
        
        fan.set_speed_percent(50)?;
        std::thread::sleep(std::time::Duration::from_secs(2));
        
        fan.set_speed_percent(75)?;
        std::thread::sleep(std::time::Duration::from_secs(2));
        
        fan.on_full()?;
        std::thread::sleep(std::time::Duration::from_secs(2));
        
        fan.off()?;
        
        Ok(())
    }
}

// Add:
// ```
// dtoverlay=pwm,pin=12,func=0
// # Export PWM0
// echo 0 > /sys/class/pwm/pwmchip0/export

// # Set 25kHz frequency
// echo 40000 > /sys/class/pwm/pwmchip0/pwm0/period

// # Set duty cycle
// echo 20000 > /sys/class/pwm/pwmchip0/pwm0/duty_cycle  # 50%

// # Enable
// echo 1 > /sys/class/pwm/pwmchip0/pwm0/enable

// # Test speeds
// echo 10000 > /sys/class/pwm/pwmchip0/pwm0/duty_cycle  # 25%
// echo 30000 > /sys/class/pwm/pwmchip0/pwm0/duty_cycle  # 75%
// echo 40000 > /sys/class/pwm/pwmchip0/pwm0/duty_cycle  # 100%