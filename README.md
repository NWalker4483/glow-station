# Glow Station

add fan speed control 
add monitoring mode to tec struct 
add indicators
add button 

20 pixels per mm 
Disable Login over serial.
Enable Hardware Serial
sudo apt install libudev-dev


sudo nano /boot/config.txt
# Or: sudo nano /boot/firmware/config.txt
```

Add:
```
dtoverlay=pwm,pin=12,func=0
# Export PWM0
echo 0 > /sys/class/pwm/pwmchip0/export

# Set 25kHz frequency
echo 40000 > /sys/class/pwm/pwmchip0/pwm0/period

# Set duty cycle
echo 20000 > /sys/class/pwm/pwmchip0/pwm0/duty_cycle  # 50%

# Enable
echo 1 > /sys/class/pwm/pwmchip0/pwm0/enable

# Test speeds
echo 10000 > /sys/class/pwm/pwmchip0/pwm0/duty_cycle  # 25%
echo 30000 > /sys/class/pwm/pwmchip0/pwm0/duty_cycle  # 75%
echo 40000 > /sys/class/pwm/pwmchip0/pwm0/duty_cycle  # 100%