#![deny(unsafe_code)]

pub mod error;
pub mod permission;

pub use error::{Error, Result};
pub use permission::Role;
