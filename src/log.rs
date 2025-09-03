use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use colored::Colorize as _;
use itertools::Itertools as _;
use log::Log as _;
use std::cell::RefCell;
use std::fmt;
use std::io::Write as _;
use std::ops::Deref;
use std::ops::DerefMut;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use tracing_log::LogTracer;
use tracing_subscriber::layer::SubscriberExt as _;

/// Keeps the tracing framework configured to store both log and trace events.
struct GlobalFileTraceLogger {
    pub log_to_file: Mutex<DelayedWriter<std::fs::File>>,
    pub log_to_tracing: LogTracer,
    pub chrome_writer: ArcMutexWriter<std::io::BufWriter<std::fs::File>>,
    chrome_guard: tracing_chrome::FlushGuard,
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

static GLOBAL_LOGGER: std::sync::OnceLock<GlobalLogger> = std::sync::OnceLock::new();

pub fn init() -> &'static GlobalLogger {
    let global_logger = GlobalLogger {
        log_to_stderr: Mutex::new(StderrLogger::new()),
        log_to_file: Arc::new(Mutex::new(Some(GlobalFileTraceLogger::init_tracing()))),
    };
    if GLOBAL_LOGGER.set(global_logger).is_err() {
        panic!("GLOBAL_LOGGER has already been initialized");
    }
    let global_logger = GLOBAL_LOGGER.get().unwrap();
    log::set_logger(global_logger).expect("global logger not set yet");
    // Include everything in the log file.
    log::set_max_level(log::LevelFilter::Trace);
    global_logger
}

pub fn get_global_logger() -> &'static GlobalLogger {
    GLOBAL_LOGGER.get().unwrap()
}

pub type InternalLogger = dyn Fn(log::Level, &str) + Send;

struct StderrLogger {
    /// The function that is called to log messages to `stderr`. If set to
    /// `None`, the `eprintln!` macro is used to write straight to `stderr`.
    ///
    /// The implementation is simplified this way by assuming the stderr logging
    /// only needs to be overridden by one interest at a time.
    pub log_fn: Option<Box<InternalLogger>>,
    /// The log level for the stderr logger.
    pub level: log::LevelFilter,
}

impl StderrLogger {
    pub fn new() -> Self {
        StderrLogger {
            log_fn: None,
            level: log::LevelFilter::Info,
        }
    }

    pub fn log(&self, record: &log::Record<'_>, context_and_msg: &str) {
        if record.level() <= self.level {
            if let Some(log_fn) = &self.log_fn {
                log_fn(record.level(), context_and_msg);
            } else {
                eprint_log(record.level(), context_and_msg);
            }
        }
    }
}

pub struct GlobalLogger {
    log_to_stderr: Mutex<StderrLogger>,
    log_to_file: Arc<Mutex<Option<GlobalFileTraceLogger>>>,
}

impl GlobalLogger {
    pub fn set_stderr_log_level(&self, level: log::LevelFilter) {
        self.log_to_stderr.lock().unwrap().level = level;
    }

    pub fn write_to_git_dir(&self, git_dir: &Path) -> Result<()> {
        self.log_to_file
            .lock()
            .unwrap()
            .as_mut()
            .map_or(Ok(()), |logger| logger.write_to_git_dir(git_dir))
    }

    pub fn finalize(&self) {
        if let Some(logger) = self.log_to_file.lock().unwrap().take() {
            logger.finalize();
        }
    }

    /// Wraps the current logger with an `indicatif::MultiProgress` instance
    /// and makes sure the progress bar does not interfere with the
    /// logging output.
    ///
    /// TODO: This implementation creates an extra thread for logging,
    /// which is not ideal.
    pub fn with_progress<F, T>(&self, task: F) -> Result<T>
    where
        F: FnOnce(indicatif::MultiProgress) -> Result<T>,
    {
        let progress = indicatif::MultiProgress::new();
        // TODO: 2025-02-27 The rate limiter seems to be a bit broken. 1 Hz is
        // not smooth, default 20 Hz hogs the CPU, 2 Hz is a good compromise as
        // the result is actually much higher anyway.
        progress.set_draw_target(indicatif::ProgressDrawTarget::stderr_with_hz(2));

        let progress_clone = progress.clone();
        let old_stderr_log_fn =
            self.log_to_stderr
                .lock()
                .unwrap()
                .log_fn
                .replace(Box::new(move |level, msg| {
                    progress_clone.suspend(|| eprint_log(level, msg));
                }));
        assert!(
            old_stderr_log_fn.is_none(),
            "only one progress logger at a time"
        );
        let result = task(progress.clone());
        // Restore to default stderr logging.
        self.log_to_stderr.lock().unwrap().log_fn.take();
        progress.clear()?;
        result
    }
}

