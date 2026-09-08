use crate::ipc::{ensure_daemon, socket_path};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    io::{BufRead, Read, Write},
    os::unix::net::UnixStream,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};
fn emit(value: &Value) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(out, "{value}");
}
pub fn run() -> Result<()> {
    // A newly opened desktop explicitly resumes the service after a tray quit.
    match std::fs::remove_file(crate::ipc::stopped_path()) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let (send, receive) = mpsc::sync_channel::<Value>(32);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let worker = thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            if crate::ipc::stopped_path().exists() {
                emit(&json!({"type":"shutdown"}));
                return;
            }
            if let Err(e) = ensure_daemon() {
                emit(&json!({"type":"error","message":e.to_string()}));
                for _ in 0..20 {
                    if flag.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                continue;
            }
            if flag.load(Ordering::Relaxed) {
                return;
            }
            let Ok(mut stream) = UnixStream::connect(socket_path()) else {
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
            emit(&json!({"type":"connected"}));
            let _ = writeln!(stream, "{}", json!({"cmd":"get_config"}));
            let mut pending = Vec::new();
            let mut block = [0; 8192];
            while !flag.load(Ordering::Relaxed) {
                let mut failed = false;
                for message in receive.try_iter() {
                    if writeln!(stream, "{message}").is_err() {
                        emit(
                            &json!({"type":"error","message":"Speech service disconnected before the request was delivered."}),
                        );
                        failed = true;
                        break;
                    }
                }
                if failed {
                    break;
                }
                match stream.read(&mut block) {
                    Ok(0) => break,
                    Ok(n) => {
                        pending.extend_from_slice(&block[..n]);
                        if pending.len() > 1_048_576 {
                            break;
                        }
                        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                            let line: Vec<_> = pending.drain(..=end).collect();
                            if let Ok(message) = serde_json::from_slice::<Value>(&line) {
                                emit(&message);
                            }
                        }
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => break,
                }
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
            if crate::ipc::stopped_path().exists() {
                emit(&json!({"type":"shutdown"}));
                return;
            }
            emit(&json!({"type":"disconnected"}));
            // Do not replay a save, toggle or capture command after reconnecting.
            while receive.try_recv().is_ok() {}
        }
    });
    let mut input = std::io::stdin().lock();
    loop {
        let mut line = Vec::new();
        let n = input.by_ref().take(65537).read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        if n > 65536 {
            emit(&json!({"type":"error","message":"Desktop request exceeds 64 KiB"}));
            break;
        }
        match serde_json::from_slice::<Value>(&line) {
            Ok(value) if value.is_object() => {
                if send.try_send(value).is_err() {
                    emit(&json!({"type":"error","message":"Speech service is busy; try again."}));
                }
            }
            _ => emit(&json!({"type":"error","message":"Expected a JSON object"})),
        }
    }
    stop.store(true, Ordering::Relaxed);
    drop(send);
    let _ = worker.join();
    Ok(())
}
