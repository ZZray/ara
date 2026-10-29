use super::*;

fn parsed(text: &str) -> WireValue {
    WireValue::parse(text).unwrap()
}

fn check(label: &str, schema: &str, value: &str, success: bool, issues: &str) {
    let schema = parsed(schema);
    let value = parsed(value);
    let result = validate_json_schema_value(&schema, &value);
    assert_eq!(result.success, success, "{label}: {}", result.to_wire_value().stringify());
    assert_eq!(is_json_schema_value_valid(&schema, &value), success, "{label}: boolean facade");
    let expected = WireValue::object(vec![("success", WireValue::Bool(success)), ("issues", parsed(issues))]);
    let actual = graph_fixture(&schema, &value);
    assert!(
        actual.deep_equal(&expected),
        "{label}: actual graph expected {}, got {}",
        expected.stringify(),
        actual.stringify()
    );
    assert!(
        result.to_wire_value().deep_equal(&expected),
        "{label}: expected {}, got {}",
        expected.stringify(),
        result.to_wire_value().stringify()
    );
}

#[test]
fn fixed_native_eight_original_titles() {
    // Fixed schema-strict-mode.test.ts:877,886,895,904,917,923,930,953.
    let cases = [
        (
            "877 propertyNames accepts",
            r#"{"type":"object","propertyNames":{"type":"string","pattern":"^[a-z]+$"}}"#,
            r#"{"abc":1}"#,
            true,
            "[]",
        ),
        (
            "877 propertyNames rejects",
            r#"{"type":"object","propertyNames":{"type":"string","pattern":"^[a-z]+$"}}"#,
            r#"{"BAD-KEY":1}"#,
            false,
            r#"[{"path":["BAD-KEY"],"message":"must match pattern","keyword":"pattern"}]"#,
        ),
        (
            "886 patternProperties accepts",
            r#"{"type":"object","patternProperties":{"^id_":{"type":"number"}}}"#,
            r#"{"id_a":1}"#,
            true,
            "[]",
        ),
        (
            "886 patternProperties rejects",
            r#"{"type":"object","patternProperties":{"^id_":{"type":"number"}}}"#,
            r#"{"id_a":"not-a-number"}"#,
            false,
            r#"[{"path":["id_a"],"message":"expected number, received string","keyword":"type","expectedTypes":["number"]}]"#,
        ),
        (
            "895 dependentRequired accepts",
            r#"{"type":"object","dependentRequired":{"credit_card":["billing_address"]}}"#,
            r#"{"credit_card":"x","billing_address":"y"}"#,
            true,
            "[]",
        ),
        (
            "895 dependentRequired rejects",
            r#"{"type":"object","dependentRequired":{"credit_card":["billing_address"]}}"#,
            r#"{"credit_card":"x"}"#,
            false,
            r#"[{"path":["billing_address"],"message":"is required when \"credit_card\" is present","keyword":"dependentRequired"}]"#,
        ),
        (
            "904 conditional inactive",
            r#"{"type":"object","properties":{"kind":{"type":"string"},"extra":{"type":"string"}},"if":{"properties":{"kind":{"const":"a"}},"required":["kind"]},"then":{"required":["extra"]}}"#,
            r#"{"kind":"b"}"#,
            true,
            "[]",
        ),
        (
            "904 conditional accepts",
            r#"{"type":"object","properties":{"kind":{"type":"string"},"extra":{"type":"string"}},"if":{"properties":{"kind":{"const":"a"}},"required":["kind"]},"then":{"required":["extra"]}}"#,
            r#"{"kind":"a","extra":"ok"}"#,
            true,
            "[]",
        ),
        (
            "904 conditional rejects",
            r#"{"type":"object","properties":{"kind":{"type":"string"},"extra":{"type":"string"}},"if":{"properties":{"kind":{"const":"a"}},"required":["kind"]},"then":{"required":["extra"]}}"#,
            r#"{"kind":"a"}"#,
            false,
            r#"[{"path":["extra"],"message":"is required","keyword":"required"}]"#,
        ),
        ("917 contains accepts", r#"{"type":"array","contains":{"type":"number"}}"#, r#"["a",1]"#, true, "[]"),
        (
            "917 contains rejects silently",
            r#"{"type":"array","contains":{"type":"number"}}"#,
            r#"["a","b"]"#,
            false,
            r#"[{"path":[],"message":"must contain at least 1 matching item(s)","keyword":"contains"}]"#,
        ),
        (
            "923 prefix accepts",
            r#"{"type":"array","prefixItems":[{"type":"string"},{"type":"number"}]}"#,
            r#"["x",1]"#,
            true,
            "[]",
        ),
        (
            "923 prefix rejects by index",
            r#"{"type":"array","prefixItems":[{"type":"string"},{"type":"number"}]}"#,
            r#"[1,"x"]"#,
            false,
            r#"[{"path":[0],"message":"expected string, received integer","keyword":"type","expectedTypes":["string"]},{"path":[1],"message":"expected number, received string","keyword":"type","expectedTypes":["number"]}]"#,
        ),
        (
            "923 array items rejected",
            r#"{"type":"array","items":[{"type":"string"}]}"#,
            r#"["x"]"#,
            false,
            r#"[{"path":[],"message":"array-valued items is not valid in JSON Schema 2020-12; use prefixItems","keyword":"items"}]"#,
        ),
    ];
    for (label, schema, value, success, issues) in cases {
        check(label, schema, value, success, issues);
    }
    let recursive = r##"{"$ref":"#/definitions/Node","definitions":{"Node":{"type":"object","properties":{"name":{"type":"string"},"child":{"$ref":"#/definitions/Node"}},"required":["name"],"additionalProperties":false}}}"##;
    check("930 recursive accepts", recursive, r#"{"name":"root","child":{"name":"leaf"}}"#, true, "[]");
    check(
        "930 recursive nested type",
        recursive,
        r#"{"name":"root","child":{"name":123}}"#,
        false,
        r#"[{"path":["child","name"],"message":"expected string, received integer","keyword":"type","expectedTypes":["string"]}]"#,
    );
    check(
        "930 recursive missing required",
        recursive,
        r#"{"name":"root","child":{"child":{"name":"x"}}}"#,
        false,
        r#"[{"path":["child","name"],"message":"is required","keyword":"required"}]"#,
    );
    let mut definitions = WireValue::Object(Vec::new());
    for index in 0..66 {
        definitions.insert(
            &format!("A{index}"),
            WireValue::object(vec![(
                "$ref",
                WireValue::String(WireString::from(format!("#/definitions/A{}", index + 1))),
            )]),
        );
    }
    definitions.insert("A66", parsed(r#"{"type":"number"}"#));
    let schema = WireValue::object(vec![
        ("$ref", WireValue::String(WireString::from("#/definitions/A0"))),
        ("definitions", definitions),
    ]);
    let result = validate_json_schema_value(&schema, &parsed(r#""not-a-number""#));
    assert!(!result.success, "953 primitive reference cap");
    assert_eq!(result.issues.len(), 1);
    assert!(result.issues[0].message.equals_ascii("reference depth exceeded"));
    assert_eq!(result.issues[0].keyword, Some("$ref"));
    let mut definitions = WireValue::Object(Vec::new());
    for index in 0..63 {
        definitions.insert(
            &format!("A{index}"),
            WireValue::object(vec![(
                "$ref",
                WireValue::String(WireString::from(format!("#/definitions/A{}", index + 1))),
            )]),
        );
    }
    definitions.insert("A63", parsed(r#"{"type":"number"}"#));
    let schema = WireValue::object(vec![
        ("$ref", WireValue::String(WireString::from("#/definitions/A0"))),
        ("definitions", definitions),
    ]);
    assert!(is_json_schema_value_valid(&schema, &parsed("1")), "exactly 64 primitive refs are admitted");
    let mismatch = validate_json_schema_value(&schema, &parsed(r#""bad""#));
    assert!(!mismatch.success);
    assert!(mismatch.issues[0].message.equals_ascii("expected number, received string"));
}

#[test]
fn complete_keyword_order_and_union_provenance_family() {
    let cases = [
        ("true schema", "true", "1", true, "[]"),
        (
            "false schema",
            "false",
            "1",
            false,
            r#"[{"path":[],"message":"must not match false schema","keyword":"false"}]"#,
        ),
        (
            "malformed schema",
            "null",
            "1",
            false,
            r#"[{"path":[],"message":"schema must be an object or boolean","keyword":"schema"}]"#,
        ),
        (
            "type before const and union",
            r#"{"type":"string","const":1,"anyOf":[false]}"#,
            "1",
            false,
            r#"[{"path":[],"message":"expected string, received integer","keyword":"type","expectedTypes":["string"]}]"#,
        ),
        (
            "unknown type rejects",
            r#"{"type":"custom"}"#,
            "1",
            false,
            r#"[{"path":[],"message":"expected custom, received integer","keyword":"type","expectedTypes":["custom"]}]"#,
        ),
        ("malformed type entries filtered", r#"{"type":[1,false,"string"]}"#, r#""x""#, true, "[]"),
        (
            "nullable short circuit",
            r#"{"type":"string","nullable":true,"const":1,"allOf":[false]}"#,
            "null",
            true,
            "[]",
        ),
        (
            "nullable contributes expected types",
            r#"{"type":"string","nullable":true}"#,
            "1",
            false,
            r#"[{"path":[],"message":"expected string or null, received integer","keyword":"type","expectedTypes":["string","null"]}]"#,
        ),
        (
            "const and enum order",
            r#"{"const":1,"enum":[2],"allOf":[false],"not":true,"if":true,"then":false}"#,
            "3",
            false,
            r#"[{"path":[],"message":"must equal const value","keyword":"const"},{"path":[],"message":"must be one of the allowed enum values","keyword":"enum"},{"path":[],"message":"must not match false schema","keyword":"false"},{"path":[],"message":"must not match excluded schema","keyword":"not"},{"path":[],"message":"must not match false schema","keyword":"false"}]"#,
        ),
        ("const structural key order ignored", r#"{"const":{"a":1,"b":[2]}}"#, r#"{"b":[2],"a":1}"#, true, "[]"),
        ("anyOf success discards earlier failures", r#"{"anyOf":[false,{"type":"number"}]}"#, "1", true, "[]"),
        (
            "anyOf first guess marked",
            r#"{"anyOf":[{"properties":{"x":{"type":"integer"}}},{"type":"string"}]}"#,
            r#"{"x":"bad"}"#,
            false,
            r#"[{"path":["x"],"message":"expected integer, received string","keyword":"type","expectedTypes":["integer"],"fromUnionBranch":true}]"#,
        ),
        (
            "empty anyOf generic",
            r#"{"anyOf":[]}"#,
            "1",
            false,
            r#"[{"path":[],"message":"must match at least one schema","keyword":"anyOf"}]"#,
        ),
        (
            "oneOf multiple matches generic",
            r#"{"oneOf":[true,{}]}"#,
            "1",
            false,
            r#"[{"path":[],"message":"must match exactly one schema","keyword":"oneOf"}]"#,
        ),
        (
            "unique const tag authoritative",
            r#"{"anyOf":[{"properties":{"tag":{"const":"a"},"payload":{"type":"number"}}},{"properties":{"tag":{"const":"b"},"payload":{"type":"string"}}}]}"#,
            r#"{"tag":"b","payload":1}"#,
            false,
            r#"[{"path":["payload"],"message":"expected string, received integer","keyword":"type","expectedTypes":["string"]}]"#,
        ),
        (
            "unique enum tag authoritative",
            r#"{"oneOf":[{"properties":{"tag":{"enum":["a","b"]},"payload":{"type":"number"}}},{"properties":{"tag":{"const":"c"}}}]}"#,
            r#"{"tag":"b","payload":false}"#,
            false,
            r#"[{"path":["payload"],"message":"expected number, received boolean","keyword":"type","expectedTypes":["number"]}]"#,
        ),
        (
            "multiple selected tags remain guess",
            r#"{"anyOf":[{"properties":{"tag":{"const":"a"},"x":{"type":"number"}}},{"properties":{"tag":{"const":"a"},"x":{"type":"string"}}}]}"#,
            r#"{"tag":"a","x":false}"#,
            false,
            r#"[{"path":["x"],"message":"expected number, received boolean","keyword":"type","expectedTypes":["number"],"fromUnionBranch":true}]"#,
        ),
        (
            "nested union preserves guess bit",
            r#"{"anyOf":[{"anyOf":[false]}]}"#,
            "1",
            false,
            r#"[{"path":[],"message":"must not match false schema","keyword":"false","fromUnionBranch":true}]"#,
        ),
        ("not failed probe ignored", r#"{"not":{"type":"string"}}"#, "1", true, "[]"),
        (
            "conditional else boolean",
            r#"{"if":{"type":"string"},"then":false,"else":false}"#,
            "1",
            false,
            r#"[{"path":[],"message":"must not match false schema","keyword":"false"}]"#,
        ),
        ("malformed required entire set ignored", r#"{"required":["missing",1]}"#, "{}", true, "[]"),
        (
            "all object keyword order",
            r#"{"required":["r"],"properties":{"x":{"type":"string"}},"propertyNames":{"pattern":"^z$"},"patternProperties":{"^x$":{"type":"boolean"}},"dependentRequired":{"x":["d",1]},"dependentSchemas":{"x":false},"additionalProperties":false,"minProperties":3,"maxProperties":0}"#,
            r#"{"x":1,"extra":true}"#,
            false,
            r#"[{"path":["r"],"message":"is required","keyword":"required"},{"path":["x"],"message":"expected string, received integer","keyword":"type","expectedTypes":["string"]},{"path":["x"],"message":"must match pattern","keyword":"pattern"},{"path":["extra"],"message":"must match pattern","keyword":"pattern"},{"path":["x"],"message":"expected boolean, received integer","keyword":"type","expectedTypes":["boolean"]},{"path":["d"],"message":"is required when \"x\" is present","keyword":"dependentRequired"},{"path":[],"message":"must not match false schema","keyword":"false"},{"path":["extra"],"message":"must not be present","keyword":"additionalProperties"},{"path":[],"message":"must have at least 3 properties","keyword":"minProperties"},{"path":[],"message":"must have at most 0 properties","keyword":"maxProperties"}]"#,
        ),
        (
            "pattern union multiple matches excludes extra",
            r#"{"patternProperties":{"^x":{"minimum":2},"x$":{"maximum":0}},"additionalProperties":false}"#,
            r#"{"x":1}"#,
            false,
            r#"[{"path":["x"],"message":"must be >= 2","keyword":"minimum"},{"path":["x"],"message":"must be <= 0","keyword":"maximum"}]"#,
        ),
        (
            "pattern compile error",
            r#"{"patternProperties":{"[":true}}"#,
            "{}",
            false,
            r#"[{"path":[],"message":"invalid patternProperties regex [","keyword":"patternProperties"}]"#,
        ),
        (
            "malformed additional schema validated",
            r#"{"additionalProperties":7}"#,
            r#"{"x":1}"#,
            false,
            r#"[{"path":["x"],"message":"schema must be an object or boolean","keyword":"schema"}]"#,
        ),
        (
            "schema-valued additional accepts",
            r#"{"additionalProperties":{"type":"number"},"minProperties":1,"maxProperties":1}"#,
            r#"{"x":1}"#,
            true,
            "[]",
        ),
        (
            "array keyword order and all duplicate pairs",
            r#"{"minItems":4,"maxItems":2,"uniqueItems":true,"prefixItems":[false],"items":false,"contains":false,"minContains":2,"maxContains":-1}"#,
            "[1,1,1]",
            false,
            r#"[{"path":[],"message":"must have at least 4 items","keyword":"minItems"},{"path":[],"message":"must have at most 2 items","keyword":"maxItems"},{"path":[1],"message":"must be unique","keyword":"uniqueItems"},{"path":[2],"message":"must be unique","keyword":"uniqueItems"},{"path":[2],"message":"must be unique","keyword":"uniqueItems"},{"path":[0],"message":"must not match false schema","keyword":"false"},{"path":[1],"message":"must not match false schema","keyword":"false"},{"path":[2],"message":"must not match false schema","keyword":"false"},{"path":[],"message":"must contain at least 2 matching item(s)","keyword":"contains"},{"path":[],"message":"must contain at most -1 matching item(s)","keyword":"maxContains"}]"#,
        ),
        (
            "array-valued items skips prefix validation",
            r#"{"prefixItems":[false],"items":[]}"#,
            "[1]",
            false,
            r#"[{"path":[],"message":"array-valued items is not valid in JSON Schema 2020-12; use prefixItems","keyword":"items"}]"#,
        ),
        ("items without prefix", r#"{"items":{"type":"number"}}"#, "[1,2]", true, "[]"),
        ("empty array contains min0", r#"{"contains":false,"minContains":0}"#, "[]", true, "[]"),
        (
            "contains upper bound",
            r#"{"contains":true,"maxContains":1}"#,
            "[1,2]",
            false,
            r#"[{"path":[],"message":"must contain at most 1 matching item(s)","keyword":"maxContains"}]"#,
        ),
        (
            "string keyword order",
            r#"{"minLength":3,"maxLength":1,"pattern":"^z$"}"#,
            r#""xy""#,
            false,
            r#"[{"path":[],"message":"must be at least 3 characters","keyword":"minLength"},{"path":[],"message":"must be at most 1 characters","keyword":"maxLength"},{"path":[],"message":"must match pattern","keyword":"pattern"}]"#,
        ),
        (
            "string pattern invalid",
            r#"{"pattern":"["}"#,
            r#""x""#,
            false,
            r#"[{"path":[],"message":"schema pattern is invalid","keyword":"pattern"}]"#,
        ),
        (
            "numeric keyword order",
            r#"{"minimum":4,"maximum":2,"exclusiveMinimum":3,"exclusiveMaximum":3,"multipleOf":2}"#,
            "3",
            false,
            r#"[{"path":[],"message":"must be >= 4","keyword":"minimum"},{"path":[],"message":"must be <= 2","keyword":"maximum"},{"path":[],"message":"must be > 3","keyword":"exclusiveMinimum"},{"path":[],"message":"must be < 3","keyword":"exclusiveMaximum"},{"path":[],"message":"must be a multiple of 2","keyword":"multipleOf"}]"#,
        ),
        (
            "legacy exclusive numeric boundary",
            r#"{"minimum":3,"maximum":3,"exclusiveMinimum":true,"exclusiveMaximum":true}"#,
            "3",
            false,
            r#"[{"path":[],"message":"must be > 3","keyword":"exclusiveMinimum"},{"path":[],"message":"must be < 3","keyword":"exclusiveMaximum"}]"#,
        ),
        ("fractional multiple epsilon", r#"{"multipleOf":0.1}"#, "0.3", true, "[]"),
        (
            "negative half multiple",
            r#"{"multipleOf":2}"#,
            "-3",
            false,
            r#"[{"path":[],"message":"must be a multiple of 2","keyword":"multipleOf"}]"#,
        ),
        ("nonpositive divisor ignored", r#"{"multipleOf":0}"#, "1", true, "[]"),
        ("format remains permissive", r#"{"format":"email"}"#, r#""invalid""#, true, "[]"),
    ];
    for (label, schema, value, success, issues) in cases {
        check(label, schema, value, success, issues);
    }
}

#[test]
fn wire_domain_prototypes_and_reference_identity_family() {
    let cases = [
        ("directional const inherited right accepts", r#"{"const":{"x":1}}"#, r#"{"__proto__":{}}"#, true, r#"[]"#),
        (
            "directional const inverse rejects",
            r#"{"const":{"__proto__":{}}}"#,
            r#"{"x":1}"#,
            false,
            r#"[{"path":[],"message":"must equal const value","keyword":"const"}]"#,
        ),
        (
            "directional nested const accepts",
            r#"{"const":{"node":{"x":1}}}"#,
            r#"{"node":{"__proto__":{}}}"#,
            true,
            r#"[]"#,
        ),
        (
            "directional nested const inverse rejects",
            r#"{"const":{"node":{"__proto__":{}}}}"#,
            r#"{"node":{"x":1}}"#,
            false,
            r#"[{"path":[],"message":"must equal const value","keyword":"const"}]"#,
        ),
        ("directional enum entry left accepts", r#"{"enum":[{"__proto__":{}}]}"#, r#"{"x":1}"#, true, r#"[]"#),
        (
            "directional enum entry left inverse rejects",
            r#"{"enum":[{"x":1}]}"#,
            r#"{"__proto__":{}}"#,
            false,
            r#"[{"path":[],"message":"must be one of the allowed enum values","keyword":"enum"}]"#,
        ),
        (
            "directional unique first left rejects",
            r#"{"uniqueItems":true}"#,
            r#"[{"__proto__":{}},{"x":1}]"#,
            false,
            r#"[{"path":[1],"message":"must be unique","keyword":"uniqueItems"}]"#,
        ),
        (
            "directional unique first left inverse accepts",
            r#"{"uniqueItems":true}"#,
            r#"[{"x":1},{"__proto__":{}}]"#,
            true,
            r#"[]"#,
        ),
        (
            "lone surrogate value and no-u pattern",
            r#"{"type":"string","pattern":"^\ud800$","minLength":1,"maxLength":1}"#,
            r#""\ud800""#,
            true,
            "[]",
        ),
        (
            "lone surrogate path",
            r#"{"propertyNames":false}"#,
            r#"{"\ud800":1}"#,
            false,
            r#"[{"path":["\ud800"],"message":"must not match false schema","keyword":"false"}]"#,
        ),
        (
            "lone surrogate pattern message",
            r#"{"patternProperties":{"[\ud800":true}}"#,
            "{}",
            false,
            r#"[{"path":[],"message":"invalid patternProperties regex [\ud800","keyword":"patternProperties"}]"#,
        ),
        (
            "lone surrogate dependency message",
            r#"{"dependentRequired":{"\ud800":["x"]}}"#,
            r#"{"\ud800":1}"#,
            false,
            r#"[{"path":["x"],"message":"is required when \"\ud800\" is present","keyword":"dependentRequired"}]"#,
        ),
        (
            "lone surrogate expected type retained",
            r#"{"type":"\ud800"}"#,
            "1",
            false,
            r#"[{"path":[],"message":"expected \ud800, received integer","keyword":"type","expectedTypes":["\ud800"]}]"#,
        ),
        (
            "astral UTF16 length and dot",
            r#"{"maxLength":1,"pattern":"^.$"}"#,
            r#""😀""#,
            false,
            r#"[{"path":[],"message":"must be at most 1 characters","keyword":"maxLength"},{"path":[],"message":"must match pattern","keyword":"pattern"}]"#,
        ),
        ("astral pattern code units", r#"{"pattern":"^😀$","minLength":2,"maxLength":2}"#, r#""😀""#, true, "[]"),
        (
            "ordinary inherited required presence",
            r#"{"required":["constructor","toString","__proto__"],"additionalProperties":false,"propertyNames":false,"maxProperties":0}"#,
            "{}",
            true,
            "[]",
        ),
        (
            "own constructor shadows intrinsic",
            r#"{"properties":{"constructor":{"type":"string"}}}"#,
            r#"{"constructor":"own"}"#,
            true,
            "[]",
        ),
        (
            "declared inherited function received type",
            r#"{"properties":{"toString":{"type":"string"}}}"#,
            "{}",
            false,
            r#"[{"path":["toString"],"message":"expected string, received function","keyword":"type","expectedTypes":["string"]}]"#,
        ),
        (
            "inherited dependent trigger",
            r#"{"dependentRequired":{"toString":["x"]}}"#,
            "{}",
            false,
            r#"[{"path":["x"],"message":"is required when \"toString\" is present","keyword":"dependentRequired"}]"#,
        ),
        (
            "inherited dependent schema trigger",
            r#"{"dependentSchemas":{"constructor":false}}"#,
            "{}",
            false,
            r#"[{"path":[],"message":"must not match false schema","keyword":"false"}]"#,
        ),
        (
            "enumeration numeric keys before insertion",
            r#"{"propertyNames":false}"#,
            r#"{"b":1,"10":1,"2":1,"a":1}"#,
            false,
            r#"[{"path":["2"],"message":"must not match false schema","keyword":"false"},{"path":["10"],"message":"must not match false schema","keyword":"false"},{"path":["b"],"message":"must not match false schema","keyword":"false"},{"path":["a"],"message":"must not match false schema","keyword":"false"}]"#,
        ),
        ("ObjectPrototype resolves as empty schema", r##"{"$ref":"#/__proto__","type":"invalid"}"##, "1", true, "[]"),
        (
            "ObjectPrototype own accessor resolves null",
            r##"{"$ref":"#/__proto__/__proto__"}"##,
            "1",
            false,
            r#"[{"path":[],"message":"schema must be an object or boolean","keyword":"schema"}]"#,
        ),
        (
            "array length reference resolves number",
            r##"{"$ref":"#/nodes/length","nodes":[]}"##,
            "1",
            false,
            r#"[{"path":[],"message":"schema must be an object or boolean","keyword":"schema"}]"#,
        ),
        (
            "array prototype reference resolves array",
            r##"{"$ref":"#/nodes/__proto__","nodes":[]}"##,
            "1",
            false,
            r#"[{"path":[],"message":"schema must be an object or boolean","keyword":"schema"}]"#,
        ),
        (
            "array prototype member reference resolves function",
            r##"{"$ref":"#/nodes/at","nodes":[]}"##,
            "1",
            false,
            r#"[{"path":[],"message":"schema must be an object or boolean","keyword":"schema"}]"#,
        ),
        (
            "array inherited object member",
            r##"{"$ref":"#/nodes/hasOwnProperty","nodes":[]}"##,
            "1",
            false,
            r#"[{"path":[],"message":"schema must be an object or boolean","keyword":"schema"}]"#,
        ),
        (
            "ref sibling skipped",
            r##"{"$ref":"#/$defs/x","$defs":{"x":true},"const":2,"allOf":[false]}"##,
            "1",
            true,
            "[]",
        ),
        ("ref to array element", r##"{"$ref":"#/nodes/0","nodes":[{"type":"number"}]}"##, "1", true, "[]"),
        (
            "noncanonical array index unresolved",
            r##"{"$ref":"#/nodes/00","nodes":[true]}"##,
            "1",
            false,
            r#"[{"path":[],"message":"unresolved reference #/nodes/00","keyword":"$ref"}]"#,
        ),
        ("pointer escapes order", r##"{"$ref":"#/$defs/a~1b~0c~01","$defs":{"a/b~c~1":true}}"##, "1", true, "[]"),
        ("pointer lone surrogate", r##"{"$ref":"#/$defs/\ud800","$defs":{"\ud800":true}}"##, "1", true, "[]"),
        (
            "external ref issue",
            r#"{"$ref":"external"}"#,
            "1",
            false,
            r#"[{"path":[],"message":"unresolved reference external","keyword":"$ref"}]"#,
        ),
        (
            "function cannot be traversed by ref",
            r##"{"$ref":"#/toString/x"}"##,
            "1",
            false,
            r#"[{"path":[],"message":"unresolved reference #/toString/x","keyword":"$ref"}]"#,
        ),
        (
            "own __proto__ data wins",
            r##"{"$ref":"#/__proto__","__proto__":false}"##,
            "1",
            false,
            r#"[{"path":[],"message":"must not match false schema","keyword":"false"}]"#,
        ),
        ("object self ref cycle", r##"{"$ref":"#"}"##, "{}", true, "[]"),
        ("array self ref cycle", r##"{"$ref":"#"}"##, "[]", true, "[]"),
        (
            "primitive self ref cap",
            r##"{"$ref":"#"}"##,
            "1",
            false,
            r#"[{"path":[],"message":"reference depth exceeded","keyword":"$ref"}]"#,
        ),
    ];
    for (label, schema, value, success, issues) in cases {
        check(label, schema, value, success, issues);
    }

    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let value = WireValue::Number(number);
        assert!(is_json_schema_value_valid(&parsed("{}"), &value));
        for kind in ["number", "integer"] {
            let result = validate_json_schema_value(
                &WireValue::object(vec![("type", WireValue::String(WireString::from(kind)))]),
                &value,
            );
            assert!(!result.success);
            assert_eq!(result.issues[0].message.to_utf8().unwrap(), format!("expected {kind}, received number"));
        }
        assert!(is_json_schema_value_valid(&WireValue::object(vec![("const", value.clone())]), &value));
        assert!(is_json_schema_value_valid(
            &WireValue::object(vec![("enum", WireValue::Array(vec![value.clone()]))]),
            &value
        ));
        assert!(is_json_schema_value_valid(&parsed(r#"{"minimum":0,"maximum":0}"#), &value));
    }
    assert_eq!(parsed("1e400").as_number(), Some(f64::INFINITY));
    assert!(is_json_schema_value_valid(&parsed("{}"), &parsed("1e400")));
    assert!(!is_json_schema_value_valid(&parsed(r#"{"type":"number"}"#), &parsed("1e400")));
    assert!(!is_json_schema_value_valid(
        &WireValue::object(vec![("const", WireValue::Number(0.0))]),
        &WireValue::Number(-0.0)
    ));
    assert!(is_json_schema_value_valid(
        &parsed(r#"{"uniqueItems":true}"#),
        &WireValue::Array(vec![WireValue::Number(0.0), WireValue::Number(-0.0)])
    ));
    assert!(!is_json_schema_value_valid(
        &parsed(r#"{"uniqueItems":true}"#),
        &WireValue::Array(vec![WireValue::Number(f64::NAN), WireValue::Number(f64::NAN)])
    ));
    let bounds = WireValue::object(vec![
        ("minimum", WireValue::Number(f64::INFINITY)),
        ("maximum", WireValue::Number(f64::NEG_INFINITY)),
        ("multipleOf", WireValue::Number(f64::NAN)),
    ]);
    let result = validate_json_schema_value(&bounds, &WireValue::Number(1.0));
    assert_eq!(
        result.issues.iter().map(|issue| issue.message.to_utf8().unwrap()).collect::<Vec<_>>(),
        ["must be >= Infinity", "must be <= -Infinity"]
    );
    assert_eq!(number_message(f64::NAN), "NaN");
    assert_eq!(number_message(-0.0), "0");
    let length = WireValue::object(vec![("minLength", WireValue::Number(f64::INFINITY))]);
    assert!(
        validate_json_schema_value(&length, &parsed(r#""x""#)).issues[0]
            .message
            .equals_ascii("must be at least Infinity characters")
    );

    // Stable distinct borrowed node IDs: equal values must not inherit an
    // active reference pair belonging to another node.
    let left = parsed("{}");
    let right = parsed("{}");
    let schema = parsed(r##"{"$ref":"#/$defs/x","$defs":{"x":{"type":"string"}}}"##);
    let state = AtomicBool::new(false);
    let mut warn = |_: &str| {};
    let mut context = ValidationContext {
        root: ValueRef::Wire(&schema),
        seen_pairs: HashSet::new(),
        ref_depth: 0,
        warning_seen: &state,
        warn: &mut warn,
    };
    let read = ReadContext::Wire;
    context.seen_pairs.insert((WireString::from("#/$defs/x"), read.identity(&ValueRef::Wire(&left)).unwrap().unwrap()));
    let mut issues = Vec::new();
    assert!(
        !validate_node(ValueRef::Wire(&schema), ValueRef::Wire(&right), &[], &read, &mut context, &mut issues).unwrap()
    );
    assert_eq!(issues.len(), 1);
    assert_eq!(read.identity(&ValueRef::Wire(&left)).unwrap(), read.identity(&ValueRef::Wire(&left)).unwrap());
    assert_ne!(read.identity(&ValueRef::Wire(&left)).unwrap(), read.identity(&ValueRef::Wire(&right)).unwrap());
    assert!(read.equal(&ValueRef::ObjectPrototype, &ValueRef::Wire(&left)).unwrap());
    assert!(read.equal(&ValueRef::ArrayPrototype, &ValueRef::Wire(&parsed("[]"))).unwrap());
}

#[test]
fn warning_once_fresh_context_and_serde_conversion_family() {
    let warning_seen = AtomicBool::new(false);
    let mut warnings = Vec::new();
    let schema = parsed(r#"{"allOf":[{"unevaluatedProperties":false},{"unevaluatedItems":false}]}"#);
    for _ in 0..2 {
        let result = validate_with_warning(&schema, &parsed("{}"), &warning_seen, &mut |message| {
            warnings.push(message.to_owned())
        });
        assert!(result.success);
        assert!(result.issues.is_empty());
    }
    assert_eq!(warnings, [UNEVALUATED_WARNING]);
    // An early type failure does not consume the warning guard.
    let isolated = AtomicBool::new(false);
    let mut warnings = Vec::new();
    let result = validate_with_warning(
        &parsed(r#"{"type":"string","unevaluatedItems":false}"#),
        &parsed("1"),
        &isolated,
        &mut |message| warnings.push(message.to_owned()),
    );
    assert!(!result.success);
    assert!(warnings.is_empty());
    assert!(!isolated.load(Ordering::Relaxed));
    let schema = serde_json::json!({"type":"integer","const":9007199254740992_u64});
    let value = serde_json::json!(9007199254740993_u64);
    // Both integers become the same ECMAScript f64 at the facade boundary.
    assert!(validate_serde_json_schema_value(&schema, &value).success);
    let facade = validate_serde_json_schema_value(&serde_json::json!({"maxLength":1}), &serde_json::json!("😀"));
    assert!(!facade.success);
    assert!(facade.issues[0].message.equals_ascii("must be at most 1 characters"));
    // More than the old 256-level global cap, with no invented Native cap.
    let mut schema = WireValue::Bool(true);
    for _ in 0..270 {
        schema = WireValue::object(vec![("allOf", WireValue::Array(vec![schema]))]);
    }
    eprintln!("schema checker diagnostic: BEFORE270");
    assert!(is_json_schema_value_valid(&schema, &parsed("1")));
    eprintln!("schema checker diagnostic: AFTER270");
    graph_actual_effects_and_carriers();
    eprintln!("schema checker diagnostic: AFTERGraphEffects");
}

use ara_rpc::value_heap::{
    CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, PropertyKey, RuntimeReply, RuntimeRequest, ValueHeap,
    ValueRuntime, ValueSnapshot,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};

/// Receipt conversion is used only for the original immutable Wire fixtures.
/// It is not the actual checker path and is never used for alias/effect cases.
fn graph_fixture_result(
    heap: &Arc<ValueHeap>,
    result: JsonSchemaValidationResult<ValueHandle, ValueHandle>,
) -> WireValue {
    fn scalar(heap: &Arc<ValueHeap>, value: &ValueHandle) -> WireValue {
        match heap.snapshot(value).unwrap() {
            ValueSnapshot::String(value) => WireValue::String(value),
            ValueSnapshot::Number(value) => WireValue::Number(value),
            other => panic!("fixture scalar: {other:?}"),
        }
    }
    let issues = result
        .issues
        .into_iter()
        .map(|issue| {
            let mut object = WireValue::object(vec![
                ("path", WireValue::Array(issue.path.iter().map(|value| scalar(heap, value)).collect())),
                ("message", WireValue::String(issue.message)),
            ]);
            if let Some(types) = issue.expected_types {
                let length = heap.array_length(&types, &NoRuntime).unwrap();
                object.insert(
                    "expectedTypes",
                    WireValue::Array(
                        (0..length)
                            .map(|index| {
                                scalar(
                                    heap,
                                    &heap
                                        .get(
                                            &types,
                                            &PropertyKey::from(WireString::from(index.to_string())),
                                            &NoRuntime,
                                        )
                                        .unwrap(),
                                )
                            })
                            .collect(),
                    ),
                );
            }
            if let Some(keyword) = issue.keyword {
                object.insert("keyword", WireValue::String(keyword.into()));
            }
            if issue.from_union_branch {
                object.insert("fromUnionBranch", WireValue::Bool(true));
            }
            object
        })
        .collect();
    WireValue::object(vec![("success", WireValue::Bool(result.success)), ("issues", WireValue::Array(issues))])
}

fn graph_fixture(schema: &WireValue, value: &WireValue) -> WireValue {
    let heap = ValueHeap::new().unwrap();
    let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
    let schema = heap.import_strict(schema.clone()).unwrap();
    let value = heap.import_strict(value.clone()).unwrap();
    let result = validate_graph_json_schema_value(&factory, &schema, &value).unwrap();
    graph_fixture_result(&heap, result)
}

#[derive(Clone)]
enum Behavior {
    Value(ValueHandle),
    Throw(ValueHandle),
    Missing,
}
struct FixtureRuntime {
    heap: Arc<ValueHeap>,
    responses: Mutex<HashMap<u64, VecDeque<Behavior>>>,
    trace: Mutex<Vec<u64>>,
    some_entry: Mutex<Option<ValueHandle>>,
}
impl FixtureRuntime {
    fn new(heap: &Arc<ValueHeap>, entries: Vec<(u64, Vec<Behavior>)>) -> Arc<Self> {
        Arc::new(Self {
            heap: heap.clone(),
            responses: Mutex::new(entries.into_iter().map(|(key, values)| (key, values.into())).collect()),
            trace: Mutex::new(Vec::new()),
            some_entry: Mutex::new(None),
        })
    }
    fn reply(&self, capability: CapabilityId) -> Result<RuntimeReply, ValueError> {
        self.trace.lock().unwrap().push(capability.0);
        let behavior = self.responses.lock().unwrap().get_mut(&capability.0).and_then(VecDeque::pop_front);
        match behavior {
            Some(Behavior::Value(value)) => Ok(RuntimeReply::Value(value)),
            Some(Behavior::Throw(value)) => Err(ValueError::Thrown(value)),
            Some(Behavior::Missing) | None => Err(ValueError::MissingCapability(capability)),
        }
    }
}
impl ValueRuntime for FixtureRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        self.reply(request.capability())
    }
    fn resolve_call(
        &self,
        capability: CapabilityId,
        _receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<RuntimeReply, ValueError> {
        if capability == CapabilityId(11) {
            let entry = self.some_entry.lock().unwrap().clone().unwrap();
            let callback = self.heap.call(&arguments[0], &self.heap.undefined()?, &[entry], effective_runtime)?;
            assert!(matches!(self.heap.snapshot(&callback)?, ValueSnapshot::Boolean(false)));
        }
        self.reply(capability)
    }
}
fn accessor(heap: &Arc<ValueHeap>, object: &ValueHandle, key: &str, capability: u64) {
    heap.define_own(
        object,
        key.into(),
        PropertyDescriptor::Accessor {
            get: Some(CallableTarget::Host(CapabilityId(capability))),
            set: None,
            enumerable: true,
            configurable: true,
        },
        &NoRuntime,
    )
    .unwrap();
}
fn own(heap: &Arc<ValueHeap>, object: &ValueHandle, key: &str, value: ValueHandle) {
    heap.define_own_data(object, key.into(), value, &NoRuntime).unwrap();
}

/// Extended inside an existing coarse family; Root executes the module once.
fn graph_actual_effects_and_carriers() {
    graph_retained_checker_callbacks();
    // filter may return any actual value. The selected expectedTypes object
    // retains identity, and a Host some can call the real supplied predicate.
    let heap = ValueHeap::new().unwrap();
    let raw = heap.array(Vec::new()).unwrap();
    let selected = heap.object().unwrap();
    own(&heap, &raw, "filter", heap.function(CapabilityId(10)).unwrap());
    own(&heap, &selected, "length", heap.number(1.0).unwrap());
    own(&heap, &selected, "some", heap.function(CapabilityId(11)).unwrap());
    own(&heap, &selected, "join", heap.function(CapabilityId(12)).unwrap());
    let runtime = FixtureRuntime::new(
        &heap,
        vec![
            (10, vec![Behavior::Value(selected.clone())]),
            (11, vec![Behavior::Value(heap.boolean(false).unwrap())]),
            (12, vec![Behavior::Value(heap.string("custom".into()).unwrap())]),
        ],
    );
    *runtime.some_entry.lock().unwrap() = Some(heap.number(7.0).unwrap());
    let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
    let schema = heap.object().unwrap();
    own(&heap, &schema, "type", raw);
    let result = validate_graph_json_schema_value(&factory, &schema, &heap.number(1.0).unwrap()).unwrap();
    assert!(!result.success);
    assert!(result.issues[0].message.equals_ascii("expected custom, received integer"));
    assert!(result.issues[0].expected_types.as_ref().unwrap().same_identity(&selected));
    assert_eq!(*runtime.trace.lock().unwrap(), [10, 11, 12]);

    // every's custom truthy result does not flatten the fresh required iterator.
    // in converts Symbol/number keys separately; issue paths retain exact values.
    let heap = ValueHeap::new().unwrap();
    let checked = heap.array(Vec::new()).unwrap();
    own(&heap, &checked, "every", heap.function(CapabilityId(20)).unwrap());
    let symbol = heap.symbol(Some("required".into())).unwrap();
    let number = heap.number(7.0).unwrap();
    let iterated = heap.array(vec![symbol.clone(), number.clone()]).unwrap();
    let runtime = FixtureRuntime::new(
        &heap,
        vec![
            (100, vec![Behavior::Value(checked), Behavior::Value(iterated)]),
            (20, vec![Behavior::Value(heap.boolean(true).unwrap())]),
        ],
    );
    let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
    let schema = heap.object().unwrap();
    accessor(&heap, &schema, "required", 100);
    let result = validate_graph_json_schema_value(&factory, &schema, &heap.object().unwrap()).unwrap();
    assert!(!result.success);
    assert_eq!(result.issues.len(), 2);
    assert!(result.issues[0].path[0].same_identity(&symbol));
    assert!(result.issues[1].path[0].same_identity(&number));
    assert_eq!(*runtime.trace.lock().unwrap(), [100, 20, 100]);

    // Numeric guard, comparison and interpolation are three independent Gets.
    // BigInt comparison stays exact rather than incorrectly ToNumber(bigint).
    let heap = ValueHeap::new().unwrap();
    let runtime = FixtureRuntime::new(
        &heap,
        vec![(
            100,
            vec![
                Behavior::Value(heap.number(5.0).unwrap()),
                Behavior::Value(heap.bigint_decimal("10").unwrap()),
                Behavior::Value(heap.string("changed".into()).unwrap()),
            ],
        )],
    );
    let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
    let schema = heap.object().unwrap();
    accessor(&heap, &schema, "minimum", 100);
    let result = validate_graph_json_schema_value(&factory, &schema, &heap.number(1.0).unwrap()).unwrap();
    assert!(!result.success);
    assert!(result.issues[0].message.equals_ascii("must be >= changed"));
    assert_eq!(*runtime.trace.lock().unwrap(), [100, 100, 100]);

    // Only the second pattern Get is in the Native try. Native throw becomes
    // a pattern issue; a missing or foreign capability remains an error.
    for mode in 0..3 {
        let heap = ValueHeap::new().unwrap();
        let thrown = if mode == 2 { ValueHeap::new().unwrap().object().unwrap() } else { heap.object().unwrap() };
        let failure = if mode == 1 { Behavior::Missing } else { Behavior::Throw(thrown) };
        let runtime =
            FixtureRuntime::new(&heap, vec![(101, vec![Behavior::Value(heap.string("x".into()).unwrap()), failure])]);
        let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
        let schema = heap.object().unwrap();
        accessor(&heap, &schema, "pattern", 101);
        let result = validate_graph_json_schema_value(&factory, &schema, &heap.string("x".into()).unwrap());
        match mode {
            0 => assert!(result.unwrap().issues[0].message.equals_ascii("schema pattern is invalid")),
            1 => assert!(matches!(result, Err(ValueError::MissingCapability(CapabilityId(101))))),
            _ => assert!(matches!(result, Err(ValueError::ForeignHeap))),
        }
        assert_eq!(*runtime.trace.lock().unwrap(), [101, 101]);
    }

    // Actual for-in properties and Object.keys known set remain different.
    let heap = ValueHeap::new().unwrap();
    let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
    let inherited = heap.object().unwrap();
    own(&heap, &inherited, "x", heap.boolean(false).unwrap());
    let properties = heap.object_with_prototype(Some(&inherited)).unwrap();
    let schema = heap.object().unwrap();
    own(&heap, &schema, "properties", properties);
    own(&heap, &schema, "additionalProperties", heap.boolean(false).unwrap());
    let value = heap.import_strict(parsed(r#"{"x":1}"#)).unwrap();
    let result = validate_graph_json_schema_value(&factory, &schema, &value).unwrap();
    assert_eq!(
        result.issues.iter().map(|issue| issue.keyword).collect::<Vec<_>>(),
        [Some("false"), Some("additionalProperties")]
    );

    // Array holes use inherited indices and live Get, never own-only slots.
    let heap = ValueHeap::new().unwrap();
    let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
    own(&heap, &heap.array_prototype(), "0", heap.number(4.0).unwrap());
    let enums = heap.array_with_length(1).unwrap();
    let schema = heap.object().unwrap();
    own(&heap, &schema, "enum", enums);
    assert!(validate_graph_json_schema_value(&factory, &schema, &heap.number(4.0).unwrap()).unwrap().success);

    // Same alias exits Object.is. A reached distinct cycle is an explicit
    // unsupported Native termination boundary, never false/true or overflow.
    let left = heap.object().unwrap();
    let right = heap.object().unwrap();
    own(&heap, &left, "self", left.clone());
    own(&heap, &right, "self", right.clone());
    let schema = heap.object().unwrap();
    own(&heap, &schema, "const", left.clone());
    assert!(validate_graph_json_schema_value(&factory, &schema, &left).unwrap().success);
    assert!(matches!(
        validate_graph_json_schema_value(&factory, &schema, &right),
        Err(ValueError::UnsupportedOperation("Fixed Native equality reached a distinct cyclic pair"))
    ));

    // For-of closes after a body throw. Native close errors retain the original
    // thrown handle; Host close failures retain both receipts in one carrier.
    for host_close in [false, true] {
        let heap = ValueHeap::new().unwrap();
        let original = heap.object().unwrap();
        let secondary = heap.object().unwrap();
        let branch = heap.object().unwrap();
        accessor(&heap, &branch, "$ref", 201);
        let iterator = heap.object().unwrap();
        own(&heap, &iterator, "next", heap.function(CapabilityId(203)).unwrap());
        own(&heap, &iterator, "return", heap.function(CapabilityId(204)).unwrap());
        let next_result = heap.object().unwrap();
        own(&heap, &next_result, "done", heap.boolean(false).unwrap());
        own(&heap, &next_result, "value", branch);
        let branches = heap.array(Vec::new()).unwrap();
        heap.define_own_data(&branches, heap.iterator_key(), heap.function(CapabilityId(202)).unwrap(), &NoRuntime)
            .unwrap();
        let runtime = FixtureRuntime::new(
            &heap,
            vec![
                (201, vec![Behavior::Throw(original.clone())]),
                (202, vec![Behavior::Value(iterator)]),
                (203, vec![Behavior::Value(next_result)]),
                (204, vec![if host_close { Behavior::Missing } else { Behavior::Throw(secondary) }]),
            ],
        );
        let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
        let schema = heap.object().unwrap();
        own(&heap, &schema, "allOf", branches);
        let error = validate_graph_json_schema_value(&factory, &schema, &heap.null().unwrap()).unwrap_err();
        if host_close {
            assert!(!error.is_native_exception());
            let ValueError::IteratorClose { original: body, close } = error else {
                panic!("missing dual receipt");
            };
            assert!(matches!(*body, ValueError::Thrown(value) if value.same_identity(&original)));
            assert!(matches!(*close, ValueError::MissingCapability(CapabilityId(204))));
        } else {
            assert!(matches!(error, ValueError::Thrown(value) if value.same_identity(&original)));
        }
        assert_eq!(*runtime.trace.lock().unwrap(), [202, 203, 201, 204]);
    }

    // Nested cursors cross a live $ref lease and required/property Gets.
    // Body and iterator-next errors have different close/unwind semantics.
    for mode in 0..3 {
        let heap = ValueHeap::new().unwrap();
        let original = heap.object().unwrap();
        let inner_throw = heap.object().unwrap();
        let outer_throw = heap.object().unwrap();
        let data = heap.object().unwrap();
        let child_data = heap.object().unwrap();
        accessor(&heap, &data, "next", 443);
        let child_schema = heap.object().unwrap();
        accessor(&heap, &child_schema, "$ref", 444);
        let properties = heap.object().unwrap();
        accessor(&heap, &properties, "next", 442);
        let required = heap.array(vec![heap.string("next".into()).unwrap()]).unwrap();
        let leaf = heap.object().unwrap();
        accessor(&heap, &leaf, "properties", 440);
        accessor(&heap, &leaf, "required", 441);
        let inner_iterator = heap.object().unwrap();
        own(&heap, &inner_iterator, "next", heap.function(CapabilityId(431)).unwrap());
        own(&heap, &inner_iterator, "return", heap.function(CapabilityId(432)).unwrap());
        let inner_next = heap.object().unwrap();
        own(&heap, &inner_next, "done", heap.boolean(false).unwrap());
        own(&heap, &inner_next, "value", leaf);
        let inner = heap.array(Vec::new()).unwrap();
        heap.define_own_data(&inner, heap.iterator_key(), heap.function(CapabilityId(430)).unwrap(), &NoRuntime).unwrap();
        let target = heap.object().unwrap();
        accessor(&heap, &target, "anyOf", 420);
        let definitions = heap.object().unwrap();
        own(&heap, &definitions, "T", target);
        let reference = heap.object().unwrap();
        accessor(&heap, &reference, "$ref", 410);
        let outer_iterator = heap.object().unwrap();
        own(&heap, &outer_iterator, "next", heap.function(CapabilityId(401)).unwrap());
        own(&heap, &outer_iterator, "return", heap.function(CapabilityId(402)).unwrap());
        let outer_next = heap.object().unwrap();
        own(&heap, &outer_next, "done", heap.boolean(false).unwrap());
        own(&heap, &outer_next, "value", reference.clone());
        let outer = heap.array(Vec::new()).unwrap();
        heap.define_own_data(&outer, heap.iterator_key(), heap.function(CapabilityId(400)).unwrap(), &NoRuntime).unwrap();
        let schema = heap.object().unwrap();
        own(&heap, &schema, "$defs", definitions.clone());
        own(&heap, &schema, "allOf", outer);
        let ref_text = heap.string("#/$defs/T".into()).unwrap();
        let runtime = FixtureRuntime::new(&heap, vec![
            (400, vec![Behavior::Value(outer_iterator)]),
            (401, vec![Behavior::Value(outer_next)]),
            (402, vec![Behavior::Throw(outer_throw.clone())]),
            (410, vec![Behavior::Value(ref_text.clone()), Behavior::Value(ref_text)]),
            (420, vec![Behavior::Value(inner)]),
            (430, vec![Behavior::Value(inner_iterator)]),
            (431, vec![if mode == 2 { Behavior::Throw(original.clone()) } else { Behavior::Value(inner_next) }]),
            (432, vec![if mode == 1 { Behavior::Missing } else { Behavior::Throw(inner_throw) }]),
            (440, vec![Behavior::Value(properties.clone()), Behavior::Value(properties)]),
            (441, vec![Behavior::Value(required.clone()), Behavior::Value(required)]),
            (442, vec![Behavior::Value(child_schema)]),
            (443, vec![Behavior::Value(child_data)]),
            (444, vec![Behavior::Throw(original.clone())]),
        ]);
        let read = ReadContext::Graph { factory: GraphExecution::new(&heap, runtime.as_ref()), runtime: runtime.as_ref() };
        let warning = AtomicBool::new(false);
        let mut warn = |_: &str| {};
        let mut context = ValidationContext { root: ValueRef::Graph(schema.clone()), seen_pairs: HashSet::new(),
            ref_depth: 0, warning_seen: &warning, warn: &mut warn };
        let mut issues = Vec::new();
        let error = validate_node(ValueRef::Graph(schema), ValueRef::Graph(data.clone()), &[], &read, &mut context, &mut issues).unwrap_err();
        if mode == 1 {
            let ValueError::IteratorClose { original: inner, close: outer } = error else { panic!("missing outer close receipt") };
            assert!(matches!(*outer, ValueError::Thrown(value) if value.same_identity(&outer_throw)));
            let ValueError::IteratorClose { original: body, close } = *inner else { panic!("missing inner close receipt") };
            assert!(matches!(*body, ValueError::Thrown(value) if value.same_identity(&original)));
            assert!(matches!(*close, ValueError::MissingCapability(CapabilityId(432))));
        } else {
            assert!(matches!(error, ValueError::Thrown(value) if value.same_identity(&original)));
        }
        assert!(issues.is_empty());
        assert!(context.seen_pairs.is_empty());
        assert_eq!(context.ref_depth, 0);
        let mut expected = if mode == 2 {
            vec![400, 401, 410, 420, 430, 431, 402]
        } else {
            vec![400, 401, 410, 420, 430, 431, 440, 440, 441, 441, 442, 443, 444, 432, 402]
        };
        assert_eq!(*runtime.trace.lock().unwrap(), expected);
        // Reuse the exact reference/value pair and context after abrupt unwind.
        // A leaked pair would incorrectly return true instead of checking false.
        own(&heap, &definitions, "T", heap.boolean(false).unwrap());
        assert!(!validate_node(ValueRef::Graph(reference), ValueRef::Graph(data), &[], &read, &mut context, &mut issues).unwrap());
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].keyword, Some("false"));
        assert!(context.seen_pairs.is_empty());
        assert_eq!(context.ref_depth, 0);
        expected.push(410);
        assert_eq!(*runtime.trace.lock().unwrap(), expected);
    }


    // Checked public entry admits both roots before any callback.
    let heap = ValueHeap::new().unwrap();
    let runtime = FixtureRuntime::new(&heap, Vec::new());
    let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
    let foreign = ValueHeap::new().unwrap().object().unwrap();
    assert!(matches!(
        validate_graph_json_schema_value(&factory, &foreign, &heap.null().unwrap()),
        Err(ValueError::ForeignHeap)
    ));
    assert!(runtime.trace.lock().unwrap().is_empty());

    // Actual scalar domain keeps typeof, f64 states and UTF16 rather than
    // narrowing through JSON. Boxed values remain objects.
    let heap = ValueHeap::new().unwrap();
    let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
    let schema = heap.import_strict(parsed(r#"{"type":"string"}"#)).unwrap();
    for (value, received) in [
        (heap.bigint_decimal("9007199254740993").unwrap(), "bigint"),
        (heap.symbol(None).unwrap(), "symbol"),
        (heap.function(CapabilityId(999)).unwrap(), "function"),
        (heap.number(f64::NAN).unwrap(), "number"),
        (heap.number(f64::INFINITY).unwrap(), "number"),
    ] {
        let result = validate_graph_json_schema_value(&factory, &schema, &value).unwrap();
        assert!(!result.success);
        assert_eq!(result.issues[0].message.to_utf8().unwrap(), format!("expected string, received {received}"));
    }
    let unpaired = heap.string(WireString::from_units(vec![0xd800])).unwrap();
    assert!(validate_graph_json_schema_value(&factory, &schema, &unpaired).unwrap().success);
    let schema = heap.object().unwrap();
    own(&heap, &schema, "const", heap.number(0.0).unwrap());
    assert!(!validate_graph_json_schema_value(&factory, &schema, &heap.number(-0.0).unwrap()).unwrap().success);
}

// The actual selected source methods supply these Functions. No spec is invoked
// directly, and the original factory/runtime are gone before any retained call.
struct CheckerCaptureRuntime {
    heap: Weak<ValueHeap>,
    original: ValueHandle,
    saved: Mutex<Vec<ValueHandle>>,
}
impl ValueRuntime for CheckerCaptureRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        Err(ValueError::MissingCapability(request.capability()))
    }
    fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<RuntimeReply, ValueError> {
        if capability != CapabilityId(9600) {
            return Err(ValueError::MissingCapability(capability));
        }
        self.saved.lock().unwrap().push(arguments[0].clone());
        self.heap
            .upgrade()
            .unwrap()
            .call(&self.original, receiver, arguments, effective_runtime)
            .map(RuntimeReply::Value)
    }
}
fn capture_checker_functions(
    heap: &Arc<ValueHeap>,
    method: &str,
    action: impl FnOnce(&GraphValueFactory),
) -> Vec<ValueHandle> {
    let prototype = heap.array_prototype();
    let original = heap.get(&prototype, &method.into(), &NoRuntime).unwrap();
    let runtime = Arc::new(CheckerCaptureRuntime {
        heap: Arc::downgrade(heap),
        original: original.clone(),
        saved: Mutex::new(Vec::new()),
    });
    let weak_runtime = Arc::downgrade(&runtime);
    let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
    own(heap, &prototype, method, heap.function(CapabilityId(9600)).unwrap());
    action(&factory);
    own(heap, &prototype, method, original);
    drop(factory);
    let saved = std::mem::take(&mut *runtime.saved.lock().unwrap());
    drop(runtime);
    assert!(weak_runtime.upgrade().is_none());
    saved
}
fn retained_checker_boolean(
    heap: &Arc<ValueHeap>,
    function: &ValueHandle,
    argument: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> bool {
    let result = heap.call(function, &heap.undefined().unwrap(), std::slice::from_ref(argument), runtime).unwrap();
    match heap.snapshot(&result).unwrap() {
        ValueSnapshot::Boolean(result) => result,
        other => panic!("retained checker expected source Boolean: {other:?}"),
    }
}
struct CheckerLaterRuntime {
    heap: Weak<ValueHeap>,
    events: Mutex<Vec<u64>>,
    reenter: Mutex<Option<(ValueHandle, ValueHandle)>>,
    value: ValueHandle,
    thrown: ValueHandle,
}
impl ValueRuntime for CheckerLaterRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        Err(ValueError::MissingCapability(request.capability()))
    }
    fn resolve_call(
        &self,
        capability: CapabilityId,
        _receiver: &ValueHandle,
        _arguments: &[ValueHandle],
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<RuntimeReply, ValueError> {
        self.events.lock().unwrap().push(capability.0);
        if capability == CapabilityId(9601) {
            let reenter = self.reenter.lock().unwrap().take();
            if let Some((function, argument)) = reenter {
                assert!(retained_checker_boolean(
                    &self.heap.upgrade().unwrap(),
                    &function,
                    &argument,
                    effective_runtime
                ));
            }
        }
        match capability.0 {
            9601 | 9602 => Ok(RuntimeReply::Value(self.value.clone())),
            9603 => Err(ValueError::Thrown(self.thrown.clone())),
            _ => Err(ValueError::MissingCapability(capability)),
        }
    }
}
fn graph_retained_checker_callbacks() {
    let weak_heap = {
        let heap = ValueHeap::new().unwrap();
        let weak = Arc::downgrade(&heap);

        // Fixed checker148: two actual invocations create distinct fresh Arrows.
        let schema = heap.import_serde(&serde_json::json!({"type":["string","integer"]})).unwrap();
        let number = heap.number(4.0).unwrap();
        let filtered = capture_checker_functions(&heap, "filter", |factory| {
            for _ in 0..2 {
                assert!(validate_graph_json_schema_value(factory, &schema, &number).unwrap().success);
            }
        });
        assert_eq!(filtered.len(), 2);
        assert!(!filtered[0].same_identity(&filtered[1]));
        assert!(retained_checker_boolean(&heap, &filtered[0], &heap.string("string".into()).unwrap(), &NoRuntime));
        assert!(!retained_checker_boolean(&heap, &filtered[1], &number, &NoRuntime));

        // Fixed checker179: capture actual required.every supplied by the checker.
        let required_schema = heap.import_serde(&serde_json::json!({"required":["x"]})).unwrap();
        let required_value = heap.import_serde(&serde_json::json!({"x":1})).unwrap();
        let required = capture_checker_functions(&heap, "every", |factory| {
            assert!(validate_graph_json_schema_value(factory, &required_schema, &required_value).unwrap().success);
        });
        assert_eq!(required.len(), 1);
        assert!(retained_checker_boolean(&heap, &required[0], &heap.string("x".into()).unwrap(), &NoRuntime));
        assert!(!retained_checker_boolean(&heap, &required[0], &heap.undefined().unwrap(), &NoRuntime));

        // Fixed checker241: the original actual value is captured, not its reader.
        let type_schema = heap.import_serde(&serde_json::json!({"type":"integer"})).unwrap();
        let matching_type = capture_checker_functions(&heap, "some", |factory| {
            assert!(validate_graph_json_schema_value(factory, &type_schema, &number).unwrap().success);
        });
        assert_eq!(matching_type.len(), 1);
        assert!(retained_checker_boolean(
            &heap,
            &matching_type[0],
            &heap.string("integer".into()).unwrap(),
            &NoRuntime
        ));
        assert!(!retained_checker_boolean(
            &heap,
            &matching_type[0],
            &heap.string("string".into()).unwrap(),
            &NoRuntime
        ));

        // Fixed checker254 runs in the branch, then93 classifies that failed
        // branch's actual enum discriminator. Their fresh source Functions differ.
        let union_schema = heap
            .import_serde(&serde_json::json!({"anyOf":[{
                "properties":{"tag":{"enum":[{"field":"old"}]}},"required":["need"]
            }]}))
            .unwrap();
        let parent = heap.import_serde(&serde_json::json!({"tag":{"field":"old"}})).unwrap();
        let candidate = heap.get(&parent, &"tag".into(), &NoRuntime).unwrap();
        let enums = capture_checker_functions(&heap, "some", |factory| {
            assert!(!validate_graph_json_schema_value(factory, &union_schema, &parent).unwrap().success);
        });
        assert_eq!(enums.len(), 2); //254 then93; no type-some is present here.
        assert!(!enums[0].same_identity(&enums[1]));
        own(&heap, &candidate, "field", heap.string("new".into()).unwrap());
        let replacement = heap.import_serde(&serde_json::json!({"field":"replacement"})).unwrap();
        own(&heap, &parent, "tag", replacement);
        let entry = heap.import_serde(&serde_json::json!({"field":"new"})).unwrap();
        for function in &enums {
            assert!(retained_checker_boolean(&heap, function, &entry, &NoRuntime));
        }

        // Later equality uses current getters. One bounded same-function
        // reentry completes after the fixture's mutex has been released.
        accessor(&heap, &entry, "field", 9601);
        accessor(&heap, &candidate, "field", 9602);
        let runtime = CheckerLaterRuntime {
            heap: Arc::downgrade(&heap),
            events: Mutex::new(Vec::new()),
            reenter: Mutex::new(Some((enums[1].clone(), entry.clone()))),
            value: heap.string("new".into()).unwrap(),
            thrown: heap.object().unwrap(),
        };
        assert!(retained_checker_boolean(&heap, &enums[1], &entry, &runtime));
        assert_eq!(*runtime.events.lock().unwrap(), vec![9601, 9601, 9602, 9602]);
        accessor(&heap, &entry, "field", 9603);
        assert!(matches!(heap.call(&enums[1], &heap.undefined().unwrap(), std::slice::from_ref(&entry), &runtime),
            Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown)));
        assert_eq!(*runtime.events.lock().unwrap(), vec![9601, 9601, 9602, 9602, 9603]);
        weak
    };
    assert!(weak_heap.upgrade().is_none());
}
