#[derive(Debug, Default)]
pub struct LogAccumulator {
    pub records: std::sync::Mutex<Vec<(log::Level, String)>>,
}

impl log::Log for LogAccumulator {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        self.records
            .lock()
            .unwrap()
            .push((record.level(), record.args().to_string()));
    }

    fn flush(&self) {}
}