impl log::Log for GlobalLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            // Let the log message include the context.
            let msg = fmt::format(*record.args());
            let context = CURRENT_LOG_SCOPE.with(|cell| {
                cell.borrow()
                    .as_ref()
                    .map_or_else(String::new, |scope| scope.full_context())
            });
            let context_and_msg = if context.is_empty() {
                msg
            } else {
                format!("{context}: {msg}")
            };

            // Log to stderr.
            self.log_to_stderr
                .lock()
                .unwrap()
                .log(record, &context_and_msg);
            // Log to file.
            if let Some(logger) = self.log_to_file.lock().unwrap().as_ref() {
                // Make a record that includes the context.
                logger.log(
                    &log::Record::builder()
                        .metadata(record.metadata().clone())
                        .args(format_args!("{context_and_msg}"))
                        .module_path(record.module_path())
                        .file(record.file())
                        .line(record.line())
                        .build(),
                );
            }
        }
    }

    fn flush(&self) {
        if let Some(logger) = self.log_to_file.lock().unwrap().as_ref() {
            logger.flush();
        }
    }
}

impl GlobalFileTraceLogger {
    pub fn init_tracing() -> Self {
        let log_to_file = Mutex::new(DelayedWriter::<std::fs::File>::Buffered(Vec::new()));
        // Convert log messages to tracing events, in case any dependency is
        // using the log framework.
        let log_to_tracing = tracing_log::LogTracer::new();

        let chrome_writer = ArcMutexWriter::new(DelayedWriter::Buffered(Vec::new()));
        let (chrome_layer, chrome_guard) = tracing_chrome::ChromeLayerBuilder::new()
            .writer(chrome_writer.clone())
            .include_args(true)
            .include_locations(false)
            .build();

        let subscriber = tracing_subscriber::registry().with(chrome_layer);
        tracing::subscriber::set_global_default(subscriber).expect("set global subscriber");

        Self {
            log_to_file,
            log_to_tracing,
            chrome_writer,
            chrome_guard,
        }
    }

    pub fn write_to_git_dir(&mut self, git_dir: &Path) -> Result<()> {
        let toprepo_dir = git_dir.join("toprepo");
        std::fs::create_dir_all(&toprepo_dir)?;
        // Unbuffered writes to log file.
        let log_path = toprepo_dir.join("log");
        let log_file = std::fs::File::create(&log_path)?;
        self.log_to_file
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
    pub fn finalize(self) {
        self.flush();
        // Finalize and close the Chrome trace file.
        std::mem::drop(self.chrome_guard);
    }

    pub fn log(&self, record: &log::Record<'_>) {
        self.log_to_tracing.log(record);
        let ts = chrono::Local::now().format("%+");
        if let Err(err) = writeln!(
            self.log_to_file.lock().unwrap(),
            "{ts} {}: {}",
            record.level().as_str(),
            record.args()
        ) {
            eprintln!("Failed to write log message to file: {err}");
        }
    }

    pub fn flush(&self) {
        self.log_to_tracing.flush();
        let _ignored = self.log_to_file.lock().unwrap().flush();
    }
}

fn eprint_log(level: log::Level, msg: &str) {
    eprintln!("{}: {msg}", log_level_colored_str(level));
}

pub type LogLevel = log::Level;

fn log_level_colored_str(level: log::Level) -> colored::ColoredString {
    let s = level.as_str();
    match level {
        log::Level::Error => s.red().bold(),
        log::Level::Warn => s.yellow().bold(),
        log::Level::Info => s.green(),
        log::Level::Debug => s.blue(),
        log::Level::Trace => s.into(),
    }
}

thread_local! {
    pub static CURRENT_LOG_SCOPE: RefCell<Option<Rc<LogScopeContext>>> = const { RefCell::new(None) };
}

struct LogScopeContext {
    /// Parent scope.
    parent: Option<Rc<LogScopeContext>>,
    /// Previous context in this thread.
    previous: Option<Rc<LogScopeContext>>,
    context: String,
}

impl LogScopeContext {
    /// Creates a full context string that includes the parent scopes.
    pub fn full_context(&self) -> String {
        if let Some(parent) = &self.parent {
            let parent_full_context = parent.full_context();
            if !parent_full_context.is_empty() {
                return format!("{parent_full_context}: {}", self.context);
            }
        }
        self.context.clone()
    }
}

pub fn current_scope() -> String {
    CURRENT_LOG_SCOPE.with(|cell| {
        cell.borrow()
            .as_ref()
            .map_or_else(String::new, |scope| scope.full_context())
    })
}

/// A scope for logging that has been entered.
pub struct LogScope {
    inner: Rc<LogScopeContext>,
}

impl LogScope {
    pub fn new(context: String) -> Self {
        let parent = CURRENT_LOG_SCOPE.with(|cell| cell.borrow().clone());
        Self::new_and_enter(context, parent)
    }

