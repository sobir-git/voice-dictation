use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Config {
    pub data: Value,
    pub path: PathBuf,
    pub data_dir: PathBuf,
}
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME must be set")
}
pub fn expand(path: &str) -> PathBuf {
    path.strip_prefix("~/")
        .map(|s| home().join(s))
        .unwrap_or_else(|| path.into())
}
pub fn defaults() -> Value {
    json!({
        "audio":{"sample_rate":16000,"format":"S16_LE","channels":1,"temp_file":"/tmp/stt_recording.wav","device":"default","pipewire_node":"","preprocess":true},
        "transcription":{"model":"parakeet-unified-en-0.6b","compute_type":"int8","language":"en","beam_size":1,"vad_filter":true},
        "performance":{"profile":"standard","threads":0,"by_model":{}},
        "input":{"trigger_key":"KEY_F16"},"output":{"method":"auto","add_space":true,"type_interval":0.0},
        "notifications":{"enabled":true,"audio_feedback":true},
        "logging":{"level":"INFO","file":"~/.local/share/speech-to-text/app.log","max_size_mb":10},
        "ui":{"cursor_indicator":false}
    })
}
pub fn merge(target: &mut Value, source: &Value) {
    match (target, source) {
        (Value::Object(dst), Value::Object(src)) => {
            for (key, value) in src {
                merge(dst.entry(key).or_insert(Value::Null), value);
            }
        }
        (dst, src) => *dst = src.clone(),
    }
}
pub fn key_code(name: &str) -> Option<evdev::KeyCode> {
    (0..=0x2ff)
        .map(evdev::KeyCode::new)
        .find(|k| format!("{k:?}") == name)
}
impl Config {
    pub fn load() -> Result<Self> {
        Self::at(&home())
    }
    pub fn at(root: &Path) -> Result<Self> {
        let path = root.join(".config/speech-to-text/config.yaml");
        let mut data = defaults();
        if path.exists() {
            let loaded: Value = serde_yaml::from_str(&fs::read_to_string(&path)?)?;
            if !loaded.is_null() {
                if !loaded.is_object() {
                    bail!("Config must contain named sections")
                }
                merge(&mut data, &loaded);
            }
        }
        Self::validate(&data)?;
        Ok(Self {
            data,
            path,
            data_dir: root.join(".local/share/speech-to-text"),
        })
    }
    pub fn string(&self, section: &str, key: &str) -> &str {
        self.data[section][key].as_str().unwrap_or("")
    }
    pub fn flag(&self, section: &str, key: &str) -> bool {
        self.data[section][key].as_bool().unwrap_or(false)
    }
    pub fn number(&self, section: &str, key: &str) -> u64 {
        self.data[section][key].as_u64().unwrap_or(0)
    }
    pub fn changed(&self, changes: &Value) -> Result<Self> {
        if !changes.is_object() {
            bail!("Expected settings sections")
        }
        let mut next = self.clone();
        merge(&mut next.data, changes);
        Self::validate(&next.data)?;
        Ok(next)
    }
    pub fn save(&self) -> Result<()> {
        Self::validate(&self.data)?;
        let directory = self.path.parent().context("Invalid config path")?;
        fs::create_dir_all(directory)?;
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        file.write_all(serde_yaml::to_string(&self.data)?.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(&self.path)?;
        fs::File::open(directory)?.sync_all()?;
        Ok(())
    }
    pub fn validate(d: &Value) -> Result<()> {
        crate::optimization::validate(d)?;
        for section in [
            "audio",
            "transcription",
            "input",
            "output",
            "logging",
            "notifications",
            "ui",
        ] {
            if !d[section].is_object() {
                bail!("Config section {section} must be a mapping")
            }
        }
        for (s, k, lo, hi) in [
            ("audio", "sample_rate", 8000, 192000),
            ("audio", "channels", 1, 8),
            ("transcription", "beam_size", 1, 10),
            ("logging", "max_size_mb", 1, 1000),
        ] {
            if !d[s][k].as_u64().is_some_and(|n| (lo..=hi).contains(&n)) {
                bail!("{s}.{k} must be an integer between {lo} and {hi}")
            }
        }
        for (s, k) in [
            ("audio", "preprocess"),
            ("transcription", "vad_filter"),
            ("output", "add_space"),
            ("notifications", "enabled"),
            ("notifications", "audio_feedback"),
            ("ui", "cursor_indicator"),
        ] {
            if !d[s][k].is_boolean() {
                bail!("{s}.{k} must be true or false")
            }
        }
        for (s, k) in [
            ("audio", "temp_file"),
            ("audio", "device"),
            ("audio", "format"),
            ("transcription", "model"),
            ("transcription", "compute_type"),
            ("logging", "file"),
        ] {
            if d[s][k].as_str().is_none_or(|v| v.trim().is_empty()) {
                bail!("{s}.{k} must be a nonempty string")
            }
        }
        if key_code(d["input"]["trigger_key"].as_str().unwrap_or("")).is_none() {
            bail!("Unknown hotkey")
        }
        if ![
            "auto", "xdotool", "ydotool", "dotool", "wtype", "xclip", "none",
        ]
        .contains(&d["output"]["method"].as_str().unwrap_or(""))
        {
            bail!("Unknown output method")
        }
        if !d["output"]["type_interval"]
            .as_f64()
            .is_some_and(|n| n.is_finite() && (0.0..=1.0).contains(&n))
        {
            bail!("Typing interval must be between 0 and 1 seconds")
        }
        if !d["audio"]["pipewire_node"].is_string() {
            bail!("Microphone node must be a string")
        }
        if !d["transcription"]["language"].is_null() && !d["transcription"]["language"].is_string()
        {
            bail!("Language must be a code or null")
        }
        if !["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"]
            .contains(&d["logging"]["level"].as_str().unwrap_or(""))
        {
            bail!("Unknown logging level")
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserve_unknown_settings_and_reject_invalid_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let original = Config::at(root.path())
            .unwrap()
            .changed(&json!({"custom":{"keep":42},"input":{"trigger_key":"KEY_RIGHTCTRL"}}))
            .unwrap();
        original.save().unwrap();
        let bytes = fs::read(&original.path).unwrap();
        assert!(original
            .changed(&json!({"audio":{"channels":true}}))
            .is_err());
        assert_eq!(fs::read(&original.path).unwrap(), bytes);
        let next = Config::at(root.path())
            .unwrap()
            .changed(&json!({"ui":{"cursor_indicator":true}}))
            .unwrap();
        next.save().unwrap();
        assert_eq!(Config::at(root.path()).unwrap().data["custom"]["keep"], 42);
    }
}
