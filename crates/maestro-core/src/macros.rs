macro_rules! define_lib_wrapper {
    ($struct_name:ident, { $( $fn_name:ident : $fn_sig:ty ),* $(,)? }) => {
        pub(crate) struct $struct_name {
            _lib: libloading::Library,
            pub path: Option<std::path::PathBuf>,
            $( pub $fn_name: $fn_sig, )*
        }

        impl $struct_name {
            pub(crate) unsafe fn new(filename: &str) -> Result<Self, libloading::Error> {
                let (lib, path) = crate::paths::load_library(filename)?;

                $(
                    let $fn_name = unsafe {*lib.get::<$fn_sig>(concat!(stringify!($fn_name), "\0").as_bytes())?};
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
