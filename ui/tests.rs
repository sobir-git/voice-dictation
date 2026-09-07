use super::*;

fn settle(ui: &mut Ui<Desktop>) {
    for _ in 0..30 {
        ui.pump(4096, |_| {}, |_| {});
        ui.layout(&mut TestText);
        if !ui.next_work().ready {
            return;
        }
    }
    panic!("Desktop did not settle");
}
fn send(ui: &mut Ui<Desktop>, command: Command) {
    assert!(ui.send(command).is_ok());
    settle(ui);
}
fn setup(size: Size) -> Ui<Desktop> {
    let mut ui = Ui::new(Desktop::new(), size, Limits::default()).unwrap();
    settle(&mut ui);
    send(
        &mut ui,
        Command::Backend(Arc::new(json!({"type":"config","config":{
        "audio":{"preprocess":true,"pipewire_node":""},"transcription":{"model":"base.en","language":"en"},
        "logging":{"level":"INFO"}
    },"microphones":[]}))),
    );
    send(
        &mut ui,
        Command::Backend(Arc::new(
            json!({"type":"state","listening":true,"model_ready":true,"log_level":"INFO"}),
        )),
    );
    ui
}
#[test]
fn draft_settings_survive_refresh_and_wait_for_acknowledgment() {
    let mut ui = setup(Size::new(1000., 860.));
    send(&mut ui, Command::Field(5));
    assert!(ui.root().dirty);
    assert_eq!(ui.root().config["audio"]["preprocess"], false);
    send(
        &mut ui,
        Command::Backend(Arc::new(
            json!({"type":"config","config":{"audio":{"preprocess":true},"logging":{"level":"DEBUG"}}}),
        )),
    );
    assert_eq!(ui.root().config["audio"]["preprocess"], false);
    send(
        &mut ui,
        Command::Backend(Arc::new(json!({"type":"config_reloaded"}))),
    );
    assert!(
        ui.root().dirty,
        "Unrelated logging changes must not discard settings"
    );
    send(&mut ui, Command::Action(3));
    assert!(ui.root().saving);
    assert!(ui.root().dirty);
    send(
        &mut ui,
        Command::Backend(Arc::new(json!({"type":"config_reloaded"}))),
    );
    assert!(!ui.root().saving);
    assert!(!ui.root().dirty);
}
#[test]
fn a_disconnected_save_can_be_retried() {
    let mut ui = setup(Size::new(1000., 860.));
    send(&mut ui, Command::Field(5));
    send(&mut ui, Command::Action(3));
    send(
        &mut ui,
        Command::Backend(Arc::new(json!({"type":"disconnected"}))),
    );
    assert!(!ui.root().saving);
    assert!(ui.root().dirty);
    assert!(!ui.root().connected);
}
#[test]
fn settings_remain_reachable_in_a_minimum_window() {
    let mut ui = setup(Size::new(420., 360.));
    send(&mut ui, Command::Page(2));
    assert!(!ui
        .semantics()
        .iter()
        .any(|n| n.semantics.role == Role::TextInput));
    ui.dispatch(
        Input::Scroll {
            position: Point::new(340., 150.),
            delta: Point::new(0., -2000.),
        },
        &mut TestText,
    );
    settle(&mut ui);
    assert!(ui
        .semantics()
        .iter()
        .any(|n| n.semantics.role == Role::TextInput));
}
#[test]
fn stale_history_results_do_not_replace_the_current_search() {
    let mut ui = setup(Size::new(1000., 860.));
    send(
        &mut ui,
        Command::Search(EditorOutput::Changed {
            revision: 1,
            text: Arc::from("new"),
        }),
    );
    send(
        &mut ui,
        Command::Backend(Arc::new(
            json!({"type":"history","search":"old","items":[{"text":"old"}]}),
        )),
    );
    assert!(ui.root().history.is_empty());
    send(
        &mut ui,
        Command::Backend(Arc::new(
            json!({"type":"history","search":"new","items":[{"text":"new"}]}),
        )),
    );
    assert_eq!(ui.root().history[0]["text"], "new");
}

#[test]
fn closing_waits_for_settings_and_failed_saves_keep_the_window_open() {
    let mut ui = setup(Size::new(1000., 860.));
    send(&mut ui, Command::Field(5));
    send(&mut ui, Command::Action(3));
    assert!(!ui.request_close());
    settle(&mut ui);
    assert!(ui.root().closing);
    send(
        &mut ui,
        Command::Backend(Arc::new(
            json!({"type":"error","message":"Model unavailable"}),
        )),
    );
    assert!(!ui.root().closing);
    assert!(ui.root().dirty);
    send(&mut ui, Command::Action(3));
    assert!(!ui.request_close());
    send(
        &mut ui,
        Command::Backend(Arc::new(json!({"type":"config_reloaded"}))),
    );
    assert!(ui.request_close());
    assert!(!ui.root().dirty);
}
