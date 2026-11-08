mod common;
use serde_json::json;
use std::sync::{Arc, Mutex};
use vatra_core::{NewItem, Store};
use vatra_service::reminders::{
    NotifyOutcome, ReminderDispatcher, ReminderNotice, ReminderNotifier,
};
struct Recorder {
    calls: Mutex<Vec<ReminderNotice>>,
    outcome: NotifyOutcome,
}
impl ReminderNotifier for Recorder {
    fn show(&self, n: &ReminderNotice) -> Result<NotifyOutcome, String> {
        self.calls.lock().unwrap().push(n.clone());
        Ok(self.outcome)
    }
}
fn setup(outcome: NotifyOutcome) -> (Arc<Store>, Arc<Recorder>, ReminderDispatcher, i64) {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let p = store.create_project("p").unwrap();
    store
        .create_item(
            &NewItem {
                kind: "task".into(),
                title: "Meeting".into(),
                body: String::new(),
                fields: json!({"remind_at":100,"timezone":"UTC"}),
                project_id: Some(p),
            },
            1,
        )
        .unwrap();
    let notifier = Arc::new(Recorder {
        calls: Mutex::new(vec![]),
        outcome,
    });
    let dispatcher = ReminderDispatcher::new(store.clone(), notifier.clone());
    (store, notifier, dispatcher, p)
}
#[test]
fn dispatcher_claims_before_show_and_retains_disabled_inbox_without_retry() {
    let (store, notifier, dispatcher, p) = setup(NotifyOutcome::Disabled);
    store
        .set_app_setting("reminders.language", Some("ro"))
        .unwrap();
    dispatcher.tick(99).unwrap();
    assert!(notifier.calls.lock().unwrap().is_empty());
    dispatcher.tick(100).unwrap();
    dispatcher.tick(101).unwrap();
    assert_eq!(notifier.calls.lock().unwrap().len(), 1);
    assert_eq!(notifier.calls.lock().unwrap()[0].language, "ro");
    assert_eq!(store.reminder_inbox(p, 50).unwrap()[0].state, "blocked");
}
#[test]
fn submitted_is_recorded_without_claiming_that_windows_displayed_it() {
    let (store, notifier, dispatcher, p) = setup(NotifyOutcome::Submitted);
    dispatcher.tick(100).unwrap();
    assert_eq!(notifier.calls.lock().unwrap().len(), 1);
    assert_eq!(store.reminder_inbox(p, 50).unwrap()[0].state, "submitted");
}
#[test]
fn restart_after_claim_records_unknown_and_never_calls_notifier_again() {
    let (store, notifier, dispatcher, p) = setup(NotifyOutcome::Submitted);
    store.claim_due_reminders(100, 50).unwrap();
    let _service = vatra_service::Service::new(store.clone(), common::scripted_agents()).unwrap();
    dispatcher.tick(101).unwrap();
    assert!(notifier.calls.lock().unwrap().is_empty());
    assert_eq!(store.reminder_inbox(p, 50).unwrap()[0].state, "unknown");
}

#[test]
fn notifier_finishing_after_reschedule_cannot_override_cancellation() {
    struct Reschedule {store:Arc<Store>,item:i64,first:std::sync::atomic::AtomicBool}
    impl ReminderNotifier for Reschedule {fn show(&self,_:&ReminderNotice)->Result<NotifyOutcome,String> {
        if self.first.swap(false,std::sync::atomic::Ordering::SeqCst) {let item=self.store.item(self.item).unwrap().unwrap();self.store.save_workspace_item(item.project_id.unwrap(),&vatra_core::ItemChange{id:self.item,expected_revision:item.revision,title:None,body:None,fields:Some(json!({"remind_at":200}))},101).unwrap();}
        Ok(NotifyOutcome::Submitted)
    }}
    let (store,_,_,p)=setup(NotifyOutcome::Submitted);let item=store.workspace_items(p,&["task"],false,None,100).unwrap()[0].id;
    let dispatcher=ReminderDispatcher::new(store.clone(),Arc::new(Reschedule{store:store.clone(),item,first:std::sync::atomic::AtomicBool::new(true)}));dispatcher.tick(100).unwrap();assert_eq!(store.reminder_inbox(p,50).unwrap()[0].state,"cancelled");dispatcher.tick(200).unwrap();let inbox=store.reminder_inbox(p,50).unwrap();assert_eq!(inbox.len(),2);assert_eq!(inbox[0].state,"submitted");
}
#[test]
fn raw_notifier_errors_are_never_written_to_the_inbox() {
    struct Failed;impl ReminderNotifier for Failed {fn show(&self,_:&ReminderNotice)->Result<NotifyOutcome,String>{Err("private-os-error-secret".into())}}
    let (store,_,_,p)=setup(NotifyOutcome::Unavailable);ReminderDispatcher::new(store.clone(),Arc::new(Failed)).tick(100).unwrap();let inbox=store.reminder_inbox(p,50).unwrap();assert_eq!(inbox[0].state,"failed");assert!(!serde_json::to_string(&inbox).unwrap().contains("private-os-error-secret"));
}

#[tokio::test(flavor="multi_thread")]
async fn runtime_owns_the_loop_and_dropping_it_stops_future_ticks_without_a_service_cycle() {
    let dir=tempfile::tempdir().unwrap();let notifier=Arc::new(Recorder{calls:Mutex::new(vec![]),outcome:NotifyOutcome::Submitted});
    let runtime=vatra_service::Runtime::start_with_notifier(dir.path(),vatra_service::config::Config::empty(),notifier.clone()).await.unwrap();
    let weak=Arc::downgrade(runtime.service());let store=runtime.service().store().clone();let p=store.create_project("p").unwrap();
    store.create_item(&NewItem{kind:"task".into(),title:"Meeting".into(),body:String::new(),fields:json!({"remind_at":100,"timezone":"UTC"}),project_id:Some(p)},1).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(8),async {loop {if !notifier.calls.lock().unwrap().is_empty() {break;}tokio::time::sleep(std::time::Duration::from_millis(50)).await;}}).await.unwrap();
    drop(runtime);for _ in 0..20 {if weak.upgrade().is_none() {break;}tokio::time::sleep(std::time::Duration::from_millis(50)).await;}
    assert!(weak.upgrade().is_none());
    store.create_item(&NewItem{kind:"task".into(),title:"After shutdown".into(),body:String::new(),fields:json!({"remind_at":100,"timezone":"UTC"}),project_id:Some(p)},1).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;assert_eq!(notifier.calls.lock().unwrap().len(),1);
}
