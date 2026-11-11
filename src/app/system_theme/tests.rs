use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use egui::Color32;

use super::{SystemTheme, Watcher, windows};

#[test]
fn windows_registry_colours_are_decoded_in_the_right_byte_order() {
    // AccentColor 0xFFD77800 is ABGR: red 0x00, green 0x78, blue 0xD7 (Windows' default blue).
    assert_eq!(
        windows::accent_from_abgr(0xFFD7_7800),
        Color32::from_rgb(0x00, 0x78, 0xD7)
    );
    // ColorizationColor 0xC40078D7 is ARGB: the same blue.
    assert_eq!(
        windows::accent_from_argb(0xC400_78D7),
        Color32::from_rgb(0x00, 0x78, 0xD7)
    );
    assert_eq!(
        windows::accent_from_abgr(0x0011_2233),
        Color32::from_rgb(0x33, 0x22, 0x11)
    );
    assert!(!windows::apps_use_light_theme(0));
    assert!(windows::apps_use_light_theme(1));
}

#[test]
fn missing_values_fall_back_field_by_field() {
    let portal = SystemTheme {
        dark: Some(false),
        accent: None,
    };
    let kde = SystemTheme {
        dark: Some(true),
        accent: Some(Color32::RED),
    };
    assert_eq!(
        portal.or(kde),
        SystemTheme {
            dark: Some(false),
            accent: Some(Color32::RED)
        }
    );
}

#[test]
fn the_watcher_reports_a_changed_theme() {
    let shared = Arc::new(Mutex::new(SystemTheme {
        dark: Some(true),
        accent: None,
    }));
    let source = Arc::clone(&shared);
    let mut watcher = Watcher::spawn(
        Box::new(move || *source.lock().unwrap()),
        Duration::from_millis(5),
        Duration::from_secs(5),
        None,
    );
    assert_eq!(watcher.poll(), (*shared.lock().unwrap(), false));
    let light = SystemTheme {
        dark: Some(false),
        accent: Some(Color32::from_rgb(1, 2, 3)),
    };
    *shared.lock().unwrap() = light;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (theme, changed) = watcher.poll();
        if changed {
            assert_eq!(theme, light);
            break;
        }
        assert!(Instant::now() < deadline, "the change never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use egui::Color32;
    use zbus::zvariant::Value;

    use super::super::{SystemTheme, linux::*};

    #[test]
    fn portal_replies_in_both_shapes() {
        // `ReadOne` returns the value; the older `Read` wraps it in one more variant.
        assert_eq!(portal_color_scheme(&Value::U32(1)), Some(true));
        assert_eq!(portal_color_scheme(&Value::U32(2)), Some(false));
        assert_eq!(portal_color_scheme(&Value::U32(0)), None);
        assert_eq!(
            portal_color_scheme(&Value::Value(Box::new(Value::U32(1)))),
            Some(true)
        );
        assert_eq!(portal_color_scheme(&Value::from("dark")), None);

        let accent = Value::from((0.2_f64, 0.5_f64, 1.0_f64));
        assert_eq!(
            portal_accent(&accent),
            Some(Color32::from_rgb(51, 128, 255))
        );
        assert_eq!(
            portal_accent(&Value::Value(Box::new(accent))),
            Some(Color32::from_rgb(51, 128, 255))
        );
        // Out of range means "no accent set".
        assert_eq!(
            portal_accent(&Value::from((-1.0_f64, -1.0_f64, -1.0_f64))),
            None
        );
        assert_eq!(portal_accent(&Value::from((0.1_f64, 0.2_f64))), None);
        assert_eq!(portal_accent(&Value::U32(3)), None);
    }

    #[test]
    fn gsettings_strings() {
        use crate::app::window_theme::gsettings_value;
        assert_eq!(
            gsettings_value("'prefer-dark'\n").as_deref(),
            Some("prefer-dark")
        );
        assert_eq!(
            gsettings_value("'prefer-dark'\r\n").as_deref(),
            Some("prefer-dark")
        );
        assert_eq!(gsettings_value("''\n"), None);
        assert_eq!(gnome_color_scheme("prefer-dark"), Some(true));
        assert_eq!(gnome_color_scheme("prefer-light"), Some(false));
        assert_eq!(gnome_color_scheme("default"), None);
        assert_eq!(
            gnome_accent("blue"),
            Some(Color32::from_rgb(0x35, 0x84, 0xe4))
        );
        assert_eq!(
            gnome_accent("slate"),
            Some(Color32::from_rgb(0x6f, 0x83, 0x96))
        );
        assert_eq!(gnome_accent("chartreuse"), None);
    }

    const BREEZE_DARK: &str = "[General]\r\nAccentColor=233,84,32\r\n\r\n[Colors:Window]\r\nBackgroundNormal=32,35,38\r\n\r\n[Colors:Selection]\r\nBackgroundNormal=61,174,233\r\n";
    const BREEZE_LIGHT: &str = "[Colors:Window]\nBackgroundNormal=239,240,241\n[Colors:Selection]\nBackgroundNormal=61,174,233\n";

    #[test]
    fn kdeglobals_colour_schemes() {
        assert_eq!(
            kde_theme(BREEZE_DARK),
            SystemTheme {
                dark: Some(true),
                accent: Some(Color32::from_rgb(233, 84, 32)),
            }
        );
        assert_eq!(
            kde_theme(BREEZE_LIGHT),
            SystemTheme {
                dark: Some(false),
                accent: Some(Color32::from_rgb(61, 174, 233)),
            }
        );
        assert_eq!(kde_theme(""), SystemTheme::default());
    }

    #[test]
    fn gtk_theme_names() {
        assert_eq!(gtk_theme_dark("Adwaita-dark"), Some(true));
        assert_eq!(gtk_theme_dark("Breeze-Dark"), Some(true));
        assert_eq!(gtk_theme_dark("Adwaita"), Some(false));
        assert_eq!(gtk_theme_dark(""), None);
    }

    #[test]
    fn readings_combine_portal_first_then_kde_gnome_and_gtk() {
        assert_eq!(resolve(&Readings::default()), SystemTheme::default());
        // The portal wins where it answers.
        let both = Readings {
            portal_dark: Some(false),
            kdeglobals: Some(BREEZE_DARK.into()),
            ..Readings::default()
        };
        assert_eq!(
            resolve(&both),
            SystemTheme {
                dark: Some(false),
                accent: Some(Color32::from_rgb(233, 84, 32)),
            }
        );
        let gnome = Readings {
            gnome_color_scheme: Some("prefer-light".into()),
            gnome_accent: Some("green".into()),
            gsettings_gtk_theme: Some("Adwaita-dark".into()),
            ..Readings::default()
        };
        assert_eq!(
            resolve(&gnome),
            SystemTheme {
                dark: Some(false),
                accent: gnome_accent("green"),
            }
        );
        // GNOME's "default" says nothing, so the GTK theme decides.
        let gtk = Readings {
            gnome_color_scheme: Some("default".into()),
            gtk_settings: Some("[Settings]\r\ngtk-theme-name=Yaru-dark\r\n".into()),
            ..Readings::default()
        };
        assert_eq!(resolve(&gtk).dark, Some(true));
        let prefer = Readings {
            gtk_settings: Some(
                "[Settings]\ngtk-theme-name=Adwaita\ngtk-application-prefer-dark-theme=1\n".into(),
            ),
            ..Readings::default()
        };
        assert_eq!(resolve(&prefer).dark, Some(true));
    }
}
