use serde_json::json;
use vatra_core::{Store,NewItem,TaskView};
#[test]
fn task_views_filter_before_pagination_and_keep_undated_tasks_upcoming() {
    let store=Store::open_in_memory().unwrap();let project=store.create_project("p").unwrap();
    let create=|title:&str,fields|store.create_item(&NewItem{kind:"task".into(),title:title.into(),body:String::new(),fields,project_id:Some(project)},1).unwrap();
    let today=create("Today",json!({"due_date":"2026-11-05"}));let overdue=create("Overdue",json!({"due_date":"2026-11-04"}));
    for _ in 0..110 {create("Future",json!({"due_date":"2026-12-01"}));}create("Undated",json!({}));create("Done",json!({"status":"completed"}));
    let rows=store.workspace_task_previews(project,TaskView::Today,"2026-11-05",false,None,100).unwrap();assert_eq!(rows.iter().map(|r|r.item.id).collect::<Vec<_>>(),vec![overdue,today]);
    assert_eq!(store.workspace_task_previews(project,TaskView::Upcoming,"2026-11-05",false,None,100).unwrap().len(),100);assert_eq!(store.workspace_task_previews(project,TaskView::Completed,"2026-11-05",false,None,100).unwrap().len(),1);
    assert!(store.workspace_task_previews(project,TaskView::Today,"2026-02-29",false,None,100).is_err());
}
