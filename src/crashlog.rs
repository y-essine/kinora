//! Kinora: a record of native crashes on Windows.
//!
//! There a crash in C code (GTK, Clapper, GStreamer ...) ends the process
//! with no console, no core file and no line in the log; Windows keeps its
//! own report where users can't read it. On an access violation this writes
//! `%LOCALAPPDATA%\kinora\crash.txt`: where it happened, the registers, and
//! every address on the stack that lies in a loaded module, as
//! `module+offset` (a call chain, give or take stale return addresses). Those
//! resolve against the bundle's DLLs: `objdump -d --start-address=...`.
//!
//! It only writes: the crash goes on as before (the handler passes it on).

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};

    const EXCEPTION_ACCESS_VIOLATION: u32 = 0xC000_0005;
    const EXCEPTION_ILLEGAL_INSTRUCTION: u32 = 0xC000_001D;
    const EXCEPTION_STACK_OVERFLOW: u32 = 0xC000_00FD;
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;

    #[repr(C)]
    struct ExceptionRecord {
        code: u32,
        flags: u32,
        record: *mut ExceptionRecord,
        address: *mut c_void,
        parameters: u32,
        information: [usize; 15],
    }

    #[repr(C)]
    struct ExceptionPointers {
        record: *mut ExceptionRecord,
        context: *mut u8,
    }

    unsafe extern "system" {
        fn SetUnhandledExceptionFilter(filter: Option<Filter>) -> Option<Filter>;
        fn GetModuleHandleExW(flags: u32, address: *const c_void, module: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
        fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
        fn GetCurrentThreadId() -> u32;
    }

    type Filter = unsafe extern "system" fn(*mut ExceptionPointers) -> i32;

    static PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    static PREVIOUS: std::sync::OnceLock<Option<Filter>> = std::sync::OnceLock::new();
    static WRITTEN: AtomicBool = AtomicBool::new(false);

    /// Hand the crash on, as if this filter were not there.
    fn pass_on(info: *mut ExceptionPointers) -> i32 {
        match PREVIOUS.get().copied().flatten() {
            // SAFETY: the filter that was installed before ours.
            Some(previous) => unsafe { previous(info) },
            None => EXCEPTION_CONTINUE_SEARCH,
        }
    }

    /// `module+0xoffset` for an address inside a loaded module.
    fn locate(address: usize) -> Option<String> {
        let mut module = std::ptr::null_mut();
        // SAFETY: plain Win32 calls with valid out pointers.
        unsafe {
            if GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, address as *const c_void, &mut module) == 0 {
                return None;
            }
            let mut name = [0u16; 512];
            let n = GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) as usize;
            let path = String::from_utf16_lossy(&name[..n]);
            let file = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
            Some(format!("{file}+{:#x}", address - module as usize))
        }
    }

    unsafe extern "system" fn handler(info: *mut ExceptionPointers) -> i32 {
        // SAFETY: Windows hands a valid EXCEPTION_POINTERS to the handler.
        let (record, context) = unsafe { (&*(*info).record, (*info).context) };
        let fatal = matches!(
            record.code,
            EXCEPTION_ACCESS_VIOLATION | EXCEPTION_ILLEGAL_INSTRUCTION | EXCEPTION_STACK_OVERFLOW
        );
        // Once: the first fatal exception is the one that matters.
        if !fatal || WRITTEN.swap(true, Ordering::SeqCst) {
            return pass_on(info);
        }
        let Some(path) = PATH.get() else {
            return pass_on(info);
        };
        // x64 CONTEXT: Rax at 0x78, then Rcx, Rdx, Rbx, Rsp, Rbp, Rsi, Rdi,
        // R8..R15 (8 bytes each), Rip at 0xF8.
        // SAFETY: the CONTEXT record is at least 0x100 bytes on x64.
        let reg = |offset: usize| unsafe { (context.add(offset) as *const usize).read_unaligned() };
        let rsp = reg(0x98);
        let mut text = String::new();
        text.push_str(&format!(
            "Kinora {} crashed: exception {:#x} at {:#x} ({}) in thread {}\n",
            env!("CARGO_PKG_VERSION"),
            record.code,
            record.address as usize,
            locate(record.address as usize).unwrap_or_else(|| "no module".into()),
            // SAFETY: no arguments.
            unsafe { GetCurrentThreadId() },
        ));
        if record.code == EXCEPTION_ACCESS_VIOLATION && record.parameters >= 2 {
            let kind = match record.information[0] {
                0 => "reading",
                1 => "writing",
                8 => "executing",
                _ => "accessing",
            };
            text.push_str(&format!("  {kind} {:#x}\n", record.information[1]));
        }
        let names = [
            "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11",
            "r12", "r13", "r14", "r15", "rip",
        ];
        for (i, name) in names.iter().enumerate() {
            let value = reg(0x78 + 8 * i);
            let at = locate(value).map(|m| format!("  ({m})")).unwrap_or_default();
            text.push_str(&format!("  {name:>3} {value:#018x}{at}\n"));
        }
        // The stack, from the crash upwards: every value that points into a
        // module. With rip 0 (a call through a null pointer) the first one
        // is the caller.
        text.push_str("stack (addresses in modules, innermost first):\n");
        let (mut low, mut high) = (0usize, 0usize);
        // SAFETY: valid out pointers.
        unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
        let start = rsp & !7;
        let end = high.min(start + 8 * 2048);
        let mut found = 0;
        if start >= low {
            let mut at = start;
            while at < end && found < 80 {
                // SAFETY: inside this thread's committed stack, from rsp up.
                let value = unsafe { (at as *const usize).read() };
                if let Some(m) = locate(value) {
                    text.push_str(&format!("  [rsp+{:#06x}] {m}\n", at - start));
                    found += 1;
                }
                at += 8;
            }
        }
        if let Ok(mut file) = std::fs::File::create(path) {
            let _ = file.write_all(text.as_bytes());
        }
        log::error!("{text}");
        pass_on(info)
    }

    pub fn install(dir: &std::path::Path) {
        let _ = PATH.set(dir.join("crash.txt"));
        // SAFETY: installing a filter with the right signature; the one it
        // replaces is called after ours.
        let previous = unsafe { SetUnhandledExceptionFilter(Some(handler)) };
        let _ = PREVIOUS.set(previous);
    }
}

/// Record native crashes to `crash.txt` in `dir` (Windows; nothing elsewhere).
pub fn install(dir: &std::path::Path) {
    #[cfg(windows)]
    imp::install(dir);
    #[cfg(not(windows))]
    let _ = dir;
}
