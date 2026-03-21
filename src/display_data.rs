#![cfg(target_os = "macos")]

// ── Public types ──────────────────────────────────────────────────────────────

pub struct DisplayInfo {
    pub display_index: i64,
    pub width_px: i64,
    pub height_px: i64,
    pub refresh_rate: Option<f64>,
    pub width_mm: Option<i64>,
    pub height_mm: Option<i64>,
    pub is_main: bool,
}

// ── FFI ───────────────────────────────────────────────────────────────────────

#[allow(non_camel_case_types)]
#[allow(non_upper_case_globals)]
mod ffi {
    pub type CGDirectDisplayID = u32;
    pub type CGDisplayCount = u32;
    pub type CGError = i32;

    pub const kCGNullDirectDisplay: CGDirectDisplayID = 0;
    pub const kCGErrorSuccess: CGError = 0;

    #[repr(C)]
    pub struct CGSize {
        pub width: f64,
        pub height: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub fn CGGetActiveDisplayList(
            max_displays: CGDisplayCount,
            active_displays: *mut CGDirectDisplayID,
            display_count: *mut CGDisplayCount,
        ) -> CGError;

        pub fn CGDisplayPixelsWide(display: CGDirectDisplayID) -> usize;
        pub fn CGDisplayPixelsHigh(display: CGDirectDisplayID) -> usize;
        pub fn CGDisplayScreenSize(display: CGDirectDisplayID) -> CGSize;
        pub fn CGMainDisplayID() -> CGDirectDisplayID;

        // returns the current display mode — we need it for refresh rate
        pub fn CGDisplayCopyDisplayMode(
            display: CGDirectDisplayID,
        ) -> *mut std::ffi::c_void; // CGDisplayModeRef (opaque)

        pub fn CGDisplayModeGetRefreshRate(mode: *mut std::ffi::c_void) -> f64;
        pub fn CGDisplayModeRelease(mode: *mut std::ffi::c_void);
    }
}

// ── Collection ────────────────────────────────────────────────────────────────

pub fn collect_displays() -> anyhow::Result<Vec<DisplayInfo>> {
    const MAX_DISPLAYS: u32 = 32;
    let mut display_ids = [ffi::kCGNullDirectDisplay; 32];
    let mut count: ffi::CGDisplayCount = 0;

    let err = unsafe {
        ffi::CGGetActiveDisplayList(MAX_DISPLAYS, display_ids.as_mut_ptr(), &mut count)
    };
    anyhow::ensure!(
        err == ffi::kCGErrorSuccess,
        "CGGetActiveDisplayList failed: {err}"
    );

    let main_id = unsafe { ffi::CGMainDisplayID() };
    let mut displays = Vec::with_capacity(count as usize);

    for (index, &display_id) in display_ids[..count as usize].iter().enumerate() {
        let width_px = unsafe { ffi::CGDisplayPixelsWide(display_id) } as i64;
        let height_px = unsafe { ffi::CGDisplayPixelsHigh(display_id) } as i64;

        let physical = unsafe { ffi::CGDisplayScreenSize(display_id) };
        let width_mm = if physical.width > 0.0 {
            Some(physical.width as i64)
        } else {
            None
        };
        let height_mm = if physical.height > 0.0 {
            Some(physical.height as i64)
        } else {
            None
        };

        let refresh_rate = unsafe {
            let mode = ffi::CGDisplayCopyDisplayMode(display_id);
            if mode.is_null() {
                None
            } else {
                let rate = ffi::CGDisplayModeGetRefreshRate(mode);
                ffi::CGDisplayModeRelease(mode);
                // 0.0 means the display doesn't report a rate (e.g. some external monitors)
                if rate > 0.0 { Some(rate) } else { None }
            }
        };

        displays.push(DisplayInfo {
            display_index: index as i64,
            width_px,
            height_px,
            refresh_rate,
            width_mm,
            height_mm,
            is_main: display_id == main_id,
        });
    }

    Ok(displays)
}