    /// Creates a new logging scope with the given context and enters it. The
    /// `parent` can refer to a scope from a different thread.
    pub fn with_parent(context: String, parent: &LogScope) -> Self {
        Self::new_and_enter(context, Some(parent.inner.clone()))
    }

    fn new_and_enter(context: String, parent: Option<Rc<LogScopeContext>>) -> Self {
        let inner = CURRENT_LOG_SCOPE.with(|cell| {
            cell.replace_with(|previous| {
                Some(Rc::new(LogScopeContext {
                    parent: parent.clone(),
                    previous: previous.take(),
                    context,
                }))
            });
            cell.borrow().clone().unwrap()
        });
        LogScope { inner }
    }

    /// Creates a full context string that includes the parent scopes.
    pub fn full_context(&self) -> String {
        self.inner.full_context()
    }
}

impl Drop for LogScope {
    fn drop(&mut self) {
        let active_scope = CURRENT_LOG_SCOPE
            .with(|cell| cell.replace(self.inner.previous.clone()))
            .expect("LogScope exists in thread");
        debug_assert!(
            Rc::ptr_eq(&active_scope, &self.inner),
            "LogScope was not dropped in the correct order"
        );
    }
}

/// Creates a new logging scope with the given context and enters it.
pub fn scope(context: impl Into<String>) -> LogScope {
    LogScope::new(context.into())
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
    fn new(strategy: ErrorMode) -> Self {
        ErrorObserver {
            counter: Arc::new(AtomicUsize::new(0)),
            strategy,
        }
    }

    pub fn run_keep_going<T>(
        keep_going: bool,
        f: impl FnOnce(&ErrorObserver) -> Result<T>,
    ) -> Result<T> {
        Self::run(ErrorMode::from_keep_going_flag(keep_going), f)
    }

    /// Runs the given function while collecting and logging errors. After `f`
    /// has returned, `has_got_errors()` is called and an error will be returned
    /// if any error was observed during processing.
    pub fn run<T>(strategy: ErrorMode, f: impl FnOnce(&ErrorObserver) -> Result<T>) -> Result<T> {
        let observer = ErrorObserver::new(strategy);
        let result = f(&observer);
        if observer.has_got_errors() {
            observer.maybe_consume(result.map(|_| ()))?;
            let error_count = observer.counter.load(std::sync::atomic::Ordering::Relaxed);
            bail!(
                "Processing failed, see the {error_count} previous error{}",
                if error_count == 1 { "" } else { "s" }
            );
        }
        result
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

    /// Log the error.
    pub fn consume_interrupted(&self, result: InterruptedResult<()>) {
        match result {
            Ok(_) => {}
            Err(InterruptedError::Interrupted) => {}
            Err(InterruptedError::Normal(err)) => {
                self.counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                log::error!("{err:#}");
            }
        }
    }

    /// Log if error.
    pub fn consume<T>(&self, result: Result<T>) -> Option<T> {
        result
            .inspect_err(|err| {
                self.counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                log::error!("{err:#}");
            })
            .ok()
    }

    /// In keep-going mode, write the error to the logger and return `Ok(())`.
    ///
    /// In fail-fast mode, simply return the result and count the errors.
    pub fn maybe_consume(&self, result: Result<()>) -> Result<()> {
        match result {
            Ok(_) => Ok(()),
            Err(err) => {
                self.counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                match self.strategy {
                    ErrorMode::KeepGoing => {
                        log::error!("{err:#}");
                        Ok(())
                    }
                    ErrorMode::FailFast => Err(err),
                }
            }
        }
    }
}

/// Macro version of `pub fn command_span<T>(name: &'static str, cmd: &mut std::process::Command) -> &mut std::process::Command`
/// to allow for constant name.
// TODO: What module?
#[macro_export]
macro_rules! command_span {
    ($span_name: expr) => {
        |cmd: &std::process::Command| -> tracing::Span {
            tracing::info_span!(
                $span_name,
                args = ?cmd.get_args().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>(),
                cwd = cmd.get_current_dir().map_or(".".to_owned(),|p| p.to_string_lossy().to_string()),
                envs = ?cmd.get_envs().map(|(key, value)| match value {
                        Some(value) => format!("{}={}", key.to_string_lossy(), value.to_string_lossy()),
                        None => key.to_string_lossy().to_string(),
                    }).collect::<Vec<_>>(),
                prog = ?cmd.get_program().to_string_lossy(),
            )
        }
    };
}

/// Format command arguments for shell command line. If escaping is needed,
/// single quotes are added.
///
/// Note: This is not a complete implementation.
///
/// # Examples
/// ```
/// # use git_toprepo::log::format_arg_lossy;
/// use std::ffi::OsStr;
///
/// assert_eq!(format_arg_lossy(OsStr::new("arg")), "arg");
/// assert_eq!(format_arg_lossy(OsStr::new("a'rg")), "a\\'rg");
/// assert_eq!(format_arg_lossy(OsStr::new("ar\"g")), "'ar\"g'");
/// assert_eq!(format_arg_lossy(OsStr::new("a$rg")), "'a$rg'");
/// assert_eq!(format_arg_lossy(OsStr::new("some arg")), "'some arg'");
/// assert_eq!(format_arg_lossy(OsStr::new("som'e arg")), "'som'\\''e arg'");
/// ```
pub fn format_arg_lossy(arg: &std::ffi::OsStr) -> String {
    let s = arg.to_string_lossy();
    if [' ', '"', '$'].iter().any(|c| s.contains(*c)) {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else if s.contains('\'') {
        s.replace('\'', "\\'")
    } else {
        s.to_string()
    }
}

pub fn command_to_string_lossy(cmd: &std::process::Command) -> String {
    format!(
        "{} {}",
        format_arg_lossy(cmd.get_program()),
        cmd.get_args().map(format_arg_lossy).join(" ")
    )
}

pub struct CommandSpanScope<'a> {
    command: &'a mut std::process::Command,
    entered_span: Option<tracing::span::EnteredSpan>,
}

impl<'a> CommandSpanScope<'a> {
    /// Creates a new command span scope.
    pub fn new(command: &'a mut std::process::Command, span: tracing::Span) -> Self {
        let cmd_string = command_to_string_lossy(command);
        let shorter_cmd_string = cmd_string.chars().take(100).collect::<String>();
        if shorter_cmd_string.len() < cmd_string.len() {
            log::debug!("Running {shorter_cmd_string} ... (truncated)",);
            log::trace!("Full command: {cmd_string}");
        } else {
            log::debug!("Running {cmd_string}");
        }
        CommandSpanScope {
            command,
            entered_span: Some(span.entered()),
        }
    }

    pub fn spawn(mut self) -> std::io::Result<(std::process::Child, tracing::span::EnteredSpan)> {
        let entered_span = self.entered_span.take().unwrap();
        match self.command.spawn() {
            Ok(child) => Ok((child, entered_span)),
            Err(err) => {
                if let Some(metadata) = entered_span.metadata() {
                    log::error!("Failed to start command {}: {}", metadata.name(), err);
                }
                Err(err)
            }
        }
    }
}

impl Deref for CommandSpanScope<'_> {
    type Target = std::process::Command;

    fn deref(&self) -> &Self::Target {
        self.command
    }
}

impl DerefMut for CommandSpanScope<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.command
    }
}

impl Drop for CommandSpanScope<'_> {
    fn drop(&mut self) {
        if let Some(entered_span) = &self.entered_span
            && let Some(metadata) = entered_span.metadata()
        {
            log::trace!("Command finished: {}", metadata.name());
        }
    }
}

pub trait CommandSpanExt {
    fn trace_command(
        &mut self,
        span_fn: impl FnOnce(&std::process::Command) -> tracing::Span,
    ) -> CommandSpanScope<'_>;
}

impl CommandSpanExt for std::process::Command {
    fn trace_command(
        &mut self,
        span_fn: impl FnOnce(&std::process::Command) -> tracing::Span,
    ) -> CommandSpanScope<'_> {
        let span = span_fn(self);
        CommandSpanScope::new(self, span)
    }
}
