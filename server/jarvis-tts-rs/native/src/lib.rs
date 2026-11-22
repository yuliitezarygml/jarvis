//! In-process Pocket TTS tensor runtime. All assets are local.
pub mod audio;
pub mod config;
pub mod defaults;
pub mod model;
mod rng;
pub mod text;
pub mod tokenizer;
pub mod weights;
mod download {
    pub fn download_if_necessary(path: &str) -> anyhow::Result<std::path::PathBuf> {
        anyhow::ensure!(
            std::path::Path::new(path).is_file(),
            "missing local model asset: {path}"
        );
        Ok(path.into())
    }
}
pub type Backend = xn::Unquantized<f32, xn::CpuDevice>;
pub use xn::set_num_threads;
