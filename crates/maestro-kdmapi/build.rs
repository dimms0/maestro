fn main() {
    if cfg!(windows) {
        // Building on Windows
        println!("cargo:rustc-cdylib-link-arg=/DEF:crates/maestro-kdmapi/Ordinals.def")
    } else if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // Cross building for Windows
        println!("cargo:rustc-cdylib-link-arg=crates/maestro-kdmapi/Ordinals.def")
    }
}
