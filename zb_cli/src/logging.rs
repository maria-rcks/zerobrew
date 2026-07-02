//! Tracing subscriber setup for library (`zb_io`/`zb_core`) diagnostics.
//!
//! Log lines are diagnostics, not data: they always go to **stderr**, with
//! ANSI gated on the same per-stream color decision as the `Ui`, and they are
//! routed through the shared `indicatif::MultiProgress` so a warning emitted
//! mid-download clears and redraws the progress bars instead of tearing them.

use indicatif::MultiProgress;
use std::io::{self, Write};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub fn init(verbose: u8, quiet: bool, progress: MultiProgress, ansi: bool) {
    let level = if quiet {
        LevelFilter::ERROR
    } else {
        match verbose {
            0 => LevelFilter::WARN,
            1 => LevelFilter::INFO,
            2 => LevelFilter::DEBUG,
            _ => LevelFilter::TRACE,
        }
    };

    let filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy();

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .without_time()
                .with_ansi(ansi)
                .with_writer(SuspendMakeWriter { progress }),
        )
        .try_init();
}

/// A `MakeWriter` that buffers each log event and emits it to stderr inside
/// `MultiProgress::suspend`, so log lines and live progress bars never
/// interleave. When no bars are active, `suspend` is effectively a direct
/// write.
struct SuspendMakeWriter {
    progress: MultiProgress,
}

impl<'a> MakeWriter<'a> for SuspendMakeWriter {
    type Writer = SuspendWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SuspendWriter {
            progress: self.progress.clone(),
            buf: Vec::with_capacity(128),
        }
    }
}

struct SuspendWriter {
    progress: MultiProgress,
    buf: Vec<u8>,
}

impl SuspendWriter {
    fn emit(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let buf = std::mem::take(&mut self.buf);
        self.progress.suspend(|| {
            let mut err = io::stderr().lock();
            err.write_all(&buf)?;
            err.flush()
        })
    }
}

impl Write for SuspendWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.emit()
    }
}

impl Drop for SuspendWriter {
    fn drop(&mut self) {
        let _ = self.emit();
    }
}

#[cfg(test)]
mod tests {
    use super::init;
    use indicatif::MultiProgress;

    #[test]
    fn init_is_idempotent() {
        init(0, false, MultiProgress::new(), false);
        init(2, false, MultiProgress::new(), false);
        init(0, true, MultiProgress::new(), false);
    }
}
