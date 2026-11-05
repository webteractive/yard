//! The app's side effects outside yardd: the CLI, settings, opening things, the terminal, and
//! finding `yard` (M6 design §5.8, §10.3, §8.3).

pub mod activation;
pub mod cli;
pub mod locator;
pub mod open;
pub mod settings;
pub mod terminal;
