use std::time::SystemTime;

struct SimpleLogger;

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
            println!("[{:02}:{:02}:{:02}] [{}] {}", hours, minutes, seconds, record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

pub fn init_logger() {
    let logger = Box::leak(Box::new(SimpleLogger));
    let _ = log::set_logger(logger);
    log::set_max_level(log::LevelFilter::Info);
}
