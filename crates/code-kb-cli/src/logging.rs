use std::fs;
use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Returns the primary log directory for the given workspace root.
pub fn get_log_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".code-kb").join("logs")
}

/// Returns the full path to the active log file.
#[allow(dead_code)]
pub fn get_log_file(workspace_root: &Path) -> PathBuf {
    get_log_dir(workspace_root).join("code-kb.log")
}

/// Initializes structured logging to `.code-kb/logs/code-kb.log` and optionally stderr.
///
/// NOTE: When `is_serve` is true (MCP mode over stdio), all stdout logging is strictly
/// suppressed to prevent protocol stream corruption.
pub fn init_logging(workspace_root: &Path, is_serve: bool, verbose: bool) -> Option<WorkerGuard> {
    let log_dir = get_log_dir(workspace_root);
    if let Err(e) = fs::create_dir_all(&log_dir) {
        eprintln!(
            "Warning: Failed to create log directory '{}': {e}",
            log_dir.display()
        );
        return None;
    }

    // Rolling daily log appender in .code-kb/logs/ capped at 7 files
    let file_appender = match tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("code-kb.log")
        .max_log_files(7)
        .build(&log_dir)
    {
        Ok(appender) => appender,
        Err(e) => {
            eprintln!("Warning: Failed to initialize rolling file appender: {e}");
            return None;
        }
    };
    let (non_blocking_file, guard) = tracing_appender::non_blocking(file_appender);

    // Build default EnvFilter
    let default_filter = if verbose {
        "code_kb_core=debug,code_kb_cli=debug,code_kb=debug,info"
    } else {
        "code_kb_core=info,code_kb_cli=info,code_kb=info,warn"
    };

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    let file_layer = fmt::layer()
        .with_writer(non_blocking_file)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true);

    // Stderr layer only in CLI mode when verbose or RUST_LOG is set
    let stderr_layer = if !is_serve && (verbose || std::env::var_os("RUST_LOG").is_some()) {
        Some(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(true)
                .with_target(false),
        )
    } else {
        None
    };

    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(stderr_layer);

    let _ = subscriber.try_init();

    Some(guard)
}
