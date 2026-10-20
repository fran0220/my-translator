//! Diagnostic values from the actual mono PCM sent to translation, before mixing.
#[derive(Clone, Debug, Default)]
pub struct Meter {
    pub rms: f32,
    pub peak: f32,
    pub waveform: Vec<(f32, f32)>,
    pub samples: usize,
}
impl Meter {
    pub fn from_pcm(pcm: &[u8]) -> Self {
        let samples: Vec<f32> = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.)
            .collect();
        if samples.is_empty() {
            return Self::default();
        }
        let rms = (samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32).sqrt();
        let peak = samples.iter().fold(0f32, |p, v| p.max(v.abs()));
        let waveform = samples
            .chunks(samples.len().div_ceil(96))
            .map(|chunk| {
                chunk
                    .iter()
                    .fold((1f32, -1f32), |(lo, hi), &v| (lo.min(v), hi.max(v)))
            })
            .collect();
        Self {
            rms,
            peak,
            waveform,
            samples: samples.len(),
        }
    }
    pub fn db(value: f32) -> f32 {
        20. * value.max(0.000001).log10()
    }
    pub fn state(&self) -> &'static str {
        if self.samples == 0 {
            "无输入"
        } else if self.peak >= 0.999 {
            "削波"
        } else if self.peak == 0. {
            "静音（零 PCM）"
        } else {
            "收到音频"
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn measures_pcm_not_ui_animation() {
        let pcm: Vec<u8> = [0i16, 16384, -32768, 0]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        let meter = Meter::from_pcm(&pcm);
        assert!((meter.rms - (1.25f32 / 4.).sqrt()).abs() < 0.00001);
        assert_eq!(meter.peak, 1.);
        assert_eq!(meter.waveform, [(0., 0.), (0.5, 0.5), (-1., -1.), (0., 0.)]);
        assert_eq!(meter.state(), "削波");
        assert_eq!(Meter::from_pcm(&[0, 0]).state(), "静音（零 PCM）");
        assert_eq!(Meter::default().state(), "无输入");
    }
}
