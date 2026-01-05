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

/// Camera controller for video recording
pub struct Camera {
    process: Option<Child>,
    video_path: String,
    pts_path: String,
}

impl Camera {
    pub fn new(experiment_dir: &str) -> Self {
        Camera {
            process: None,
            video_path: format!("{}/video.h264", experiment_dir),
            pts_path: format!("{}/timestamps.txt", experiment_dir),
        }
    }

    pub fn start(&mut self) -> std::io::Result<()> {
        println!("Starting video capture...");

        let process = Command::new("rpicam-vid")
            .args([
                "-o",
                &self.video_path,
                "-t",
                "0",
                "--save-pts",
                &self.pts_path,
                "--flush",
                "--nopreview",
                "--mode",
                "1920:1080:10:P",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        self.process = Some(process);
        Ok(())
    }

    pub fn stop(&mut self) -> std::io::Result<()> {
        if let Some(mut process) = self.process.take() {
            println!("Stopping video recording...");

            let pid = Pid::from_raw(process.id() as i32);
            println!("Sending SIGINT to camera process (PID: {})...", pid);

            // Wait briefly to ensure process is running
            thread::sleep(Duration::from_secs(1));

            // Send SIGINT to gracefully stop recording
            nix::sys::signal::kill(pid, Some(Signal::SIGINT))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

            println!("Waiting for camera process to terminate...");
            let status = process.wait()?;
            println!("Camera process exited with status: {}", status);
        }
        Ok(())
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
