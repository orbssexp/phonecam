use serde::{Deserialize, Serialize};

/// Energy/quality profile chosen in the UI (spec §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Economy,
    #[default]
    Balanced,
    Quality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoMode {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub max_bitrate_bps: u32,
}

impl Profile {
    pub fn mode(self) -> VideoMode {
        match self {
            Profile::Economy => VideoMode { width: 1280, height: 720, fps: 30, max_bitrate_bps: 2_500_000 },
            Profile::Balanced => VideoMode { width: 1920, height: 1080, fps: 30, max_bitrate_bps: 5_000_000 },
            Profile::Quality => VideoMode { width: 3840, height: 2160, fps: 30, max_bitrate_bps: 15_000_000 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_balanced_1080p30() {
        let m = Profile::default().mode();
        assert_eq!((m.width, m.height, m.fps), (1920, 1080, 30));
    }

    #[test]
    fn profiles_map_to_spec_modes() {
        assert_eq!(Profile::Economy.mode(), VideoMode { width: 1280, height: 720, fps: 30, max_bitrate_bps: 2_500_000 });
        assert_eq!(Profile::Balanced.mode(), VideoMode { width: 1920, height: 1080, fps: 30, max_bitrate_bps: 5_000_000 });
        assert_eq!(Profile::Quality.mode(), VideoMode { width: 3840, height: 2160, fps: 30, max_bitrate_bps: 15_000_000 });
    }

    #[test]
    fn profile_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&Profile::Economy).unwrap(), "\"economy\"");
    }
}
