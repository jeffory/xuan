//! Windows: the "app mode" and accent colour from the current user's registry.
//!
//! The colour conversions are pure functions, compiled into the Linux tests too; only
//! [`read`] touches the registry.

use egui::Color32;

/// `HKCU\…\Themes\Personalize\AppsUseLightTheme`: 0 means dark apps, anything else light.
pub(in crate::app) fn apps_use_light_theme(value: u32) -> bool {
    value != 0
}

/// `HKCU\Software\Microsoft\Windows\DWM\AccentColor` is `0xAABBGGRR`: red in the low byte.
pub(in crate::app) fn accent_from_abgr(value: u32) -> Color32 {
    let [r, g, b, _a] = value.to_le_bytes();
    Color32::from_rgb(r, g, b)
}

/// `HKCU\Software\Microsoft\Windows\DWM\ColorizationColor` is `0xAARRGGBB`: blue in the low byte.
pub(in crate::app) fn accent_from_argb(value: u32) -> Color32 {
    let [b, g, r, _a] = value.to_le_bytes();
    Color32::from_rgb(r, g, b)
}

/// The theme of the signed-in user.
#[cfg(windows)]
pub(in crate::app) fn read() -> super::SystemTheme {
    const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
    const DWM: &str = r"Software\Microsoft\Windows\DWM";
    super::SystemTheme {
        dark: registry::dword(PERSONALIZE, "AppsUseLightTheme").map(|v| !apps_use_light_theme(v)),
        accent: registry::dword(DWM, "AccentColor")
            .map(accent_from_abgr)
            .or_else(|| registry::dword(DWM, "ColorizationColor").map(accent_from_argb)),
    }
}

#[cfg(windows)]
mod registry {
    use windows_sys::Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A `REG_DWORD` value under `HKEY_CURRENT_USER`, or `None` when it is missing or of
    /// another type.
    pub(super) fn dword(subkey: &str, value: &str) -> Option<u32> {
        let (subkey, value) = (wide(subkey), wide(value));
        let mut data: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: `subkey` and `value` are NUL-terminated UTF-16 strings that outlive the call.
        // `RRF_RT_REG_DWORD` makes the call fail unless the value is a 4-byte DWORD, so it
        // writes at most `size` (4) bytes into `data`. The type out-pointer may be null.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&raw mut data).cast(),
                &mut size,
            )
        };
        (status == ERROR_SUCCESS).then_some(data)
    }
}
