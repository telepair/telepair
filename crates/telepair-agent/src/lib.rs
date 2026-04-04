#![deny(unsafe_code)]

pub mod pty;
pub mod virtual_target;

pub(crate) fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}
