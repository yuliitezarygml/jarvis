use anyhow::{Result, ensure};
use pocket_native::{
    Backend,
    config::{Config, Origin},
    model::{LoadOptions, TtsModel, VoiceState},
};
use std::{path::Path, sync::atomic::AtomicBool};

pub struct Engine {
    model: TtsModel<Backend>,
    voice: VoiceState<Backend>,
}
impl Engine {
    pub fn load(dir: &Path) -> Result<Self> {
        let mut config = Config::parse(&std::fs::read_to_string(dir.join("model-config.yaml"))?)?;
        config.weights_path = Some(
            dir.join("model_fast.safetensors")
                .to_string_lossy()
                .into_owned(),
        );
        config.weights_path_without_voice_cloning = None;
        config.flow_lm.lookup_table.tokenizer_path =
            dir.join("tokenizer.model").to_string_lossy().into_owned();
        ensure!(config.mimi.sample_rate == 24000, "expected 24 kHz model");
        let model = TtsModel::load(
            config,
            Origin::Custom("Jarvis RU".into()),
            &LoadOptions::default(),
        )?;
        let voice = model
            .get_state_for_audio_prompt(&dir.join("jarvis-ru.wav").to_string_lossy(), false)?;
        Ok(Self { model, voice })
    }
    pub fn synth(
        &self,
        text: &str,
        stop: &AtomicBool,
        sink: &mut (dyn FnMut(&[f32]) -> Result<()> + Send),
    ) -> Result<()> {
        self.model
            .generate_audio_stream(&self.voice, text, 50, None, stop, sink)
    }
}
