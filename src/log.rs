use anyhow::Result;
use anyhow::bail;
use colored::Colorize as _;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

pub fn eprint_log(level: LogLevel, msg: &str) {
    eprintln!("{}: {msg}", level.to_colored_str());
}

pub enum LogLevel {
    Error,
    Warning,
    Info,
    Trace,
}

impl LogLevel {
    pub fn as_str(&self) -> &str {
        match self {
            LogLevel::Error => "ERROR",
            LogLevel::Warning => "WARNING",
            LogLevel::Info => "INFO",
            LogLevel::Trace => "TRACE",
        }
    }

    pub fn to_colored_str(&self) -> colored::ColoredString {
        match self {
            LogLevel::Error => self.as_str().red().bold(),
            LogLevel::Warning => self.as_str().yellow().bold(),
            LogLevel::Info => self.as_str().green(),
            LogLevel::Trace => self.as_str().blue(),
        }
    }
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

    /// Creates a new logger that prints to stderr.
    pub fn new_to_stderr<F, T>(task: F) -> Result<T>
    where
        F: FnOnce(Logger) -> Result<T>,
    {
        let log_receiver = LogReceiver::new(eprint_log);
        let result = task(log_receiver.get_logger());
        log_receiver.join();
        result
    }

    /// Wraps the current logger with an `indicatif::MultiProgress` instance
    /// and makes sure the progress bar does not interfere with the
    /// logging output.
    ///
    /// TODO: This implementation creates an extra thread for logging,
    /// which is not ideal.
    pub fn with_progress<F, T>(&self, task: F) -> Result<T>
    where
        F: FnOnce(Logger, indicatif::MultiProgress) -> Result<T>,
    {
        let progress = indicatif::MultiProgress::new();
        // TODO: 2025-02-27 The rate limiter seems to be a bit broken. 1 Hz is
        // not smooth, default 20 Hz hogs the CPU, 2 Hz is a good compromise as
        // the result is actually much higher anyway.
        progress.set_draw_target(indicatif::ProgressDrawTarget::stderr_with_hz(2));

        let progress_clone = progress.clone();
        let self_clone = self.clone();
        let log_receiver = LogReceiver::new(move |level, msg| {
            progress_clone.suspend(|| self_clone.log(level, msg.to_owned()));
        });
        let result = task(log_receiver.get_logger(), progress.clone());
        log_receiver.join();
        progress.clear()?;
        result
    }

    pub fn with_context(&self, context: &str) -> Self {
        let full_context = format!("{}: {context}", self.context);
        Logger {
            context: full_context,
            sender: self.sender.clone(),
        }
    }

    pub fn log(&self, level: LogLevel, msg: String) {
        self.sender
            .send(LogTask {
                level,
                context: self.context.clone(),
                message: msg,
            })
            .expect("receiver never closed");
    }

    pub fn error(&self, msg: String) {
        self.log(LogLevel::Error, msg);
    }

    pub fn warning(&self, msg: String) {
        self.log(LogLevel::Warning, msg);
    }

    pub fn info(&self, msg: String) {
        self.log(LogLevel::Info, msg);
    }

    pub fn trace(&self, msg: String) {
        self.log(LogLevel::Trace, msg);
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

pub enum InterruptedError {
    Normal(anyhow::Error),
    Interrupted,
}

impl InterruptedError {
    pub fn get_normal(self) -> InterruptedResult<anyhow::Error> {
        match self {
            InterruptedError::Normal(err) => Ok(err),
            InterruptedError::Interrupted => Err(InterruptedError::Interrupted),
        }
    }
}

impl From<anyhow::Error> for InterruptedError {
    fn from(err: anyhow::Error) -> Self {
        InterruptedError::Normal(err)
    }
}

pub type InterruptedResult<T> = Result<T, InterruptedError>;

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

    /// Returns an error if at least one error has occurred.
    pub fn get_result<T>(&self, default: T) -> Result<T> {
        let error_count = self.counter.load(std::sync::atomic::Ordering::Relaxed);
        match error_count {
            0 => Ok(default),
            1 => bail!("1 error reported"),
            n => bail!("{n} errors reported"),
        }
    }

    /// Write the error to the logger.
    pub fn consume_interrupted(&self, logger: &Logger, result: InterruptedResult<()>) {
        match result {
            Ok(_) => {}
            Err(InterruptedError::Interrupted) => {}
            Err(InterruptedError::Normal(err)) => {
                self.counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                logger.error(format!("{err:#}"));
            }
        }
    }

    /// Write the error to the logger.
    pub fn consume<T>(&self, logger: &Logger, result: Result<T>) -> Option<T> {
        result
            .inspect_err(|err| {
                self.counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                logger.error(format!("{err:#}"));
            })
            .ok()
    }

    /// Write the error to the logger if in keep-going mode and return the
    /// result. Return the error in fail-fast mode.
    pub fn maybe_consume(&self, logger: &Logger, result: Result<()>) -> Result<()> {
        match result {
            Ok(_) => Ok(()),
            Err(err) => {
                self.counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                match self.strategy {
                    ErrorMode::KeepGoing => {
                        logger.error(format!("{err:#}"));
                        Ok(())
                    }
                    ErrorMode::FailFast => Err(err),
                }
            }
        }
    }
}

struct LogTask {
    level: LogLevel,
    context: String,
    message: String,
}

pub struct LogReceiver {
    logger_thread: std::thread::JoinHandle<()>,
    logger: Logger,
}

impl LogReceiver {
    pub fn new<F>(draw_callback: F) -> Self
    where
        F: Fn(LogLevel, &str) + Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel::<LogTask>();
        let logger_thread = std::thread::Builder::new()
            .name("logger".into())
            .spawn(move || {
                rx.iter().for_each(|task| {
                    let context_and_msg = &format!("{}: {}", task.context, task.message)[2..];
                    draw_callback(task.level, context_and_msg);
                });
            })
            .expect("failed to spawn thread");
        let logger = Logger::new(tx);
        LogReceiver {
            logger_thread,
            logger,
        }
    }

    pub fn get_logger(&self) -> Logger {
        self.logger.clone()
    }

    pub fn join(self) {
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
    use itertools::Itertools as _;
    use std::ops::DerefMut as _;

    pub struct LogAccumulator {
        log_receiver: LogReceiver,
        pub all_messages: Arc<std::sync::Mutex<Vec<(LogLevel, String)>>>,
    }

    impl LogAccumulator {
        pub fn new() -> (Self, Logger) {
            let all_messages = Arc::new(std::sync::Mutex::new(Vec::new()));
            let all_messages_clone = all_messages.clone();
            let log_receiver = LogReceiver::new(move |level, msg| {
                all_messages_clone
                    .lock()
                    .unwrap()
                    .push((level, msg.to_string()));
            });
            let logger = log_receiver.get_logger();
            let ret = LogAccumulator {
                log_receiver,
                all_messages,
            };
            (ret, logger)
        }

        pub fn join(self) -> Vec<(LogLevel, String)> {
            self.log_receiver.join();
            std::mem::take(self.all_messages.lock().unwrap().deref_mut())
        }

        pub fn join_nothing_logged(self) -> Result<()> {
            let all_messages = self.join();
            if !all_messages.is_empty() {
                let concatenated_messages = all_messages
                    .iter()
                    .map(|(level, msg)| format!("{}: {msg}", level.as_str()))
                    .join("\n");
                anyhow::bail!(
                    "{} log messages:\n{}",
                    all_messages.len(),
                    concatenated_messages,
                );
            }
            Ok(())
        }
    }
}
