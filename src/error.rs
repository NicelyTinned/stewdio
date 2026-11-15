#![allow(dead_code)]
use std::fmt;

pub const GENERAL: i32 = 1;
pub const ARGS: i32 = 2;
pub const PROJECT: i32 = 3;
pub const PARSE: i32 = 4;
pub const COMPILE: i32 = 5;
pub const AUTH: i32 = 6;
pub const UPLOAD: i32 = 7;
pub const UNSUPPORTED: i32 = 8;

#[derive(Debug)]
pub struct CliError {
    pub code: i32,
    pub message: String,
}

impl CliError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        CliError { code, message: message.into() }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::new(GENERAL, format!("I/O error: {e}"))
    }
}

pub type CliResult<T> = std::result::Result<T, CliError>;
