macro_rules! define_lib_wrapper {
    ($struct_name:ident, { $( $fn_name:ident : $fn_sig:ty ),* $(,)? }) => {
        pub(crate) struct $struct_name {
            _lib: libloading::Library,
            pub path: Option<std::path::PathBuf>,
            $( pub $fn_name: $fn_sig, )*
        }

        impl $struct_name {
            pub(crate) unsafe fn new(filename: &str) -> Result<Self, String> {
                // fallback to Maestro's copy if the host's doesnt have our functions
                #[cfg(windows)]
                if let Some((lib, path)) = crate::paths::open_already_loaded(filename) {
                    if let Ok(lib) = unsafe { Self::resolve(lib, path) } {
                        return Ok(lib);
                    }
                }

                let (lib, path) = crate::paths::load_own_library(filename).map_err(|e| e.to_string())?;
                unsafe { Self::resolve(lib, path) }
            }

            unsafe fn resolve(
                lib: libloading::Library,
                path: Option<std::path::PathBuf>,
            ) -> Result<Self, String> {
                $(
                    let $fn_name = unsafe {
                        *lib.get::<$fn_sig>(concat!(stringify!($fn_name), "\0").as_bytes())
                            .map_err(|e| format!(
                                "{} in {}: {e}",
                                stringify!($fn_name),
                                path.as_deref().unwrap_or(std::path::Path::new("?")).display(),
                            ))?
                    };
                )*

                Ok(Self {
                    _lib: lib,
                    path,
                    $( $fn_name, )*
                })
            }
        }
    };
}
