//! Source views must preserve the original evidence while bounding model input.
use super::*;

struct Fixture {
    store: Shared,
    id: String,
}

impl Fixture {
    fn new(text: &str) -> Self {
        let path = std::env::temp_dir().join(format!("lite-reference-{}", crate::fresh_id()));
        let mut store = Store::load(&path, "fixture".into(), &[]).unwrap();
        let id = store.add_reference("objective", text.into(), "run1").unwrap();
        Self { store: Arc::new(Mutex::new(store)), id }
    }

    fn perform(&self, name: &str, args: Value) -> Result<Value> {
        let tool = MemoryTool {
            store: self.store.clone(),
            path: PathBuf::new(),
            run: "run2".into(),
            token: String::new(),
            evidence: Arc::new(|| Ok(Vec::new())),
            schema: Tool {
                name: name.into(),
                description: String::new(),
                parameters: Schema(json!({"type":"object"})),
                constrained_sampling: None,
            },
        };
        Ok(serde_json::from_str(&tool.perform(args, &CancellationToken::new())?)?)
    }

    fn read(&self, args: Value) -> Result<Value> {
        let mut args = args;
        args["id"] = json!(self.id);
        self.perform("reference_read", args)
    }
}

#[test]
fn windows_reassemble_utf8_original_and_preserve_every_source_field() {
    let original = "a中🙂\n".repeat(7000);
    let fixture = Fixture::new(&original);
    let before = serde_json::to_value(&*fixture.store.lock().unwrap()).unwrap();
    let metadata = &before["references"][0];
    let mut recovered = String::new();
    let mut offset = 0;
    while offset < original.len() {
        let page = fixture.read(json!({"offset":offset,"limit":37})).unwrap();
        for (key, value) in metadata.as_object().unwrap() {
            if key != "text" {
                assert_eq!(&page[key], value, "source metadata changed: {key}");
            }
        }
        let next = page["next_offset"].as_u64().unwrap() as usize;
        let text = page["text"].as_str().unwrap();
        assert!(next > offset && text.len() <= 37);
        assert_eq!(text, &original[offset..next]);
        assert_eq!(page["bytes"], original.len());
        assert_eq!(page["offset"], offset);
        assert_eq!(page["hash_scope"], "full_source_text_utf8");
        assert_eq!(page["truncated"], offset > 0 || next < original.len());
        recovered.push_str(text);
        offset = next;
    }
    assert_eq!(recovered, original);
    assert_eq!(hash(recovered.as_bytes()), metadata["hash"].as_str().unwrap());
    assert_eq!(serde_json::to_value(&*fixture.store.lock().unwrap()).unwrap(), before);
}

#[test]
fn read_default_maximum_and_final_partial_window_are_explicit() {
    let fixture = Fixture::new(&"x".repeat(20000));
    let default = fixture.read(json!({})).unwrap();
    assert_eq!(default["text"].as_str().unwrap().len(), 8192);
    assert_eq!(default["next_offset"], 8192);
    assert_eq!(default["truncated"], true);
    let maximum = fixture.read(json!({"limit":16384})).unwrap();
    assert_eq!(maximum["text"].as_str().unwrap().len(), 16384);
    let last = fixture.read(json!({"offset":19999})).unwrap();
    assert_eq!(last["text"], "x");
    assert_eq!(last["next_offset"], 20000);
    assert_eq!(last["truncated"], true);
    let small = Fixture::new("whole source").read(json!({})).unwrap();
    assert_eq!(small["text"], "whole source");
    assert_eq!(small["truncated"], false);
}

