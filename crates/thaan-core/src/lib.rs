//! Headless engine of the database manager.
//!
//! Everything a user can do is a command registered in [`commands`] and executed by
//! [`engine::Engine`]. The GUI, CLI and MCP are clients of this crate.

pub mod commands;
pub mod connection;
pub mod dialect;
pub mod doc;
pub mod driver;
pub mod engine;
pub mod error;
pub mod export;
pub mod params;
pub mod result;
pub mod schema;
pub mod splitter;
pub mod store;
pub mod table;
pub mod value;

pub use error::{Error, Result};
pub use result::{Column, ResultSet};
pub use value::Value;
