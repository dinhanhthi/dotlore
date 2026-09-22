//! Dotlore's macOS menu-bar app.

use std::path::PathBuf;

fn main() {
    // The only environment this binary (the desktop app) reads, once, at
    // startup: `DOTLORE_HOME` (through `default_home`) and `$HOME`.
    let home = dotlore::config::default_home();
    let home_dir = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    dotlore::run(home, home_dir);
}
