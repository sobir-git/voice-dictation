use crate::{Command, Desktop, Output};
use fire_ui_native::WakeHandle;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command as Process, Stdio},
    sync::{mpsc, Arc},
    thread,
};

pub struct Bridge {
    process: Child,
    send: Option<mpsc::SyncSender<Arc<Value>>>,
}
impl Bridge {
    pub fn start(wake: WakeHandle<Desktop>) -> Result<Self, String> {
        let project = std::env::var_os("VOICE_DICTATION_PROJECT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into());
        let python = project.join("venv/bin/python3");
        let mut process = Process::new(python)
            .args(["-m", "speech_to_text.desktop"])
            .env("PYTHONPATH", project.join("src"))
            .current_dir(&project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| e.to_string())?;
        let stdout = process.stdout.take().unwrap();
        let mut stdin = process.stdin.take().unwrap();
        let (send, receive) = mpsc::sync_channel::<Arc<Value>>(32);
        thread::spawn(move || {
            while let Ok(message) = receive.recv() {
                if writeln!(stdin, "{message}")
                    .and_then(|_| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        thread::spawn(move || {
            // The adapter is local and each JSON record is bounded before posting.
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                use std::io::Read;
                match reader
                    .by_ref()
                    .take(1_048_577)
                    .read_until(b'\n', &mut bytes)
                {
                    Ok(0) | Err(_) => break,
                    Ok(n) if n > 1_048_576 => break,
                    Ok(_) => {
                        if let Ok(message) = serde_json::from_slice(&bytes) {
                            let _ = wake.post(Command::Backend(Arc::new(message)));
                        }
                    }
                }
            }
            let _ = wake.post(Command::Backend(Arc::new(json!({"type":"disconnected"}))));
        });
        Ok(Self {
            process,
            send: Some(send),
        })
    }
    pub fn send(&self, value: Arc<Value>) -> Result<(), String> {
        self.send
            .as_ref()
            .ok_or("Desktop adapter is closed.")?
            .try_send(value)
            .map_err(|_| "Speech service is busy or disconnected. Try again.".into())
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.send.take();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if self.process.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

pub fn demo(output: Output, wake: &WakeHandle<Desktop>, state: &mut Value) {
    let mut value = match output {
        Output::Start => {
            let _ = wake.post(Command::Backend(Arc::new(json!({"type":"config","config":{
                "audio":{"pipewire_node":"","device":"default","preprocess":true},
                "transcription":{"model":"base.en","compute_type":"int8","language":"en","beam_size":1,"vad_filter":true},
                "input":{"trigger_key":"KEY_RIGHTCTRL"},"output":{"method":"auto","add_space":true},
                "notifications":{"enabled":true,"audio_feedback":true},"logging":{"level":"INFO"}
            },"microphones":[{"name":"desk-mic","description":"Desk microphone"}]}))));
            let _ = wake.post(Command::Backend(Arc::new(json!({"type":"transcription","text":"A thought worth keeping.\n\nLet's make the next version simpler, faster, and a pleasure to use.","duration":3.2}))));
            state.clone()
        }
        Output::Request(value) => {
            eprintln!("demo request: {value}");
            match value["cmd"].as_str().unwrap_or("") {
                "toggle_listening" => {
                    state["listening"] = json!(!state["listening"].as_bool().unwrap_or(true));
                    state.clone()
                }
                "history" => {
                    json!({"type":"history","search":value["search"].as_str().unwrap_or(""),"items":[
                        {"id":3,"timestamp":"Today, 11:42","text":"A thought worth keeping.\n\nLet's make the next version simpler, faster, and a pleasure to use."},
                        {"id":2,"timestamp":"Today, 10:18","text":"Remember to leave room for the unexpected. Good tools should get out of the way."},
                        {"id":1,"timestamp":"Yesterday, 17:06","text":"Three ideas for tomorrow: finish the prototype, take a walk, and call home."}
                    ]})
                }
                "save_config" => json!({"type":"config_reloaded"}),
                "clear_error" => {
                    state["last_error"] = json!("");
                    state.clone()
                }
                "capture_hotkey" => json!({"type":"hotkey","key":"KEY_F16"}),
                "test_microphone" => {
                    json!({"type":"microphone_test","level":0.65,"message":"Microphone signal detected. Test complete.","done":true})
                }
                "diagnostics" => {
                    json!({"type":"diagnostics","text":"Voice Dictation\n\nSpeech service connected\nMicrophone: System default\nModel: base.en / int8\nText output: xdotool\n\nAll required tools are available.","logs":"11:42  Dictation completed\n11:42  Audio capture ready"})
                }
                "set_log_level" => {
                    state["log_level"] = value["value"].clone();
                    state.clone()
                }
                _ => state.clone(),
            }
        }
    };
    if value["type"] == "history" {
        let search = value["search"].as_str().unwrap_or("").to_lowercase();
        if let Some(items) = value["items"].as_array_mut() {
            items.retain(|i| {
                i["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&search)
            });
        }
    }
    let _ = wake.post(Command::Backend(Arc::new(value)));
}
