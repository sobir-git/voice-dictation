use crate::{config::Config, process};
use anyhow::{bail, Result};
use std::{path::PathBuf, process::Command, time::Duration};
pub fn ydotool_socket() -> Option<PathBuf> {
    [
        std::env::var("YDOTOOL_SOCKET").unwrap_or_default(),
        format!("/run/user/{}/.ydotool_socket", unsafe { libc::getuid() }),
        "/tmp/.ydotool_socket".into(),
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|p| p.metadata().is_ok_and(|m| m.file_type().is_socket()))
}
use std::os::unix::fs::FileTypeExt;
pub fn resolve(method: &str) -> String {
    if method != "auto" {
        return method.into();
    }
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland");
    let order = if wayland {
        ["wtype", "dotool", "ydotool", "none"]
    } else {
        ["xdotool", "dotool", "ydotool", "wtype"]
    };
    for method in order {
        if !process::exists(method) {
            continue;
        }
        let ok = match method {
            "xdotool" => process::run(
                Command::new(method).arg("getdisplaygeometry"),
                None,
                Duration::from_secs(2),
            )
            .is_ok(),
            "wtype" => {
                process::run(Command::new(method).arg(""), None, Duration::from_secs(2)).is_ok()
            }
            "ydotool" => ydotool_socket().is_some(),
            "dotool" => std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/uinput")
                .is_ok(),
            _ => false,
        };
        if ok {
            return method.into();
        }
    }
    "none".into()
}
pub fn deliver(text: &str, config: &Config, method: &str) -> Result<()> {
    if config.string("output", "method") == "none" {
        return Ok(());
    }
    if method == "none" {
        bail!("No working text output tool. Text is saved in History.")
    }
    let out = format!(
        "{text}{}",
        if config.flag("output", "add_space") {
            " "
        } else {
            ""
        }
    );
    let interval = config.data["output"]["type_interval"]
        .as_f64()
        .unwrap_or(0.);
    let mut delay = (interval * 1000.) as u64;
    let mut cmd = Command::new(method);
    let input = match method {
        "xdotool" => {
            if !out.is_ascii() {
                delay = delay.max(12)
            }
            cmd.args([
                "type",
                "--clearmodifiers",
                "--delay",
                &delay.to_string(),
                "--file",
                "-",
            ]);
            Some(out.as_bytes().to_vec())
        }
        "ydotool" => {
            cmd.env(
                "YDOTOOL_SOCKET",
                ydotool_socket().ok_or_else(|| {
                    anyhow::anyhow!("ydotoold is unavailable. Text is saved in History.")
                })?,
            )
            .args(["type", &format!("--key-delay={delay}"), "--file", "-"]);
            Some(out.as_bytes().to_vec())
        }
        "wtype" => {
            cmd.args(["-d", &delay.to_string(), "--", &out]);
            None
        }
        "dotool" => Some(dotool_input(&out).into_bytes()),
        "xclip" => {
            cmd.args(["-selection", "clipboard"]);
            Some(out.as_bytes().to_vec())
        }
        _ => bail!("Unknown output tool"),
    };
    process::run(
        &mut cmd,
        input,
        Duration::from_secs_f64(
            (out.chars().count() as f64 * (delay as f64 / 1000.) * 2. + 5.).max(10.),
        ),
    )
    .map_err(|_| anyhow::anyhow!("{method} could not deliver text. Text is saved in History."))?;
    Ok(())
}
pub fn dotool_input(text: &str) -> String {
    format!(
        "{}\n",
        text.split('\n')
            .map(|s| format!("type {s}"))
            .collect::<Vec<_>>()
            .join("\nkey enter\n")
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dictation_cannot_inject_dotool_commands() {
        assert_eq!(
            dotool_input("hello\nkey ctrl+a"),
            "type hello\nkey enter\ntype key ctrl+a\n"
        );
    }
}
