use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    DataSaver,
    Standard,
    #[default]
    #[serde(other)]
    Best,
}
impl Quality {
    pub fn next(self) -> Self {
        match self {
            Self::DataSaver => Self::Standard,
            Self::Standard => Self::Best,
            Self::Best => Self::DataSaver,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::DataSaver => crate::i18n::t("Data saver"),
            Self::Standard => crate::i18n::t("Standard"),
            Self::Best => crate::i18n::t("Best available"),
        }
    }
    pub fn format(self) -> &'static str {
        match self {
            Self::DataSaver => "bestaudio[abr<=64]/worstaudio/best[height<=240]/worst",
            Self::Standard => "bestaudio[abr<=160]/bestaudio/best",
            Self::Best => "bestaudio/best",
        }
    }
    pub fn level(self) -> u8 {
        match self {
            Self::DataSaver => 0,
            Self::Standard => 1,
            Self::Best => 2,
        }
    }
    pub fn from_level(level: u8) -> Self {
        match level {
            0 => Self::DataSaver,
            1 => Self::Standard,
            _ => Self::Best,
        }
    }
}

/// Validated audio preferences shared by native playback and downloads.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub jiosaavn: bool,
    pub streaming_quality: Quality,
    pub download_quality: Quality,
    pub normalize: bool,
    pub equalizer: [f64; 5],
    pub output: String,
    pub crossfade: f64,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            jiosaavn: true,
            streaming_quality: Quality::Best,
            download_quality: Quality::Best,
            normalize: false,
            equalizer: [0.; 5],
            output: "auto".into(),
            crossfade: 0.,
        }
    }
}
impl AudioSettings {
    pub fn validate(&mut self) {
        for gain in &mut self.equalizer {
            *gain = if gain.is_finite() {
                gain.clamp(-12., 12.)
            } else {
                0.
            };
        }
        self.crossfade = if self.crossfade.is_finite() {
            self.crossfade.clamp(0., 10.)
        } else {
            0.
        };
        if self.output.is_empty()
            || self.output.len() > 512
            || self.output.chars().any(char::is_control)
        {
            self.output = "auto".into();
        }
    }
}
