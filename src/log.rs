use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use colored::Colorize as _;
use std::collections::HashSet;
use std::io::Write as _;
use std::ops::Deref;
use std::ops::DerefMut as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use tracing_subscriber::layer::SubscriberExt as _;

/// Keeps the tracing framework configured to store both log and trace events.
pub struct GlobalTraceLogger {
    pub log_writer: ArcMutexWriter<std::fs::File>,
    pub chrome_writer: ArcMutexWriter<std::io::BufWriter<std::fs::File>>,
    chrome_guard: crate::tracing_chrome::FlushGuard,
}

pub struct ArcMutexWriter<T>(std::sync::Arc<std::sync::Mutex<DelayedWriter<T>>>);

impl<T> ArcMutexWriter<T> {
    pub(crate) fn new(inner: DelayedWriter<T>) -> Self {
        ArcMutexWriter(std::sync::Arc::new(std::sync::Mutex::new(inner)))
    }
}

impl<T> Clone for ArcMutexWriter<T> {
    fn clone(&self) -> Self {
        ArcMutexWriter(self.0.clone())
    }
}

impl<T> Deref for ArcMutexWriter<T> {
    type Target = std::sync::Mutex<DelayedWriter<T>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: std::io::Write> std::io::Write for ArcMutexWriter<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}

impl<'writer, T> tracing_subscriber::fmt::MakeWriter<'writer> for ArcMutexWriter<T>
where
    T: std::io::Write + 'writer,
{
    type Writer = tracing_subscriber::fmt::writer::MutexGuardWriter<'writer, DelayedWriter<T>>;

    /// Creates a writer that has already locked the mutex and returns the
    /// guard.
    fn make_writer(&'writer self) -> Self::Writer {
        self.0.deref().make_writer()
    }
}

pub enum DelayedWriter<T> {
    /// Buffer in memory.
    Buffered(Vec<u8>),
    /// Output to a writer.
    Writer(T),
}

impl<T: std::io::Write> DelayedWriter<T> {
    /// Instead of buffering, write the buffered and future data to `writer`.
    pub fn set_writer(&mut self, mut writer: T) -> Result<()> {
        match self {
            DelayedWriter::Buffered(buffer) => {
                // Write the buffered log to the new writer.
                writer.write_all(buffer)?;
            }
            DelayedWriter::Writer(old_writer) => {
                old_writer.flush()?;
            }
        }
        *self = DelayedWriter::Writer(writer);
        Ok(())
    }
}

impl<T: std::io::Write> std::io::Write for DelayedWriter<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            DelayedWriter::Buffered(buffer) => {
                buffer.extend_from_slice(buf);
                Ok(buf.len())
            }
            DelayedWriter::Writer(writer) => writer.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            DelayedWriter::Buffered(_) => Ok(()),
            DelayedWriter::Writer(writer) => writer.flush(),
        }
    }
}

struct LogMultiplexer {
    pub backends: Vec<Box<dyn log::Log>>,
}

impl log::Log for LogMultiplexer {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.backends
            .iter()
            .any(|backend| backend.enabled(metadata))
    }

    fn log(&self, record: &log::Record) {
        println!("FRME {}: {}", record.level(), record.args());
        for backend in &self.backends {
            backend.log(record);
        }
    }

    fn flush(&self) {
        for backend in &self.backends {
            backend.flush();
        }
    }
}

