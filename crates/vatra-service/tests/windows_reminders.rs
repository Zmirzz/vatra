use std::sync::{Arc, Mutex};
use vatra_service::reminders::windows::{ToastBackend, WindowsReminderNotifier, toast_payload};
use vatra_service::reminders::{NotifyOutcome, ReminderNotice, ReminderNotifier};
struct Backend {
    status: NotifyOutcome,
    received: Mutex<Vec<(String, String, String)>>,
}
impl ToastBackend for Backend {
    fn status(&self, _: &str) -> NotifyOutcome {
        self.status
    }
    fn show(&self, _: &str, xml: &str, tag: &str, group: &str) -> Result<(), String> {
        self.received
            .lock()
            .unwrap()
            .push((xml.into(), tag.into(), group.into()));
        Ok(())
    }
}
fn notice(generation: i64) -> ReminderNotice {
    ReminderNotice {
        id: 12,
        generation,
        title: "Ședință & <text> \"quoted\"\0".into(),
        language: "ro".into(),
    }
}
#[test]
fn toast_escapes_text_has_no_activation_command_and_distinguishes_generations() {
    let backend = Arc::new(Backend {
        status: NotifyOutcome::Submitted,
        received: Mutex::new(vec![]),
    });
    let notifier = WindowsReminderNotifier::with_backend("dev.vatra.desktop", backend.clone());
    for generation in [1, 2] {
        assert_eq!(
            notifier.show(&notice(generation)).unwrap(),
            NotifyOutcome::Submitted
        );
    }
    let received = backend.received.lock().unwrap();
    let (xml, tag, group) = &received[0];
    assert!(xml.contains("Ședință &amp; &lt;text&gt; &quot;quoted&quot;"));
    assert!(!xml.contains('\0'));
    assert!(!xml.contains("launch="));
    assert!(!xml.contains("src="));
    assert!(!xml.contains("arguments="));
    assert_eq!(tag, &received[1].1);
    assert_ne!(group, &received[1].2);
    assert_eq!(xml.matches("<text>").count(), 2);
}
#[test]
fn disabled_or_missing_identity_never_calls_show() {
    for status in [NotifyOutcome::Disabled, NotifyOutcome::Unavailable] {
        let backend = Arc::new(Backend {
            status,
            received: Mutex::new(vec![]),
        });
        let notifier = WindowsReminderNotifier::with_backend("dev.vatra.desktop", backend.clone());
        assert_eq!(notifier.show(&notice(1)).unwrap(), status);
        assert!(backend.received.lock().unwrap().is_empty());
    }
}
#[test]
fn first_submission_does_not_claim_settings_are_already_enabled() {
    struct FirstSubmission(Backend);
    impl ToastBackend for FirstSubmission {
        fn status(&self, _: &str) -> NotifyOutcome {
            NotifyOutcome::Unavailable
        }
        fn submission_status(&self, _: &str) -> NotifyOutcome {
            NotifyOutcome::Submitted
        }
        fn show(&self, app_id: &str, xml: &str, tag: &str, group: &str) -> Result<(), String> {
            self.0.show(app_id, xml, tag, group)
        }
    }
    let backend = Arc::new(FirstSubmission(Backend {
        status: NotifyOutcome::Unavailable,
        received: Mutex::new(vec![]),
    }));
    let notifier = WindowsReminderNotifier::with_backend("dev.vatra.desktop", backend.clone());
    assert_eq!(notifier.status(), NotifyOutcome::Unavailable);
    assert_eq!(notifier.show(&notice(1)).unwrap(), NotifyOutcome::Submitted);
    assert_eq!(backend.0.received.lock().unwrap().len(), 1);
}
#[cfg(windows)]
#[test]
fn generated_payload_is_accepted_by_the_real_windows_xml_parser() {
    // Parser contract only, no toast is submitted by this test.
    let xml = toast_payload(&notice(1));
    let parsed = windows::Data::Xml::Dom::XmlDocument::new().unwrap();
    parsed.LoadXml(&windows::core::HSTRING::from(xml)).unwrap();
    assert_eq!(
        parsed
            .GetElementsByTagName(&windows::core::HSTRING::from("text"))
            .unwrap()
            .Length()
            .unwrap(),
        2
    );
}
#[test]
#[ignore = "opt-in: submits a real Windows toast using the already installed Vatra identity"]
fn real_notification_submission_requires_an_existing_identity() {
    let notifier = WindowsReminderNotifier::new("dev.vatra.desktop");
    assert_eq!(
        notifier.show(&notice(1)).unwrap(),
        NotifyOutcome::Submitted,
        "A valid existing identity and enabled notifications are required; API acceptance alone does not prove visibility."
    );
}
