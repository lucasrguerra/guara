#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::if_same_then_else,
    clippy::large_enum_variant,
    clippy::needless_range_loop
)]
//! Guará: núcleo de recuperação de arquivos.
//!
//! Princípio central: a origem é aberta **somente para leitura**; nada neste
//! crate escreve no dispositivo que está sendo recuperado.

pub mod aff4;
pub mod assemble;
pub mod audit;
pub mod carve;
pub mod crypt;
pub mod defrag;
pub mod device;
pub mod error;
pub mod ewf;
pub mod explorer;
pub mod external;
pub mod formats;
pub mod fs;
pub mod hashing;
pub mod health;
pub mod http;
pub mod image;
pub mod jpegscan;
pub mod lvm;
pub mod meta;
pub mod mp4rebuild;
pub mod notify;
pub mod owner;
pub mod partition;
pub mod raid;
pub mod raid_detect;
pub mod reconnect;
pub mod recover;
pub mod report;
pub mod scan;
pub mod search;
pub mod service;
pub mod settings;
pub mod signatures;
pub mod source;
pub mod sqlite;
pub mod thumb;
pub mod types;
pub mod vdisk;
pub mod volume;

pub use error::{Error, Result};
pub use types::*;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