#[test]
fn invalid_ranges_fail_without_rounding_offsets_or_exposing_wrong_bytes() {
    let fixture = Fixture::new("中🙂x");
    for args in [
        json!({"offset":1}),
        json!({"offset":4}),
        json!({"offset":9}),
        json!({"offset":-1}),
        json!({"offset":1.5}),
        json!({"offset":"0"}),
        json!({"offset":null}),
        json!({"limit":0}),
        json!({"limit":16385}),
        json!({"limit":-1}),
        json!({"limit":1.5}),
        json!({"limit":"8"}),
        json!({"limit":null}),
        json!({"limit":2}),
        json!({"offset":3,"limit":3}),
    ] {
        assert!(fixture.read(args.clone()).is_err(), "accepted invalid range: {args}");
    }
    let scalar = fixture.read(json!({"offset":3,"limit":4})).unwrap();
    assert_eq!(scalar["text"], "🙂");
    assert_eq!(scalar["next_offset"], 7);
    let eof = fixture.read(json!({"offset":8})).unwrap();
    assert_eq!(eof["text"], "");
    assert_eq!(eof["next_offset"], 8);
    assert_eq!(eof["truncated"], true);
}

#[test]
fn deep_unicode_match_coordinates_select_the_original_body() {
    let prefix = "原文🙂".repeat(4000);
    let query = "目标🙂=蓝色";
    let original = format!("{prefix}{query}{}", "后文🙂".repeat(1000));
    let fixture = Fixture::new(&original);
    let result = fixture.perform("reference_search", json!({"query":query})).unwrap();
    let found = &result["matches"][0];
    assert_eq!(result["matches"].as_array().unwrap().len(), 1);
    assert_eq!(found["match_offset"], prefix.len());
    assert_eq!(found["match_bytes"], query.len());
    assert_eq!(found["hash"], hash(original.as_bytes()));
    let start = found["excerpt_offset"].as_u64().unwrap() as usize;
    let end = found["excerpt_next_offset"].as_u64().unwrap() as usize;
    assert_eq!(found["excerpt"], &original[start..end]);
    assert!(found["excerpt"].as_str().unwrap().contains(query));
    assert!(end - start <= 512);
    let page = fixture.read(json!({"offset":found["match_offset"],"limit":query.len()})).unwrap();
    assert_eq!(page["text"], query);
    assert_eq!(page["hash"], found["hash"]);
    assert!(fixture.perform("reference_search", json!({"query":""})).is_err());
    assert!(fixture.perform("reference_search", json!({"query":"x".repeat(1025)})).is_err());
}

#[test]
fn corrected_and_revoked_sources_cannot_be_retrieved_by_either_tool() {
    let fixture = Fixture::new("OLD_ONLY");
    let command = format!("/reference correct {} NEW_ONLY", fixture.id);
    fixture.store.lock().unwrap().command(&command, "run2", 0).unwrap();
    assert!(fixture.read(json!({})).is_err());
    let old = fixture.perform("reference_search", json!({"query":"OLD_ONLY"})).unwrap();
    assert!(old["matches"].as_array().unwrap().is_empty());
    let current = fixture.perform("reference_search", json!({"query":"NEW_ONLY"})).unwrap();
    let id = current["matches"][0]["id"].as_str().unwrap();
    assert_eq!(fixture.perform("reference_read", json!({"id":id})).unwrap()["version"], 2);
    fixture.store.lock().unwrap().command(&format!("/reference revoke {id}"), "run3", 0).unwrap();
    assert!(fixture.perform("reference_read", json!({"id":id})).is_err());
    assert!(fixture.perform("reference_search", json!({"query":"NEW_ONLY"})).unwrap()["matches"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn long_literal_is_not_cut_out_of_the_bounded_search_excerpt() {
    let query = "目".repeat(300) + "🙂";
    let original = format!("{}{}{}", "前文🙂".repeat(1000), query, "x".repeat(1000));
    let fixture = Fixture::new(&original);
    let result = fixture.perform("reference_search", json!({"query":query})).unwrap();
    let found = &result["matches"][0];
    let excerpt = found["excerpt"].as_str().unwrap();
    assert!(excerpt.contains(&query));
    assert!(excerpt.len() <= 1120);
    assert_eq!(found["version"], 1);
    let at = found["match_offset"].as_u64().unwrap() as usize;
    assert_eq!(&original[at..at + query.len()], query);
}
