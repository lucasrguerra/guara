//! Guará — núcleo de recuperação de arquivos.
//!
//! Princípio central: a origem é aberta **somente para leitura**; nada neste
//! crate escreve no dispositivo que está sendo recuperado.

pub mod carve;
pub mod device;
pub mod error;
pub mod fs;
pub mod image;
pub mod partition;
pub mod recover;
pub mod scan;
pub mod service;
pub mod signatures;
pub mod source;
pub mod types;

pub use error::{Error, Result};
pub use types::*;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
