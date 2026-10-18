use maestro_api_rs::{App, ErrorKind, FileRenderOptions, Maestro, RealtimeOptions};

fn maestro() -> Option<Maestro> {
    let path = std::env::var_os("MAESTRO_LIBRARY")?;
    // SAFETY: the variable names a Maestro build.
    Some(unsafe { Maestro::load_from(path) }.expect("MAESTRO_LIBRARY should load"))
}

#[test]
fn the_library_provides_every_interface() {
    let Some(maestro) = maestro() else { return };
    for id in [
        maestro_api_rs::ffi::MAESTRO_CORE_V1,
        maestro_api_rs::ffi::MAESTRO_REALTIME_V1,
        maestro_api_rs::ffi::MAESTRO_FILE_RENDER_V1,
    ] {
        assert!(!maestro.raw_interface(id).is_null(), "{id:?}");
    }
    assert!(maestro.raw_interface(c"maestro.none/1").is_null());
}

// An invalid app_id fails before anything touches the user's settings or the
// audio device, so this is safe to run anywhere.
#[test]
fn an_invalid_app_id_is_rejected_with_a_message() {
    let Some(maestro) = maestro() else { return };
    let app = App::new("not a valid id!");
    let err = maestro
        .open_realtime(&app, RealtimeOptions::new())
        .err()
        .expect("should fail");
    assert_eq!(err.kind(), ErrorKind::InvalidArg);
    assert!(err.message().contains("app_id"), "{err}");

    let err = maestro
        .open_file_render(&app, FileRenderOptions::new("in.mid", "out.wav"))
        .err()
        .expect("should fail");
    assert_eq!(err.kind(), ErrorKind::InvalidArg);
}
