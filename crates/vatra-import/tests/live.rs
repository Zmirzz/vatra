//! Opt-in, local-only validation. Never opens authentication files or calls a model.
use std::sync::atomic::AtomicBool;
use sha2::{Digest,Sha256};
use vatra_import::{SourceDirs,GuardedFs,history_files,parse_claude_log,parse_codex_log,import_log};
use vatra_core::Store;

#[test]
#[ignore="reads one small real conversation log into an isolated temporary database"]
fn real_history_search_preserves_source(){
    let dirs=SourceDirs::detect();let guard=GuardedFs::new();
    let mut checked=0;
    for (path,origin) in history_files(&dirs) {
        let Ok(meta)=guard.metadata(&path)else{continue};if meta.len()>5*1024*1024{continue;}
        let before=guard.read_to_string(&path).unwrap();
        let parsed=if origin=="claude"{parse_claude_log(std::io::Cursor::new(&before))}else{parse_codex_log(std::io::Cursor::new(&before))};
        let word=parsed.messages.iter().flat_map(|message|message.text.split(|c:char|!c.is_alphanumeric())).find(|word|word.len()>=5).map(str::to_owned);
        let Some(word)=word else{continue;};
        let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("validation.db")).unwrap();
        import_log(&path,origin,&store,&[],&AtomicBool::new(false),&mut |_,_|{}).unwrap();
        assert!(!store.search_history(&word,None,10).unwrap().is_empty(),"real message was not searchable");
        assert_eq!(Sha256::digest(before.as_bytes()),Sha256::digest(guard.read_to_string(&path).unwrap().as_bytes()),"source log changed");
        checked+=1;if checked==2{break;}
    }
    assert!(checked>0,"no small real conversation log was available for validation");
    println!("Validated {checked} real log(s): searchable, source hashes unchanged, no model calls.");
}
