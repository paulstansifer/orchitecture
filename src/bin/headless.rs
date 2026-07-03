//! Headless testing mode entry point. Reads commands from stdin and writes
//! responses to stdout; no window, renderer, or Bevy `App` involved. See
//! `orchitecture_lib::headless` for the protocol and command reference, or
//! run with `help` as the first command.
//!
//! Usage: cargo run --bin headless [-- --seed <n>]

fn main() {
    orchitecture_lib::headless::run();
}