impl GlobalTraceLogger {
    pub fn init() -> Result<Self> {
        // Convert log messages to tracing events, in case any dependency is
        // using the log framework.
        let log_to_trace = tracing_log::LogTracer::new();
        let log_to_stderr = env_logger::builder()
            // .filter_level(log::LevelFilter::Info)
            .parse_default_env()
            .format_file(false)
            .format_line_number(false)
            .format_module_path(false)
            .format_source_path(false)
            .format_target(false)
            .format_timestamp(None)
            .build();
        log::set_max_level(log::LevelFilter::Trace);
        let multiplex_logger = LogMultiplexer {
            backends: vec![Box::new(log_to_trace), Box::new(log_to_stderr)],
        };
        log::set_boxed_logger(Box::new(multiplex_logger))?;

        let log_writer = ArcMutexWriter::new(DelayedWriter::<std::fs::File>::Buffered(Vec::new()));
        let log_layer = tracing_subscriber::fmt::layer()
            .with_writer(log_writer.clone())
            .with_file(false)
            .with_line_number(false)
            .without_time()
            // Disable ANSI colors in the log file.
            .with_ansi(false)
            // The only target is "git_toprepo", so it is not useful.
            .with_target(false);

        let chrome_writer = ArcMutexWriter::new(DelayedWriter::Buffered(Vec::new()));
        let (chrome_layer, chrome_guard) = crate::tracing_chrome::ChromeLayerBuilder::new()
            .writer(chrome_writer.clone())
            .include_args(true)
            .include_locations(false)
            .build();

        let subscriber = tracing_subscriber::Registry::default()
            .with(log_layer)
            .with(chrome_layer);
        tracing::subscriber::set_global_default(subscriber).expect("set global subscriber");

        Ok(Self {
            log_writer,
            chrome_writer,
            chrome_guard,
        })
    }

    pub fn write_to_git_dir(&mut self, git_dir: &Path) -> Result<()> {
        let toprepo_dir = git_dir.join("toprepo");
        std::fs::create_dir_all(&toprepo_dir)?;
        // Unbuffered writes to log file.
        let log_path = toprepo_dir.join("events.log");
        let log_file = std::fs::File::create(&log_path)?;
        self.log_writer
            .lock()
            .unwrap()
            .set_writer(log_file)
            .with_context(|| format!("Failed to set log writer to {}", log_path.display()))?;
        // Buffered writes to Chrome trace file.
        let chrome_path = toprepo_dir.join("trace.json");
        let chrome_file = std::fs::File::create(&chrome_path)?;
        let chrome_file = std::io::BufWriter::new(chrome_file);
        self.chrome_writer
            .lock()
            .unwrap()
            .set_writer(chrome_file)
            .with_context(|| format!("Failed to set trace writer to {}", log_path.display()))?;
        Ok(())
    }

    /// Prints the current in memory log to `stderr` or flushes it to the log file.
    pub fn finalize(self) -> Result<()> {
        match self.log_writer.lock().unwrap().deref_mut() {
            DelayedWriter::Buffered(buffer) => {
                // Print the buffered log to stderr.
                eprint!("{}", String::from_utf8_lossy(buffer));
            }
            DelayedWriter::Writer(writer) => {
                // Flush the file to ensure all events are written.
                writer.flush()?;
            }
        }
        self.chrome_guard.flush();
        Ok(())
    }
}

pub fn eprint_warning(msg: &str) {
    eprintln!("{}: {msg}", "WARNING".yellow().bold());
}

pub fn log_task_to_stderr<F, T>(
    error_counter: Arc<AtomicUsize>,
    log_config: &mut crate::config::LogConfig,
    task: F,
) -> Result<T>
where
    F: FnOnce(Logger, indicatif::MultiProgress) -> Result<T>,
{
    let progress = indicatif::MultiProgress::new();
    // TODO: 2025-02-27 The rate limiter seems to be a bit broken. 1 Hz is
    // not smooth, default 20 Hz hogs the CPU, 2 Hz is a good compromise as
    // the result is actually much higher anyway.
    progress.set_draw_target(indicatif::ProgressDrawTarget::stderr_with_hz(2));

    let progress_clone = progress.clone();
    let ignore_warnings = log_config
        .ignore_warnings
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let log_receiver = LogReceiver::new(error_counter, ignore_warnings, move |msg| {
        progress_clone.suspend(|| eprintln!("{msg}"));
    });
    let result = task(log_receiver.get_logger(), progress.clone());
    if let Err(err) = &result {
        log_receiver.get_logger().error(format!("{err:#}"));
    }

    let log_result = log_receiver.join();
    log_config.reported_errors = log_result.reported_errors.clone();
    log_config.reported_warnings = log_result.reported_warnings.clone();
    log_result.print_to_stderr();
    if !log_result.is_success() {
        bail!("Task failed");
    }
    progress.clear()?;
    Ok(result.unwrap())
}

