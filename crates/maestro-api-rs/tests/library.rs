use maestro_api_rs::{ErrorKind, Maestro, OpenOptions, Version};

fn maestro() -> Option<Maestro> {
    let path = std::env::var_os("MAESTRO_LIBRARY")?;
    // SAFETY: the variable names a Maestro build.
    Some(unsafe { Maestro::load_from(path) }.expect("MAESTRO_LIBRARY should load"))
}

#[test]
fn the_library_loads_and_is_compatible() {
    let Some(maestro) = maestro() else { return };
    assert!(maestro.version().is_compatible());
    assert_eq!(maestro.version().major, Version::REQUIRED.major);
}

#[test]
fn host_time_moves_forward() {
    let Some(maestro) = maestro() else { return };
    let before = maestro.now();
    let later = maestro.after(std::time::Duration::from_millis(10));
    assert!(later > before);
    assert!(maestro.now() >= before);
}

// An invalid app_id fails before anything touches the user's settings or the
// audio device, so this is safe to run anywhere.
#[test]
fn an_invalid_app_id_is_rejected_with_a_message() {
    let Some(maestro) = maestro() else { return };
    let err = maestro
        .open(OpenOptions::new("not a valid id!"))
        .err()
        .expect("should fail");
    assert_eq!(err.kind(), ErrorKind::InvalidArg);
    assert!(err.message().contains("app_id"), "{err}");
}
