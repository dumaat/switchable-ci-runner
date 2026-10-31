//! The portable part of switchable-ci-runner: the decision, the config, the
//! plan for several runners and the runs the machine took.
//! The tray, the idle and power readings and the runner process live in the binary.

pub mod config;
pub mod cpu;
pub mod decision;
pub mod parallel;
pub mod runs;
