//! Natural-language commands: a small local model maps a request to one typed tool call.

pub mod call;
#[cfg(feature = "ai")]
pub mod model;
pub mod prompt;
pub mod tools;
