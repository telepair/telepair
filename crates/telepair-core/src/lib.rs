#![deny(unsafe_code)]

pub mod config;
pub mod error;
pub mod permission;
pub mod protocol;
pub mod session;
pub mod storage;
pub mod target;

pub use error::{Error, Result};
pub use permission::Role;
