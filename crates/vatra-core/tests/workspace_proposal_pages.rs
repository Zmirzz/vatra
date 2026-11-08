use vatra_core::{Store,WorkspaceProposal,ProposalPayload,ProposalState,NewItem};
use serde_json::json;
#[test]
fn proposal_pages_fit_a_bounded_message_and_preserve_access_to_older_suggestions() {
    let store=Store::open_in_memory().unwrap();let project=store.create_project("p").unwrap();let session=store.create_session(project,"fixture").unwrap();
    for n in 0..9 {store.save_workspace_proposal(&WorkspaceProposal{id:0,project_id:project,session_id:session,client_op_id:n.to_string(),target_id:None,base_revision:None,payload:ProposalPayload::Tasks{items:vec![NewItem{kind:"task".into(),title:"Large".into(),body:"x".repeat(1024*1024),fields:json!({}),project_id:Some(project)}]},state:ProposalState::Pending,created_at:1}).unwrap();}
    let first=store.workspace_proposals_page(project,None,None,100).unwrap();assert!(!first.is_empty());assert!(first.len()<9);assert!(serde_json::to_vec(&first).unwrap().len()<8*1024*1024);
    let next=store.workspace_proposals_page(project,None,first.last().map(|p|p.id),100).unwrap();assert_eq!(first.len()+next.len(),9);assert!(first.iter().all(|a|next.iter().all(|b|a.id!=b.id)));
}
