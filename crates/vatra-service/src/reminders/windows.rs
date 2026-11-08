//! WinRT toast adapter. A successful Show means submitted, never observed by the person.
use super::{NotifyOutcome, ReminderNotice, ReminderNotifier};
use std::sync::Arc;
pub const VATRA_APP_ID: &str = "dev.vatra.desktop";
pub trait ToastBackend: Send + Sync {
    fn status(&self, app_id: &str) -> NotifyOutcome;
    fn submission_status(&self, app_id: &str) -> NotifyOutcome {
        self.status(app_id)
    }
    fn show(&self, app_id: &str, xml: &str, tag: &str, group: &str) -> Result<(), String>;
}
pub struct WindowsReminderNotifier {
    app_id: String,
    backend: Arc<dyn ToastBackend>,
}
impl WindowsReminderNotifier {
    pub fn new(app_id: &str) -> Self {
        Self::with_backend(app_id, Arc::new(SystemToastBackend))
    }
    pub fn with_backend(app_id: &str, backend: Arc<dyn ToastBackend>) -> Self {
        Self {
            app_id: app_id.into(),
            backend,
        }
    }
}
impl ReminderNotifier for WindowsReminderNotifier {
    fn status(&self) -> NotifyOutcome {
        self.backend.status(&self.app_id)
    }
    fn show(&self, notice: &ReminderNotice) -> Result<NotifyOutcome, String> {
        let status = self.backend.submission_status(&self.app_id);
        if status != NotifyOutcome::Submitted {
            return Ok(status);
        }
        self.backend.show(
            &self.app_id,
            &toast_payload(notice),
            &format!("r{:x}", notice.id),
            &format!("g{:x}", notice.generation),
        )?;
        Ok(NotifyOutcome::Submitted)
    }
}
pub fn toast_payload(notice: &ReminderNotice) -> String {
    fn escaped(text: &str) -> String {
        let mut output = String::new();
        for ch in text.chars().take(240) {
            match ch {
                '&' => output.push_str("&amp;"),
                '<' => output.push_str("&lt;"),
                '>' => output.push_str("&gt;"),
                '"' => output.push_str("&quot;"),
                '\'' => output.push_str("&apos;"),
                '\t' | '\r' | '\n' => output.push(ch),
                c if c >= ' ' && c != '\u{fffe}' && c != '\u{ffff}' => output.push(c),
                _ => {}
            }
        }
        output
    }
    let heading = if notice.language == "ro" {
        "Vatra — reamintire"
    } else {
        "Vatra — reminder"
    };
    format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{heading}</text><text>{}</text></binding></visual></toast>",
        escaped(&notice.title)
    )
}
struct SystemToastBackend;
#[cfg(not(windows))]
impl ToastBackend for SystemToastBackend {
    fn status(&self, _: &str) -> NotifyOutcome {
        NotifyOutcome::Unavailable
    }
    fn show(&self, _: &str, _: &str, _: &str, _: &str) -> Result<(), String> {
        Err("Windows notifications are unavailable.".into())
    }
}
#[cfg(windows)]
mod native {
    use super::*;
    use std::{os::windows::ffi::OsStrExt, path::Path};
    use windows::{
        Data::Xml::Dom::XmlDocument,
        UI::Notifications::{NotificationSetting, ToastNotification, ToastNotificationManager},
        Win32::{
            Foundation::PROPERTYKEY,
            System::{
                Com::CoTaskMemFree,
                WinRT::{
                    RO_INIT_MULTITHREADED, RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize,
                },
            },
            UI::Shell::{IShellItem2, SHCreateItemFromParsingName},
        },
        core::{GUID, HSTRING, PCWSTR},
    };
    struct Apartment;
    impl Apartment {
        fn enter() -> Result<Self, String> {
            let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
            match initialized {
                Ok(()) => Ok(Self),
                Err(error) if error.code().0 == 0x80010106u32 as i32 => {
                    unsafe { RoInitialize(RO_INIT_SINGLETHREADED) }.map_err(|_| {
                        "Windows notifications could not be initialized.".to_string()
                    })?;
                    Ok(Self)
                }
                Err(_) => Err("Windows notifications could not be initialized.".into()),
            }
        }
    }
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                RoUninitialize();
            }
        }
    }
    fn shell_item(path: &Path) -> Option<IShellItem2> {
        // Shell parsing rejects forward slashes, even when filesystem APIs accept them.
        let path = dunce::canonicalize(path).ok()?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok() }
    }
    fn shortcut_id(path: &Path) -> Option<String> {
        // System.AppUserModel.ID, a documented Windows property key; no shortcut is created or changed.
        let key = PROPERTYKEY {
            fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
            pid: 5,
        };
        unsafe {
            let item = shell_item(path)?;
            let value = item.GetString(&key).ok()?;
            let text = value.to_string().ok();
            CoTaskMemFree(Some(value.0.cast()));
            text
        }
    }
    #[test]
    fn notification_identity_accepts_mixed_path_separators() {
        let _apartment = Apartment::enter().unwrap();
        let folder = tempfile::tempdir().unwrap();
        std::fs::create_dir(folder.path().join("shortcuts")).unwrap();
        let path = folder.path().join("shortcuts/Vatra.lnk");
        std::fs::write(&path, []).unwrap();
        assert!(
            shell_item(&path).is_some(),
            "Shell must be able to parse the existing identity path"
        );
    }
    fn registered(app_id: &str) -> bool {
        let Some(roaming) = std::env::var_os("APPDATA") else {
            return false;
        };
        let root = std::path::PathBuf::from(roaming).join("Microsoft/Windows/Start Menu/Programs");
        let mut folders = vec![(root, 0)];
        let mut visited = 0;
        while let Some((folder, depth)) = folders.pop() {
            let Ok(entries) = std::fs::read_dir(folder) else {
                continue;
            };
            for entry in entries.flatten() {
                visited += 1;
                if visited > 2000 {
                    return false;
                }
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() && depth < 3 {
                    folders.push((path, depth + 1));
                    continue;
                }
                if entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case("Vatra.lnk")
                    && shortcut_id(&path).as_deref() == Some(app_id)
                {
                    return true;
                }
            }
        }
        false
    }
    impl ToastBackend for SystemToastBackend {
        fn status(&self, app_id: &str) -> NotifyOutcome {
            notification_status(app_id, false)
        }
        fn submission_status(&self, app_id: &str) -> NotifyOutcome {
            notification_status(app_id, true)
        }
        fn show(&self, app_id: &str, xml: &str, tag: &str, group: &str) -> Result<(), String> {
            let _apartment = Apartment::enter()?;
            let submit = || -> windows::core::Result<()> {
                let document = XmlDocument::new()?;
                document.LoadXml(&HSTRING::from(xml))?;
                let toast = ToastNotification::CreateToastNotification(&document)?;
                toast.SetTag(&HSTRING::from(tag))?;
                toast.SetGroup(&HSTRING::from(group))?;
                ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))?
                    .Show(&toast)
            };
            submit().map_err(|_| "Windows did not accept the reminder notification.".into())
        }
    }
    fn notification_status(app_id: &str, first_submission: bool) -> NotifyOutcome {
        let Ok(_apartment) = Apartment::enter() else {
            return NotifyOutcome::Unavailable;
        };
        if !registered(app_id) {
            return NotifyOutcome::Unavailable;
        }
        let Ok(notifier) =
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))
        else {
            return NotifyOutcome::Unavailable;
        };
        setting_status(notifier.Setting(), first_submission)
    }
    fn setting_status(
        setting: windows::core::Result<NotificationSetting>,
        first_submission: bool,
    ) -> NotifyOutcome {
        match setting {
            Ok(NotificationSetting::Enabled) => NotifyOutcome::Submitted,
            Ok(_) => NotifyOutcome::Disabled,
            // On a fresh classic-desktop install, Setting fails until the first Show.
            // Only submission with an independently verified shortcut may proceed;
            // status queries still report Unavailable, and Windows enforces its policy.
            Err(error) if first_submission && error.code().0 == 0x80070490u32 as i32 => {
                NotifyOutcome::Submitted
            }
            Err(_) => NotifyOutcome::Unavailable,
        }
    }
    #[test]
    fn first_toast_setting_is_unknown_until_submission() {
        let unknown = || {
            Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                0x80070490u32 as i32,
            )))
        };
        assert_eq!(setting_status(unknown(), false), NotifyOutcome::Unavailable);
        assert_eq!(setting_status(unknown(), true), NotifyOutcome::Submitted);
        assert_eq!(
            setting_status(Ok(NotificationSetting::DisabledForApplication), true),
            NotifyOutcome::Disabled
        );
        assert_eq!(
            setting_status(
                Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80070005u32 as i32
                ))),
                true
            ),
            NotifyOutcome::Unavailable
        );
    }
}
