//! Explicit output selection, stable hardware identity and exact preflight.
use crate::{alsa_guard, source_format::Format};
use alsa::{
    Direction, ValueOr,
    pcm::{Access, HwParams, PCM},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Read, path::PathBuf};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sample {
    S16,
    S32,
}
impl Sample {
    pub fn raw(self) -> i32 {
        match self {
            Self::S16 => 2,
            Self::S32 => 10,
        }
    }
    pub fn bits(self) -> u16 {
        match self {
            Self::S16 => 16,
            Self::S32 => 32,
        }
    }
    pub fn mpv(self) -> &'static str {
        match self {
            Self::S16 => "s16",
            Self::S32 => "s32",
        }
    }
    fn alsa(self) -> alsa::pcm::Format {
        match self {
            Self::S16 => alsa::pcm::Format::S16LE,
            Self::S32 => alsa::pcm::Format::S32LE,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    #[serde(skip)]
    pub card: i32,
    #[serde(skip)]
    pub device: u32,
    #[serde(skip)]
    pub card_id: String,
}
impl Device {
    pub fn route(&self) -> String {
        format!("hw:CARD={},DEV={},SUBDEV=0", self.card_id, self.device)
    }
}
// Index is only a current lookup location; identity hashes the physical parent,
// card ID, PCM ID, device number and available hardware/USB serial attributes.
pub fn identity(card: i32, device: u32, pcm_id: &[u8]) -> Option<String> {
    let root = PathBuf::from(format!("/sys/class/sound/card{card}"));
    let physical = std::fs::canonicalize(root.join("device")).ok()?;
    if physical.to_string_lossy().contains("/virtual/") {
        return None;
    }
    let card_id = std::fs::read(root.join("id")).ok()?;
    let mut hash = Sha256::new();
    for value in [
        physical.as_os_str().as_encoded_bytes(),
        card_id.as_slice(),
        pcm_id,
        &device.to_le_bytes(),
    ] {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value);
    }
    for parent in physical
        .ancestors()
        .take_while(|parent| parent.starts_with("/sys/devices"))
        .take(6)
    {
        for name in ["idVendor", "idProduct", "serial", "vendor", "device"] {
            if let Ok(file) = std::fs::File::open(parent.join(name)) {
                let mut value = Vec::new();
                if file.take(4097).read_to_end(&mut value).is_ok() && value.len() <= 4096 {
                    hash.update(name.as_bytes());
                    hash.update((value.len() as u64).to_le_bytes());
                    hash.update(value);
                }
            }
        }
    }
    Some(format!("{:x}", hash.finalize()))
}
fn device_name(card: &str, pcm: &str, pcm_id: &str, device: u32) -> String {
    let card = if card.trim().is_empty() {
        "ALSA card"
    } else {
        card.trim()
    };
    let fallback = format!("Playback {device}");
    let pcm = [pcm.trim(), pcm_id.trim()]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or(&fallback);
    format!("{card} · {pcm}").chars().take(160).collect()
}
pub fn explain_preflight(error: &str, source: Option<Format>) -> String {
    match source {
        Some(source)
            if matches!(
                error,
                "Fidelity device does not support the exact source rate"
                    | "Fidelity device has insufficient native sample precision"
                    | "Fidelity device does not support the exact source format."
            ) =>
        {
            format!(
                "{}. Source: {} Hz, {}-bit stereo. Choose Desktop/PipeWire or a compatible device; Fidelity does not resample or reduce precision.",
                error.trim_end_matches('.'),
                source.rate,
                source.bits
            )
        }
        _ => error.to_owned(),
    }
}
pub fn discover() -> Vec<Device> {
    let mut devices = Vec::new();
    for card in alsa::card::Iter::new().take(64).flatten() {
        let Ok(ctl) = alsa::Ctl::from_card(&card, true) else {
            continue;
        };
        let Ok(info) = ctl.card_info() else {
            continue;
        };
        let Ok(card_id) = info.get_id() else {
            continue;
        };
        if card_id.is_empty()
            || card_id.len() > 64
            || !card_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            continue;
        }
        for device in alsa::ctl::DeviceIter::new(&ctl).take(64) {
            let Ok(pcm) = ctl.pcm_info(device as u32, 0, Direction::Playback) else {
                continue;
            };
            let Ok(pcm_id) = pcm.get_id() else {
                continue;
            };
            let Some(id) = identity(card.get_index(), device as u32, pcm_id.as_bytes()) else {
                continue;
            };
            let name = device_name(
                info.get_name().unwrap_or("ALSA card"),
                pcm.get_name().unwrap_or("Playback"),
                pcm_id,
                device as u32,
            );
            devices.push(Device {
                id,
                name,
                card: card.get_index(),
                device: device as u32,
                card_id: card_id.to_owned(),
            });
            if devices.len() == 128 {
                return devices;
            }
        }
    }
    devices
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    #[default]
    Desktop,
    Fidelity {
        device: String,
    },
}
impl Selection {
    pub fn fidelity(&self) -> bool {
        matches!(self, Self::Fidelity { .. })
    }
    pub fn resolve(&self) -> Result<Option<Device>, &'static str> {
        let Self::Fidelity { device } = self else {
            return Ok(None);
        };
        let matches = discover()
            .into_iter()
            .filter(|d| d.id == *device)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(
                "The selected fidelity device is missing or ambiguous; reconnect/refresh it or explicitly select Desktop.",
            );
        }
        Ok(matches.into_iter().next())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u8,
    selection: Selection,
}
pub struct Settings {
    pub selection: Option<Selection>,
    issue: Option<&'static str>,
    path: Option<PathBuf>,
}
impl Settings {
    pub fn load(path: Option<PathBuf>) -> Self {
        let read = || {
            let root = path
                .as_ref()
                .ok_or("Output settings directory unavailable; playback stays paused.")?;
            let dir = crate::cache_files::Directory::open_settings(root)?;
            let Some(file) = dir.file("output.json")? else {
                return Ok(Selection::Desktop);
            };
            if file
                .metadata()
                .map_err(|_| "Output settings unavailable")?
                .len()
                > 4096
            {
                return Err("Invalid output settings preserved; playback stays paused.");
            }
            let prefs: Preferences = serde_json::from_reader(file.take(4097))
                .map_err(|_| "Invalid output settings preserved; playback stays paused.")?;
            if prefs.version != 1 {
                return Err("Newer output settings preserved; playback stays paused.");
            }
            if let Selection::Fidelity { device } = &prefs.selection
                && (device.len() != 64 || !device.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err("Invalid fidelity identity preserved; playback stays paused.");
            }
            Ok(prefs.selection)
        };
        match read() {
            Ok(selection) => Self {
                selection: Some(selection),
                issue: None,
                path,
            },
            Err(issue) => Self {
                selection: None,
                issue: Some(issue),
                path,
            },
        }
    }
    pub fn request(
        &self,
        request: &serde_json::Value,
        devices: &[Device],
    ) -> Result<Selection, &'static str> {
        if let Some(issue) = self.issue {
            return Err(issue);
        }
        match request["mode"].as_str() {
            Some("desktop") => Ok(Selection::Desktop),
            Some("fidelity") => {
                if !alsa_guard::available() {
                    return Err("ALSA submission guard unavailable; fidelity cannot be enabled.");
                }
                let id = request["device"]
                    .as_str()
                    .ok_or("Select a discovered direct ALSA device first.")?;
                if devices.iter().filter(|d| d.id == id).count() != 1 {
                    return Err("Select one currently discovered direct ALSA device.");
                }
                Ok(Selection::Fidelity {
                    device: id.to_owned(),
                })
            }
            _ => Err("Select Desktop or Fidelity output."),
        }
    }
    pub fn set(&mut self, selection: Selection) -> Result<(), &'static str> {
        if let Some(issue) = Self::load(self.path.clone()).issue.or(self.issue) {
            return Err(issue);
        }
        let dir = crate::cache_files::Directory::open_settings(
            self.path.as_ref().ok_or("Output settings unavailable")?,
        )?;
        let bytes = serde_json::to_vec(&Preferences {
            version: 1,
            selection: selection.clone(),
        })
        .map_err(|_| "Output settings unavailable")?;
        dir.atomic("output.json", &bytes)?;
        self.selection = Some(selection);
        Ok(())
    }
    pub fn view(&self, devices: &[Device]) -> serde_json::Value {
        let selected = self.selection.as_ref();
        let missing = matches!(selected,Some(Selection::Fidelity{device}) if devices.iter().filter(|d|d.id==*device).count()!=1);
        serde_json::json!({"selection":selected,"available":self.issue.is_none(),"devices":devices,"device_missing":missing,"qualification":"unverified","ready":false,"fidelity_available":alsa_guard::available(),"observed":null,"source":null,"message":self.issue.unwrap_or(if missing{"Selected device unavailable; no Desktop fallback. Refresh devices or choose another output."}else{"Changing output pauses, releases the old route and preserves position. Explicit Resume is required."})})
    }
}
// No frames are submitted during capability/parameter installation. The native
// handle is opened later under a newly armed guard, and checked again there.
pub fn preflight(device: &Device, source: Format) -> Result<Sample, &'static str> {
    if !crate::alsa_guard::available() {
        return Err("ALSA submission guard is unavailable; fidelity output is disabled.");
    }
    let attempt = |sample: Sample| -> Result<Sample, &'static str> {
        alsa_guard::arm(device.clone(), source, sample)?;
        let pcm = PCM::new(&device.route(), Direction::Playback, true).map_err(|e| {
            if e.errno() == libc::EBUSY {
                "Fidelity device is busy; release it elsewhere and explicitly Resume."
            } else {
                "Fidelity device unavailable; playback stays paused."
            }
        })?;
        let params =
            HwParams::any(&pcm).map_err(|_| "Fidelity hardware capabilities unavailable")?;
        params
            .set_rate_resample(false)
            .map_err(|_| "Fidelity could not disable resampling")?;
        params
            .set_access(Access::RWInterleaved)
            .map_err(|_| "Fidelity requires interleaved integer access")?;
        params
            .set_format(sample.alsa())
            .map_err(|_| "Fidelity device has insufficient native sample precision")?;
        params
            .set_channels(2)
            .map_err(|_| "Fidelity device cannot provide stereo without conversion")?;
        params
            .set_rate(source.rate, ValueOr::Nearest)
            .map_err(|_| "Fidelity device does not support the exact source rate")?;
        pcm.hw_params(&params)
            .map_err(|_| "Fidelity rejected observed hardware parameters")?;
        if alsa_guard::observed().is_none() {
            return Err("Actual ALSA negotiation was not observed; fidelity stays paused.");
        }
        alsa_guard::verify_preflight()?;
        Ok(sample)
    };
    let mut result = Err("Fidelity device does not support the exact source format.");
    for sample in [Sample::S32, Sample::S16] {
        if sample.bits() < source.bits {
            continue;
        }
        result = attempt(sample);
        alsa_guard::disable();
        if result.is_ok()
            || result.as_ref().err().is_some_and(|e| {
                e.contains("busy")
                    || e.contains("unavailable")
                    || e.contains("mapping")
                    || e.contains("layout")
            })
        {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_selection_persists_without_opening_pcm_and_unknown_preferences_are_preserved() {
        let path = std::env::temp_dir().join(format!("qoda-output-{}", uuid::Uuid::new_v4()));
        let mut settings = Settings::load(Some(path.clone()));
        assert_eq!(settings.selection, Some(Selection::Desktop));
        let selection = Selection::Fidelity {
            device: "1".repeat(64),
        };
        settings.set(selection.clone()).unwrap();
        let restored = Settings::load(Some(path.clone()));
        assert_eq!(restored.selection, Some(selection));
        let view = restored.view(&[]);
        assert_eq!(view["device_missing"], true);
        assert_eq!(view["qualification"], "unverified");
        assert!(view["observed"].is_null());
        let future = b"{\"version\":2,\"selection\":{\"mode\":\"desktop\"}}";
        std::fs::write(path.join("output.json"), future).unwrap();
        let mut settings = Settings::load(Some(path.clone()));
        assert!(settings.selection.is_none());
        assert!(settings.set(Selection::Desktop).is_err());
        assert_eq!(std::fs::read(path.join("output.json")).unwrap(), future);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn legacy_readable_config_keeps_output_and_gain_available() {
        use std::os::unix::fs::PermissionsExt;
        let path =
            std::env::temp_dir().join(format!("qoda-legacy-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let identities = path.join("source-identities.json");
        std::fs::write(&identities, b"preserve existing identities").unwrap();
        std::fs::set_permissions(&identities, std::fs::Permissions::from_mode(0o600)).unwrap();
        let settings = Settings::load(Some(path.clone()));
        assert_eq!(settings.view(&[])["available"], true);
        assert_eq!(settings.selection, Some(Selection::Desktop));
        assert_eq!(
            crate::gain::Settings::load(Some(path.clone())).view()["available"],
            true
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::read(&identities).unwrap(),
            b"preserve existing identities"
        );
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn unnamed_driver_pcms_keep_speaker_hdmi_and_headset_labels_distinct() {
        assert_eq!(
            device_name("Sound card", "", "Built-in Speakers (*)", 1),
            "Sound card · Built-in Speakers (*)"
        );
        assert_eq!(
            device_name("Sound card", "", "HDMI2 (*)", 2),
            "Sound card · HDMI2 (*)"
        );
        assert_eq!(
            device_name("Sound card", "Headphones", "Headset (*)", 1),
            "Sound card · Headphones"
        );
        assert_eq!(device_name("", "", "", 3), "ALSA card · Playback 3");
        assert_eq!(
            device_name(&"a".repeat(200), "", "", 0).chars().count(),
            160
        );
    }
    #[test]
    fn refused_source_rate_reports_format_and_explicit_desktop_alternative() {
        let error = explain_preflight(
            "Fidelity device does not support the exact source rate",
            Some(Format {
                rate: 44100,
                bits: 16,
            }),
        );
        assert!(error.contains("44100 Hz"));
        assert!(error.contains("16-bit"));
        assert!(error.contains("Desktop/PipeWire"));
        assert!(error.contains("does not resample"));
        assert_eq!(
            explain_preflight(
                "Fidelity device is busy",
                Some(Format {
                    rate: 48000,
                    bits: 24
                })
            ),
            "Fidelity device is busy"
        );
        assert_eq!(
            explain_preflight(
                "Fidelity device does not support the exact source rate",
                None
            ),
            "Fidelity device does not support the exact source rate"
        );
    }
    #[test]
    fn raw_or_missing_device_names_are_not_accepted_as_fidelity_identity() {
        let settings = Settings::load(Some(
            std::env::temp_dir().join(format!("qoda-output-{}", uuid::Uuid::new_v4())),
        ));
        for device in ["default", "plughw:0", "hw:0", "null", &"0".repeat(64)] {
            assert!(
                settings
                    .request(&serde_json::json!({"mode":"fidelity","device":device}), &[])
                    .is_err()
            );
        }
        assert!(
            settings
                .request(&serde_json::json!({"mode":"automatic"}), &[])
                .is_err()
        );
        std::fs::remove_dir_all(settings.path.unwrap()).unwrap();
    }
}
