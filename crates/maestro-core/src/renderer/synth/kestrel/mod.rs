mod gpu;
pub(super) mod library;
mod synth;

pub const KESTREL_VERSION: &str = env!("KESTREL_VERSION");

pub use gpu::{KestrelAdapter, kestrel_adapters};
pub(crate) use library::KestrelLib;
pub(crate) use synth::KestrelSynth;
