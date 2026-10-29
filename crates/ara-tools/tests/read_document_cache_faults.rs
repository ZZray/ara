//! Supplemental execution of changed wrapper empty/cancel publication paths.
//! Original upstream assertions live separately in read_document_cache_original.
use ara_tools::read_document_cache::{CONVERTER_VERSION, DocumentCache, convert_buffer_with_cache};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

#[test]
fn empty_and_cancelled_conversions_never_publish_a_cache_entry() {
    let dir = tempfile::tempdir().unwrap();
    let cache = DocumentCache::new(dir.path().into(), CONVERTER_VERSION.into());
    let calls = AtomicUsize::new(0);
    let cancel = CancellationToken::new();
    let bytes = b"empty first, recover second";
    let entry = dir.path().join(format!("{}.json", cache.key("pdf", bytes)));
    let empty = convert_buffer_with_cache(Some(&cache), bytes, "pdf", &cancel, true, |_, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(String::new())
    }).unwrap();
    assert!(!empty.ok);
    assert_eq!(empty.error.as_deref(), Some("Conversion produced no output"));
    assert!(!entry.exists(), "Empty conversion must not create even an invalid cache payload");
    let recovered = convert_buffer_with_cache(Some(&cache), bytes, "pdf", &cancel, true, |_, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok("recovered body".into())
    }).unwrap();
    assert!(recovered.ok);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(entry.is_file());

    let cancelled_bytes = b"converter returns after cancellation";
    let cancelled_entry = dir.path().join(format!("{}.json", cache.key("pdf", cancelled_bytes)));
    let cancelled = convert_buffer_with_cache(Some(&cache), cancelled_bytes, "pdf", &cancel, true, |_, _| {
        cancel.cancel();
        Ok("must not publish this body".into())
    });
    assert!(cancelled.is_err(), "Cancellation must propagate instead of becoming a conversion diagnostic");
    assert!(!cancelled_entry.exists());
    let reached = AtomicUsize::new(0);
    let already_cancelled = convert_buffer_with_cache(Some(&cache), bytes, "pdf", &cancel, true, |_, _| {
        reached.fetch_add(1, Ordering::SeqCst);
        Ok("must not reach converter".into())
    });
    assert!(already_cancelled.is_err(), "Even an existing cache hit cannot hide cancellation");
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}
