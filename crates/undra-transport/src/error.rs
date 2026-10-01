//! The error of starting a server.

use core::fmt;
use std::io;

use undra_runtime::InitError;

/// Why [`Server::start`](crate::Server::start) or [`Server::bind`](crate::Server::bind) failed.
#[derive(Debug)]
pub enum ServeError {
    /// The listening socket could not be bound (address in use, no permission, no such
    /// interface) or its address could not be read.
    Io(io::Error),
    /// The closure given to [`Server::start`](crate::Server::start) failed to build the
    /// runtime.
    Runtime(InitError),
}

impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServeError::Io(e) => write!(f, "could not listen: {e}"),
            ServeError::Runtime(e) => write!(f, "could not start the runtime: {e}"),
        }
    }
}

impl std::error::Error for ServeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ServeError::Io(e) => Some(e),
            ServeError::Runtime(e) => Some(e),
        }
    }
}

impl From<io::Error> for ServeError {
    fn from(e: io::Error) -> Self {
        ServeError::Io(e)
    }
}

impl From<InitError> for ServeError {
    fn from(e: InitError) -> Self {
        ServeError::Runtime(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_the_cause() {
        let io = ServeError::from(io::Error::new(io::ErrorKind::AddrInUse, "in use"));
        assert_eq!(io.to_string(), "could not listen: in use");
        let init = ServeError::from(InitError::InvalidMode("x".into()));
        assert!(init.to_string().starts_with("could not start the runtime"));
        assert!(std::error::Error::source(&io).is_some());
        assert!(std::error::Error::source(&init).is_some());
    }
}