pub enum LogLevel {
    Error,
    Warning,
}

/// Because we cannot change the git history, bad data need to be gracefully
/// handled as warnings. These kind of problems are reported through this
/// logger.
#[derive(Clone)]
pub struct Logger {
    context: String,
    sender: std::sync::mpsc::Sender<LogTask>,
}

impl Logger {
    fn new(sender: std::sync::mpsc::Sender<LogTask>) -> Self {
        Logger {
            context: String::new(),
            sender,
        }
    }

    pub fn with_context(&self, context: &str) -> Self {
        let full_context = format!("{}: {context}", self.context);
        Logger {
            context: full_context,
            sender: self.sender.clone(),
        }
    }

    fn log(&self, level: LogLevel, msg: String) {
        self.sender
            .send(LogTask::Log {
                level,
                context: self.context.clone(),
                message: msg,
            })
            .expect("receiver never closed");
    }

    pub fn warning(&self, msg: String) {
        self.log(LogLevel::Warning, msg);
    }

    pub fn error(&self, msg: String) {
        self.log(LogLevel::Error, msg);
    }
}

#[derive(Clone)]
pub enum ErrorMode {
    /// Print errors when they arise but continue processing.
    KeepGoing,
    /// Fail fast on the first error, setting the `AtomicBool` to `true`.
    /// In this mode, the error is printed after all threads have stopped.
    FailFast,
}

impl ErrorMode {
    pub fn from_keep_going_flag(keep_going: bool) -> Self {
        if keep_going {
            ErrorMode::KeepGoing
        } else {
            ErrorMode::FailFast
        }
    }
}

#[derive(Clone)]
pub struct ErrorObserver {
    /// Number of errors reported.
    pub counter: Arc<AtomicUsize>,
    pub strategy: ErrorMode,
}

impl ErrorObserver {
    pub fn new(strategy: ErrorMode) -> Self {
        ErrorObserver {
            counter: Arc::new(AtomicUsize::new(0)),
            strategy,
        }
    }

    /// Returns `true` if the strategy is `FailFast` and the processing
    /// should be interrupted.
    pub fn should_interrupt(&self) -> bool {
        match self.strategy {
            ErrorMode::KeepGoing => false,
            ErrorMode::FailFast => self.has_got_errors(),
        }
    }

    /// Returns `true` if at least one error has been reported.
    pub fn has_got_errors(&self) -> bool {
        self.counter.load(std::sync::atomic::Ordering::Relaxed) > 0
    }
}

#[derive(Clone, Debug)]
pub struct LogResult {
    pub reported_errors: Vec<String>,
    pub reported_warnings: Vec<String>,
}

impl LogResult {
    /// Number of errors reported.
    pub fn error_count(&self) -> usize {
        self.reported_errors.len()
    }

    /// Number of warnings reported.
    pub fn warning_count(&self) -> usize {
        self.reported_warnings.len()
    }

    /// Print the number of errors and warnings to `stderr`.
    pub fn print_to_stderr(&self) {
        let error_str = if self.error_count() == 1 {
            "error"
        } else {
            "errors"
        };
        let warning_str = if self.warning_count() == 1 {
            "warning"
        } else {
            "warnings"
        };
        match (self.error_count(), self.warning_count()) {
            (0, 0) => (),
            (0, wcnt) => eprintln!("{}", format!("Found {wcnt} {warning_str}").yellow()),
            (ecnt, 0) => eprintln!("{}", format!("Failed due to {ecnt} {error_str}").red()),
            (ecnt, wcnt) => eprintln!(
                "{} and {}",
                format!("Failed due to {ecnt} {error_str}").red(),
                format!("{wcnt} {warning_str}").yellow()
            ),
        }
    }

    /// Checks that no errors have been reported.
    pub fn is_success(&self) -> bool {
        self.reported_errors.is_empty()
    }

    /// Returns an error if at least one error has been reported.
    pub fn check(&self) -> Result<()> {
        if !self.is_success() {
            anyhow::bail!(
                "{} errors and {} warnings",
                self.error_count(),
                self.warning_count()
            );
        }
        Ok(())
    }
}

