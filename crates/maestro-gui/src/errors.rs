use crate::{ErrorState, MainWindow, SlintErrorDialog};
use slint::{ComponentHandle, Global, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};

static UI: OnceLock<slint::Weak<MainWindow>> = OnceLock::new();
static NEXT_ID: AtomicI32 = AtomicI32::new(1);

pub fn init(ui: &MainWindow) {
    let model = Rc::new(VecModel::<SlintErrorDialog>::default());
    ErrorState::get(ui).set_dialogs(ModelRc::from(model));

    let weak = ui.as_weak();
    ErrorState::get(ui).on_dismiss(move |id| {
        if let Some(ui) = weak.upgrade() {
            with_model(&ui, |model| {
                if let Some(pos) =
                    (0..model.row_count()).find(|&i| model.row_data(i).map(|d| d.id) == Some(id))
                {
                    model.remove(pos);
                }
            });
        }
    });

    let _ = UI.set(ui.as_weak());
}

pub fn report(title: impl Into<String>, message: impl Into<String>) {
    let title = title.into();
    let message = message.into();
    eprintln!("[maestro-gui] {title}: {message}");

    let Some(weak) = UI.get().cloned() else {
        return;
    };
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = weak.upgrade() {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            with_model(&ui, |model| {
                model.push(SlintErrorDialog {
                    id,
                    title: SharedString::from(title),
                    message: SharedString::from(message),
                });
            });
        }
    });
}

fn with_model(ui: &MainWindow, f: impl FnOnce(&VecModel<SlintErrorDialog>)) {
    let dialogs = ErrorState::get(ui).get_dialogs();
    if let Some(model) = dialogs
        .as_any()
        .downcast_ref::<VecModel<SlintErrorDialog>>()
    {
        f(model);
    }
}
