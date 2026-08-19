//! Dependency-free local CLI for the InBharat Harness release candidate.

#![forbid(unsafe_code)]

mod app;
mod benchmark;
mod demo;
mod policy;

fn main() {
    std::process::exit(app::main_entry());
}
