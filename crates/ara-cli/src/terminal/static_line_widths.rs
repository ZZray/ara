//! Fixed utils.ts WeakMap line identity, snapshot and epoch publication.
use super::super::text;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, Weak},
};
struct Entry {
    lines: Weak<Vec<String>>,
    snapshot: Vec<String>,
    widths: Arc<Vec<usize>>,
    epoch: u64,
}
static ENTRIES: OnceLock<Mutex<HashMap<usize, Entry>>> = OnceLock::new();
fn entries() -> &'static Mutex<HashMap<usize, Entry>> {
    ENTRIES.get_or_init(|| Mutex::new(HashMap::new()))
}
pub fn publish_line_widths(lines: &Arc<Vec<String>>, widths: Arc<Vec<usize>>) -> std::io::Result<()> {
    publish_at_epoch(lines, widths, text::get_width_config_epoch())
}
/// Renderer-owned epoch prevents stamping widths measured across a change with
/// the newer epoch. No retry: the next ordinary frame recalculates.
pub(super) fn publish_at_epoch(lines: &Arc<Vec<String>>, widths: Arc<Vec<usize>>, epoch: u64) -> std::io::Result<()> {
    if widths.len() != lines.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Cannot publish {} widths for {} lines", widths.len(), lines.len()),
        ));
    }
    let mut entries = entries().lock().unwrap_or_else(|e| e.into_inner());
    entries.retain(|_, e| e.lines.strong_count() > 0);
    if epoch != text::get_width_config_epoch() {
        return Ok(());
    }
    entries.insert(
        Arc::as_ptr(lines) as usize,
        Entry { lines: Arc::downgrade(lines), snapshot: (**lines).clone(), widths, epoch },
    );
    Ok(())
}
pub fn get_published_line_widths(lines: &Arc<Vec<String>>) -> Option<Arc<Vec<usize>>> {
    let mut entries = entries().lock().unwrap_or_else(|e| e.into_inner());
    let key = Arc::as_ptr(lines) as usize;
    let valid = entries.get(&key).is_some_and(|e| {
        e.epoch == text::get_width_config_epoch()
            && e.widths.len() == lines.len()
            && e.snapshot == **lines
            && e.lines.upgrade().is_some_and(|a| Arc::ptr_eq(&a, lines))
    });
    if valid {
        entries.get(&key).map(|e| e.widths.clone())
    } else {
        entries.remove(&key);
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_utils_publication_snapshot_identity_and_length_contract() {
        let lines = Arc::new(vec!["abc".to_owned()]);
        let mut widths = Arc::new(vec![3]);
        publish_line_widths(&lines, widths.clone()).unwrap();
        let carried = get_published_line_widths(&lines).unwrap();
        assert_eq!(*carried, vec![3]);
        assert!(Arc::ptr_eq(&carried, &get_published_line_widths(&lines).unwrap()));
        Arc::make_mut(&mut widths)[0] = 99;
        assert_eq!(*get_published_line_widths(&lines).unwrap(), vec![3], "fixed utils.ts copied widths snapshot");
        let copy = Arc::new((*lines).clone());
        assert!(get_published_line_widths(&copy).is_none(), "identity is the exact lines owner");
        let wrong = Arc::new(vec!["a".to_owned(), "b".to_owned()]);
        let error = publish_line_widths(&wrong, Arc::new(vec![1])).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(error.to_string(), "Cannot publish 1 widths for 2 lines");
        assert!(get_published_line_widths(&wrong).is_none(), "fixed utils.ts:56-59 wrong length installs no entry");
        let stale = Arc::new(vec!["old".to_owned()]);
        publish_at_epoch(&stale, Arc::new(vec![3]), text::get_width_config_epoch().wrapping_add(1)).unwrap();
        assert!(
            get_published_line_widths(&stale).is_none(),
            "cross-epoch render does not stamp old rows under a new epoch"
        );
        let mut changed = lines.clone();
        Arc::make_mut(&mut changed)[0] = "changed".into();
        assert!(
            get_published_line_widths(&changed).is_none(),
            "native mutation creates new identity; old publication stays owned snapshot"
        );
    }
}