enum LogTask {
    /// Log a message.
    Log {
        level: LogLevel,
        context: String,
        message: String,
    },
    /// Return a copy of the current result into the `tx` channel.
    PeekResult { tx: oneshot::Sender<LogResult> },
}

pub struct LogReceiver {
    logger_thread: std::thread::JoinHandle<LogResult>,
    logger: Logger,
}

impl LogReceiver {
    pub fn new<F>(
        error_counter: Arc<AtomicUsize>,
        ignore_warnings: HashSet<String>,
        draw_callback: F,
    ) -> Self
    where
        F: Fn(&str) + Send + 'static,
    {
        let mut seen_warnings: HashSet<String> =
            HashSet::from_iter(ignore_warnings.iter().cloned());
        let (tx, rx) = std::sync::mpsc::channel::<LogTask>();
        let logger_thread = std::thread::Builder::new()
            .name("logger".into())
            .spawn(move || {
                let mut seen_errors = HashSet::new();
                let mut result = LogResult {
                    reported_errors: Vec::new(),
                    reported_warnings: Vec::new(),
                };
                rx.iter().for_each(|log_task| match log_task {
                    LogTask::Log {
                        level,
                        context,
                        message,
                    } => {
                        let context_and_msg = &format!("{context}: {message}")[2..];
                        let level_str = match level {
                            LogLevel::Error => {
                                if !seen_errors.insert(context_and_msg.to_string()) {
                                    return;
                                }
                                result.reported_errors.push(context_and_msg.to_string());
                                error_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                "ERROR:".red()
                            }
                            LogLevel::Warning => {
                                if !seen_warnings.insert(context_and_msg.to_string()) {
                                    return;
                                }
                                result.reported_warnings.push(context_and_msg.to_string());
                                "WARNING:".yellow()
                            }
                        };
                        draw_callback(&format!("{level_str} {context_and_msg}"));
                    }
                    LogTask::PeekResult { tx } => {
                        let _ignore_error = tx.send(result.clone());
                    }
                });
                result
            })
            .expect("spawn thread");
        let logger = Logger::new(tx);
        LogReceiver {
            logger_thread,
            logger,
        }
    }

    /// Get the current result of the logger thread after pending log tasks have
    /// been processed.
    pub fn peek_result(&self) -> LogResult {
        let (tx, rx) = oneshot::channel();
        self.logger
            .sender
            .send(LogTask::PeekResult { tx })
            .expect("logger thread never panics, so receiver never closes");
        rx.recv()
            .expect("logger thread never panics and join cannot run concurrently")
    }

    pub fn get_logger(&self) -> Logger {
        self.logger.clone()
    }

    pub fn join(self) -> LogResult {
        drop(self.logger);
        // When all the loggers have been dropped, the channel will be closed
        // and the receiver exit when the channel is exhausted.
        self.logger_thread
            .join()
            .expect("logger thread never panics")
    }
}

/// Accumulates all messages to be printed into a buffer.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    pub struct LogAccumulator {
        log_receiver: LogReceiver,
        pub all_messages: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl LogAccumulator {
        pub fn new(error_counter: Arc<AtomicUsize>) -> (Self, Logger) {
            let all_messages = Arc::new(std::sync::Mutex::new(Vec::new()));
            let all_messages_clone = all_messages.clone();
            let log_receiver = LogReceiver::new(error_counter, HashSet::new(), move |msg| {
                all_messages_clone.lock().unwrap().push(msg.to_string());
            });
            let logger = log_receiver.get_logger();
            let ret = LogAccumulator {
                log_receiver,
                all_messages,
            };
            (ret, logger)
        }

        pub fn join_no_warnings(self) -> Result<()> {
            let log_result = self.log_receiver.join();
            let all_messages = std::mem::take(self.all_messages.lock().unwrap().deref_mut());
            if log_result.error_count() != 0 || log_result.warning_count() != 0 {
                let messages = all_messages.join("\n");
                anyhow::bail!(
                    "{} errors and {} warnings:\n{}",
                    log_result.error_count(),
                    log_result.warning_count(),
                    messages
                );
            }
            Ok(())
        }
    }
}
