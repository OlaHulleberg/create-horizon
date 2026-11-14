//! The mod's own log. The launcher discards the game's standard error and the mod API has no log
//! of its own, so lines go to the file `CREATE_DEBUG_LOG` names (set it per instance with
//! `horizon-launcher instance launch <instance> --env "CREATE_DEBUG_LOG=/path/create.log"`), and
//! nowhere without it. `create_log!("...")` formats like `eprintln!`; each line carries the time
//! since the mod's first line and the thread (`Server thread`, `Render thread`).

use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static FILE: OnceLock<Option<(Mutex<File>, Instant)>> = OnceLock::new();

fn file() -> Option<&'static (Mutex<File>, Instant)> {
    FILE.get_or_init(|| {
        let path = std::env::var_os("CREATE_DEBUG_LOG")?;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        Some((Mutex::new(f), Instant::now()))
    })
    .as_ref()
}

/// Writes one line. Use `create_log!`.
pub fn line(args: std::fmt::Arguments<'_>) {
    let Some((f, start)) = file() else {
        return;
    };
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("?");
    if let Ok(mut f) = f.lock() {
        let _ = writeln!(f, "[{:9.3}] [{name}] {args}", start.elapsed().as_secs_f64());
    }
}

/// `create_log!("[create] {n} states")`: a line in the mod's log, when it has one.
#[macro_export]
macro_rules! create_log {
    ($($arg:tt)*) => {
        $crate::glue::log::line(format_args!($($arg)*))
    };
}
