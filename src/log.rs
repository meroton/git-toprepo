use anyhow::Result;
use anyhow::bail;
use colored::Colorize as _;
use std::collections::HashSet;
use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub fn log_task_to_stderr<F, T>(error_mode: ErrorMode, task: F) -> Result<T>
where
    F: FnOnce(Logger, indicatif::MultiProgress) -> Result<T>,
{
    let progress = indicatif::MultiProgress::new();
    // TODO: 2025-02-27 The rate limiter seems to be a bit broken. 1 Hz is
    // not smooth, default 20 Hz hogs the CPU, 2 Hz is a good compromise as
    // the result is actually much higher anyway.
    progress.set_draw_target(indicatif::ProgressDrawTarget::stderr_with_hz(2));

    let progress_clone = progress.clone();
    let log_receiver = LogReceiver::new(error_mode.clone(), move |msg| {
        progress_clone.suspend(|| eprintln!("{msg}"));
    });
    let result = task(log_receiver.get_logger(), progress.clone());
    if let Err(err) = &result {
        log_receiver.get_logger().error(format!("{err:#}"));
    }

    let log_result = log_receiver.join();
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
    FailFast(Arc<AtomicBool>),
}

impl ErrorMode {
    pub fn from_keep_going_flag(keep_going: bool) -> Self {
        if keep_going {
            ErrorMode::KeepGoing
        } else {
            ErrorMode::FailFast(Default::default())
        }
    }

    pub fn interrupted(&self) -> Arc<AtomicBool> {
        match self {
            ErrorMode::KeepGoing => Arc::new(AtomicBool::new(false)),
            ErrorMode::FailFast(interrupted) => interrupted.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogResult {
    pub error_count: usize,
    pub warning_count: usize,
}

impl LogResult {
    pub fn print_to_stderr(&self) {
        let error_str = if self.error_count == 1 {
            "error"
        } else {
            "errors"
        };
        let warning_str = if self.warning_count == 1 {
            "warning"
        } else {
            "warnings"
        };
        match (self.error_count, self.warning_count) {
            (0, 0) => (),
            (0, wcnt) => eprintln!("{}", format!("Found {} {}", wcnt, warning_str).yellow()),
            (ecnt, 0) => eprintln!("{}", format!("Failed due to {} {}", ecnt, error_str).red()),
            (ecnt, wcnt) => eprintln!(
                "{} and {}",
                format!("Failed due to {} {}", ecnt, error_str).red(),
                format!("{} {}", wcnt, warning_str).yellow()
            ),
        }
    }

    pub fn is_success(&self) -> bool {
        self.error_count == 0
    }

    pub fn check(&self) -> Result<()> {
        if !self.is_success() {
            anyhow::bail!(
                "{} errors and {} warnings",
                self.error_count,
                self.warning_count
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
    PeekResult {
        tx: std::sync::mpsc::Sender<LogResult>,
    },
}

pub struct LogReceiver {
    logger_thread: std::thread::JoinHandle<LogResult>,
    logger: Logger,
}

impl LogReceiver {
    /// Create a new `LogReceiver` that prints to `stderr`.
    pub fn new_stderr(error_mode: ErrorMode) -> Self {
        Self::new(error_mode, |msg| {
            eprintln!("{}", msg);
        })
    }

    pub fn new<F>(error_mode: ErrorMode, draw_callback: F) -> Self
    where
        F: Fn(&str) + Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel::<LogTask>();
        let logger_thread = std::thread::Builder::new()
            .name("logger".into())
            .spawn(move || {
                let mut seen_warnings = HashSet::new();
                let mut seen_errors = HashSet::new();
                let mut result = LogResult {
                    error_count: 0,
                    warning_count: 0,
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
                                if !seen_errors.insert(message) {
                                    return;
                                }
                                result.error_count += 1;
                                match error_mode {
                                    ErrorMode::KeepGoing => (),
                                    ErrorMode::FailFast(ref interrupted) => interrupted
                                        .store(true, std::sync::atomic::Ordering::Relaxed),
                                }
                                "ERROR:".red()
                            }
                            LogLevel::Warning => {
                                if !seen_warnings.insert(message) {
                                    return;
                                }
                                result.warning_count += 1;
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
            .expect("failed to spawn thread");
        let logger = Logger::new(tx);
        LogReceiver {
            logger_thread,
            logger,
        }
    }

    /// Get the current result of the logger thread after pending log tasks have
    /// been processed.
    pub fn peek_result(&self) -> LogResult {
        let (tx, rx) = std::sync::mpsc::channel();
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
pub struct LogAccumulator {
    log_receiver: LogReceiver,
    all_messages: Arc<std::sync::Mutex<Vec<String>>>,
}

impl LogAccumulator {
    pub fn new(error_mode: ErrorMode) -> (Self, Logger) {
        let all_messages = Arc::new(std::sync::Mutex::new(Vec::new()));
        let all_messages_clone = all_messages.clone();
        let log_receiver = LogReceiver::new(error_mode, move |msg| {
            all_messages_clone.lock().unwrap().push(msg.to_string());
        });
        let logger = log_receiver.get_logger();
        let ret = LogAccumulator {
            log_receiver,
            all_messages,
        };
        (ret, logger)
    }

    pub fn new_fail_fast() -> (Self, Logger, Arc<AtomicBool>) {
        let interrupted = Arc::new(AtomicBool::new(false));
        let (ret, logger) = Self::new(ErrorMode::FailFast(interrupted.clone()));
        (ret, logger, interrupted)
    }

    pub fn join_allow_warnings(self) -> Result<()> {
        let log_result = self.log_receiver.join();
        let all_messages = std::mem::take(self.all_messages.lock().unwrap().deref_mut());
        if log_result.error_count != 0 || log_result.warning_count != 0 {
            let messages = all_messages.join("\n");
            anyhow::bail!(
                "{} errors and {} warnings:\n{}",
                log_result.error_count,
                log_result.warning_count,
                messages
            );
        }
        Ok(())
    }

    pub fn join_no_warnings(self) -> Result<()> {
        let log_result = self.log_receiver.join();
        let all_messages = std::mem::take(self.all_messages.lock().unwrap().deref_mut());
        if log_result.error_count != 0 || log_result.warning_count != 0 {
            let messages = all_messages.join("\n");
            anyhow::bail!(
                "{} errors and {} warnings:\n{}",
                log_result.error_count,
                log_result.warning_count,
                messages
            );
        }
        Ok(())
    }
}
