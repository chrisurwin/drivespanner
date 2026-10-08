use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::time::SystemTime;

struct SimpleLogger {
    file_mutex: Mutex<Option<std::fs::File>>,
}

impl log::Log for SimpleLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let elapsed = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let hours = (elapsed / 3600) % 24;
            let minutes = (elapsed / 60) % 60;
            let seconds = elapsed % 60;
            let line = format!("[{:02}:{:02}:{:02}] [{}] {}\n", hours, minutes, seconds, record.level(), record.args());
            let _ = std::io::stdout().write_all(line.as_bytes());

            if let Ok(mut lock) = self.file_mutex.lock() {
                if let Some(ref mut f) = *lock {
                    let _ = f.write_all(line.as_bytes());
                    let _ = f.flush();
                }
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut lock) = self.file_mutex.lock() {
            if let Some(ref mut f) = *lock {
                let _ = f.flush();
            }
        }
    }
}

pub fn init_logger() {
    let mut log_file = None;
    let log_dir = std::path::PathBuf::from("C:\\ProgramData\\DriveSpanner");
    let _ = fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("drivespanner.log");
    if let Ok(f) = OpenOptions::new().create(true).append(true).open(&log_path) {
        log_file = Some(f);
    }

    let logger = Box::leak(Box::new(SimpleLogger {
        file_mutex: Mutex::new(log_file),
    }));
    let _ = log::set_logger(logger);
    log::set_max_level(log::LevelFilter::Info);
}

