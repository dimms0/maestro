#[cfg(windows)]
mod imp {
    use std::sync::{Mutex, OnceLock};

    use windows_sys::Win32::Foundation::{HANDLE, HWND};
    use windows_sys::Win32::Media::Audio::{
        CALLBACK_EVENT, CALLBACK_FUNCTION, CALLBACK_THREAD, CALLBACK_WINDOW, HMIDI, HMIDIOUT,
        MHDR_DONE, MHDR_INQUEUE, MHDR_PREPARED, MIDIERR_NOTREADY, MIDIERR_STILLPLAYING,
        MIDIERR_UNPREPARED, MIDIHDR,
    };
    use windows_sys::Win32::Media::{
        MM_MOM_DONE as MOM_DONE, MMSYSERR_INVALPARAM, MMSYSERR_NOERROR,
    };
    use windows_sys::Win32::System::SystemInformation::GetTickCount64;
    use windows_sys::Win32::System::Threading::SetEvent;
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindow, PostMessageW, PostThreadMessageW};

    #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
    type DWORD = u32;
    #[allow(non_camel_case_types)]
    type DWORD_PTR = usize;

    type CallbackFunction = unsafe extern "C" fn(HMIDIOUT, DWORD, DWORD_PTR, DWORD_PTR, DWORD_PTR);
    unsafe extern "C" fn def_callback(
        _: HMIDIOUT,
        _: DWORD,
        _: DWORD_PTR,
        _: DWORD_PTR,
        _: DWORD_PTR,
    ) {
    }

    #[derive(Clone, Copy)]
    struct CallbackState {
        dummy_device: usize,
        callback_instance: DWORD_PTR,
        callback: CallbackFunction,
        callback_type: DWORD,
    }

    fn callback_state() -> &'static Mutex<CallbackState> {
        static CALLBACK_STATE: OnceLock<Mutex<CallbackState>> = OnceLock::new();
        CALLBACK_STATE.get_or_init(|| {
            Mutex::new(CallbackState {
                dummy_device: 0,
                callback_instance: 0,
                callback: def_callback,
                callback_type: 0,
            })
        })
    }

    unsafe fn check_header(hdr: *mut MIDIHDR, size: u32) -> Result<(), u32> {
        if hdr.is_null() || (size as usize) < std::mem::size_of::<MIDIHDR>() {
            return Err(MMSYSERR_INVALPARAM);
        }
        if unsafe { (*hdr).lpData }.is_null() {
            return Err(MMSYSERR_INVALPARAM);
        }
        Ok(())
    }

    /// # Safety
    /// `IIMidiHdr` must point at a prepared header of at least `IIMidiHdrSize` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn SendDirectLongData(
        IIMidiHdr: *mut MIDIHDR,
        IIMidiHdrSize: u32,
    ) -> u32 {
        if let Err(code) = unsafe { check_header(IIMidiHdr, IIMidiHdrSize) } {
            return code;
        }

        unsafe {
            if (*IIMidiHdr).dwFlags & MHDR_PREPARED == 0 {
                return MIDIERR_UNPREPARED;
            }

            let data = std::slice::from_raw_parts(
                (*IIMidiHdr).lpData as *const u8,
                (*IIMidiHdr).dwBufferLength as usize,
            );
            if !crate::send_long(data) {
                return MIDIERR_NOTREADY;
            }

            (*IIMidiHdr).dwFlags |= MHDR_DONE;
            (*IIMidiHdr).dwFlags &= !MHDR_INQUEUE;
            RunCallbackFunction(MOM_DONE, IIMidiHdr as DWORD_PTR, 0);
        }

        MMSYSERR_NOERROR
    }

    /// # Safety
    /// `MidiHdrData` must point at `MidiHdrDataLen` readable bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn SendDirectLongDataNoBuf(
        MidiHdrData: *mut u8,
        MidiHdrDataLen: DWORD,
    ) -> u32 {
        if MidiHdrData.is_null() || MidiHdrDataLen == 0 {
            return MMSYSERR_INVALPARAM;
        }

        let data = unsafe { std::slice::from_raw_parts(MidiHdrData, MidiHdrDataLen as usize) };
        if crate::send_long(data) {
            MMSYSERR_NOERROR
        } else {
            MIDIERR_NOTREADY
        }
    }

    /// # Safety
    /// `IIMidiHdr` must point at a header of at least `IIMidiHdrSize` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn PrepareLongData(IIMidiHdr: *mut MIDIHDR, IIMidiHdrSize: u32) -> u32 {
        if let Err(code) = unsafe { check_header(IIMidiHdr, IIMidiHdrSize) } {
            return code;
        }

        // Nothing is locked down: the buffer is read once, inside
        // SendDirectLongData, so only the bookkeeping flag matters here.
        unsafe {
            (*IIMidiHdr).dwFlags |= MHDR_PREPARED;
            (*IIMidiHdr).dwFlags &= !MHDR_DONE;
        }

        MMSYSERR_NOERROR
    }

    /// # Safety
    /// `IIMidiHdr` must point at a header of at least `IIMidiHdrSize` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn UnprepareLongData(IIMidiHdr: *mut MIDIHDR, IIMidiHdrSize: u32) -> u32 {
        if let Err(code) = unsafe { check_header(IIMidiHdr, IIMidiHdrSize) } {
            return code;
        }

        unsafe {
            if (*IIMidiHdr).dwFlags & MHDR_INQUEUE != 0 {
                return MIDIERR_STILLPLAYING;
            }
            (*IIMidiHdr).dwFlags &= !MHDR_PREPARED;
        }

        MMSYSERR_NOERROR
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn LoadCustomSoundFontsList(_Directory: *const u16) {}

    #[unsafe(no_mangle)]
    pub extern "C" fn timeGetTime64() -> u64 {
        unsafe { GetTickCount64() }
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn modMessage(
        _uDeviceID: u32,
        _uMsg: u32,
        _dwUser: DWORD_PTR,
        _dwParam1: DWORD_PTR,
        _dwParam2: DWORD_PTR,
    ) -> u32 {
        1
    }

    #[unsafe(no_mangle)]
    #[allow(clippy::missing_safety_doc)]
    pub unsafe extern "C" fn InitializeCallbackFeatures(
        OMHM: HMIDI,
        OMCB: CallbackFunction,
        OMI: DWORD_PTR,
        _OMU: DWORD_PTR,
        OMCM: DWORD,
    ) -> u32 {
        let mut state = callback_state().lock().unwrap();
        state.dummy_device = OMHM as usize;
        state.callback = OMCB;
        state.callback_instance = OMI;
        state.callback_type = OMCM;

        if OMCM == CALLBACK_WINDOW
            && !std::ptr::fn_addr_eq(state.callback, def_callback as CallbackFunction)
            && unsafe { IsWindow(state.callback as HWND) } != 0
        {
            return 0;
        }

        1
    }

    #[unsafe(no_mangle)]
    #[allow(clippy::missing_safety_doc)]
    pub unsafe extern "C" fn RunCallbackFunction(Msg: DWORD, P1: DWORD_PTR, P2: DWORD_PTR) {
        let state = *callback_state().lock().unwrap();

        //We do a match case just to support stuff if needed
        match state.callback_type {
            CALLBACK_FUNCTION => unsafe {
                (state.callback)(
                    state.dummy_device as HMIDIOUT,
                    Msg,
                    P1,
                    P2,
                    state.callback_instance,
                );
            },
            CALLBACK_EVENT => unsafe {
                SetEvent(state.callback as HANDLE);
            },
            CALLBACK_THREAD => {
                // The field holds a thread id rather than a function in this mode.
                if let Ok(p2) = P2.try_into() {
                    unsafe { PostThreadMessageW(state.callback as usize as DWORD, Msg, P1, p2) };
                }
            }
            CALLBACK_WINDOW => {
                if let Ok(p2) = P2.try_into() {
                    unsafe { PostMessageW(state.callback as HWND, Msg, P1, p2) };
                }
            }
            _ => crate::log_info("Callback type was NULL, doing nothing"),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::ffi::{c_char, c_void};
    use std::time::Instant;

    /// # Safety
    /// No safety, it will just read and parse whatever you throw at it without checks
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn SendDirectLongData(ptr: *mut u8, size: u32) -> u32 {
        if ptr.is_null() || size == 0 {
            return 0;
        }

        let long = unsafe { std::slice::from_raw_parts(ptr, size as usize) };
        if crate::send_long(long) { size } else { 0 }
    }

    /// # Safety
    /// No safety, it will just read and parse whatever you throw at it without checks
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn SendDirectLongDataNoBuf(ptr: *mut u8, size: u32) -> u32 {
        unsafe { SendDirectLongData(ptr, size) }
    }

    // There is nothing to lock: the buffer is read once, inside
    // SendDirectLongData, and never held on to.
    #[unsafe(no_mangle)]
    pub extern "C" fn PrepareLongData(_ptr: *mut u8, _size: u32) -> u32 {
        0
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn UnprepareLongData(_ptr: *mut u8, _size: u32) -> u32 {
        0
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn LoadCustomSoundFontsList(_directory: *const c_char) {}

    #[unsafe(no_mangle)]
    pub extern "C" fn timeGetTime64() -> u64 {
        // idgaf it's not used on Linux anyway
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn modMessage(
        _uDeviceID: u32,
        _uMsg: u32,
        _dwUser: usize,
        _dwParam1: usize,
        _dwParam2: usize,
    ) -> u32 {
        1
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn InitializeCallbackFeatures(
        _OMHM: *mut c_void,
        _OMCB: *mut c_void,
        _OMI: usize,
        _OMU: usize,
        _OMCM: u32,
    ) -> u32 {
        1
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn RunCallbackFunction(_Msg: u32, _P1: usize, _P2: usize) {}
}
