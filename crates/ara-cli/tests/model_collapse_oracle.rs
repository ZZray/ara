//! Stateful replay of unchanged fixed OMP collapse.ts, including UTF-16,
//! undefined own properties, non-finite numbers, and retained array identities.
use ara_cli::{
    catalog_rules::RevisionTerm,
    js_regex::JsRegExp,
    model_collapse::{
        CollapseError, CollapseRuntime, InheritedTableProperty, ReviewedCollapseLookup, SpecRef, VariantAliasSources,
        VariantCollapseTable, VariantFamilyTemplate, VariantSpec,
    },
};
use ara_rpc::{WireString, WireValue as W};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

const COUNTS: &[(&str, usize)] = &[
    ("reviewed-table", 11),
    ("reviewed-family", 1642),
    ("stale-family", 67),
    ("extra-alias", 2),
    ("reviewed-alias", 67),
    ("reviewed-template", 18),
    ("template-first", 8),
    ("default-repair", 10),
    ("provider-alias", 17),
    ("thinking-pair", 12),
    ("thinking-token", 6),
    ("cursor-gate", 25),
    ("alias-state", 5),
    ("retarget", 1),
    ("caller-contract", 6),
    ("custom-table", 8),
    ("custom-template", 4),
    ("lossless-mandatory", 15),
    ("inherited-marker", 3),
    ("catalog-built", 1),
    ("catalog-provider", 67),
];
const EXPORTS: &[(&str, usize)] = &[
    ("reviewedCollapseTable", 14),
    ("deriveThinkingPairFamilies", 40),
    ("isCollapsedVariantSpec", 73),
    ("collapseVariants", 3626),
    ("collapseBuiltVariants", 84),
    ("resolveVariantSelector", 436),
    ("resolveBareVariantSelector", 373),
    ("getVariantAliasSources", 216),
];
fn array(v: &W) -> &[W] {
    v.as_array().expect("corpus array")
}
fn field<'a>(v: &'a W, k: &str) -> &'a W {
    v.get(k).expect(k)
}
fn text(v: &W) -> String {
    v.as_string().expect("corpus string").to_utf8().expect("ASCII metadata")
}
fn number(v: &W) -> usize {
    v.as_number().expect("index") as usize
}
fn string(v: &W, k: &str) -> WireString {
    field(v, k).as_string().expect(k).clone()
}
fn s(v: &str) -> W {
    W::String(v.into())
}
fn n(v: usize) -> W {
    W::Number(v as f64)
}
fn path(v: &W) -> Vec<WireString> {
    array(v).iter().map(|v| v.as_string().unwrap().clone()).collect()
}

fn at_mut<'a>(mut value: &'a mut W, path: &[WireString]) -> &'a mut W {
    for key in path {
        value = match value {
            W::Object(entries) => {
                let index = entries.iter().position(|(k, _)| k == key).unwrap_or_else(|| {
                    entries.push((key.clone(), W::Null));
                    entries.len() - 1
                });
                &mut entries[index].1
            }
            W::Array(items) => &mut items[key.to_utf8().unwrap().parse::<usize>().unwrap()],
            _ => panic!("invalid corpus path"),
        };
    }
    value
}
fn decode(encoded: &W) -> VariantSpec {
    let mut value = match field(encoded, "rawWireJSON") {
        W::Null => W::Null,
        v => W::parse(&text(v)).expect("lossless original JSON"),
    };
    let undefined_paths: Vec<_> = array(field(encoded, "undefinedPaths")).iter().map(path).collect();
    for p in &undefined_paths {
        *at_mut(&mut value, p) = W::Null;
    }
    for item in array(field(encoded, "specialNumbers")) {
        let value_number = match text(field(item, "value")).as_str() {
            "NaN" => f64::NAN,
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            "-0" => -0.0,
            _ => panic!("unknown number"),
        };
        *at_mut(&mut value, &path(field(item, "path"))) = W::Number(value_number);
    }
    for item in array(field(encoded, "ownKeys")) {
        let W::Object(entries) = at_mut(&mut value, &path(field(item, "path"))) else { continue };
        let mut old = std::mem::take(entries);
        for key in array(field(item, "keys")) {
            let key = key.as_string().unwrap();
            let index = old.iter().position(|(k, _)| k == key).expect("restored own key");
            entries.push(old.remove(index));
        }
        assert!(old.is_empty(), "ownKeys must retain every object key");
    }
    VariantSpec { value, undefined_paths }
}
fn encode(spec: &VariantSpec) -> W {
    fn visit(
        v: &W,
        p: &mut Vec<WireString>,
        spec: &VariantSpec,
        undefined: &mut Vec<W>,
        special: &mut Vec<W>,
        own: &mut Vec<W>,
    ) {
        let encoded_path = || W::Array(p.iter().cloned().map(W::String).collect());
        if spec.undefined_paths.contains(p) {
            undefined.push(encoded_path());
            return;
        }
        if let W::Number(value) = v
            && (!value.is_finite() || (*value == 0.0 && value.is_sign_negative()))
        {
            let label = if value.is_nan() {
                "NaN"
            } else if *value == f64::INFINITY {
                "Infinity"
            } else if *value == f64::NEG_INFINITY {
                "-Infinity"
            } else {
                "-0"
            };
            special.push(W::object(vec![("path", encoded_path()), ("value", s(label))]));
            return;
        }
        let entries: Vec<(WireString, &W)> = match v {
            W::Object(_) => v.entries().unwrap().into_iter().map(|(k, v)| (k.clone(), v)).collect(),
            W::Array(items) => items.iter().enumerate().map(|(i, v)| (i.to_string().into(), v)).collect(),
            _ => return,
        };
        if entries.iter().any(|(key, _)| {
            let mut next = p.clone();
            next.push(key.clone());
            spec.undefined_paths.contains(&next)
        }) {
            own.push(W::object(vec![
                ("path", encoded_path()),
                ("keys", W::Array(entries.iter().map(|(k, _)| W::String(k.clone())).collect())),
            ]));
        }
        for (key, value) in entries {
            p.push(key);
            visit(value, p, spec, undefined, special, own);
            p.pop();
        }
    }
    let (mut undefined, mut special, mut own) = (Vec::new(), Vec::new(), Vec::new());
    visit(&spec.value, &mut Vec::new(), spec, &mut undefined, &mut special, &mut own);
    let raw = if spec.undefined_paths.contains(&Vec::new()) {
        W::Null
    } else {
        W::String(spec.to_wire_json().stringify().into())
    };
    W::object(vec![
        ("rawWireJSON", raw),
        ("undefinedPaths", W::Array(undefined)),
        ("specialNumbers", W::Array(special)),
        ("ownKeys", W::Array(own)),
    ])
}
fn specs_value(specs: &[SpecRef]) -> VariantSpec {
    VariantSpec {
        value: W::Array(specs.iter().map(|x| x.value.clone()).collect()),
        undefined_paths: specs
            .iter()
            .enumerate()
            .flat_map(|(i, x)| {
                x.undefined_paths.iter().map(move |p| {
                    let mut next = vec![WireString::from(i.to_string())];
                    next.extend(p.iter().cloned());
                    next
                })
            })
            .collect(),
    }
}
fn insert_record(out: &mut VariantSpec, key: &str, item: &VariantSpec) {
    out.set(key, item.value.clone());
    out.undefined_paths.extend(item.undefined_paths.iter().map(|p| {
        let mut next = vec![key.into()];
        next.extend(p.iter().cloned());
        next
    }));
}
fn templates(input: &W) -> Option<Vec<VariantFamilyTemplate>> {
    input.get("templates").map(|v| {
        array(v)
            .iter()
            .map(|t| {
                let pattern = field(t, "pattern");
                let source = WireString::from_units(
                    array(field(pattern, "sourceUnits")).iter().map(|n| n.as_number().unwrap() as u16).collect(),
                );
                let mut regexp =
                    JsRegExp::new(source, &text(field(pattern, "flags"))).expect("source validated table regex");
                regexp.set_last_index(field(pattern, "lastIndex").as_number().unwrap());
                VariantFamilyTemplate {
                    family: decode(field(t, "family")),
                    revision: t
                        .get("revision")
                        .map(|v| serde_json::from_str::<Vec<RevisionTerm>>(&v.stringify()).unwrap()),
                    pattern: Arc::new(Mutex::new(regexp)),
                }
            })
            .collect()
    })
}
fn table(input: &W) -> Arc<VariantCollapseTable> {
    Arc::new(VariantCollapseTable {
        families: array(field(input, "families")).iter().map(decode).collect(),
        templates: templates(input),
        provider_aliases: input.get("providerAliases").map(|v| decode(v).value),
    })
}
fn table_value(lookup: ReviewedCollapseLookup) -> VariantSpec {
    let mut out = VariantSpec::from_wire(W::Object(Vec::new()));
    match lookup {
        ReviewedCollapseLookup::None => out.set("kind", s("undefined")),
        ReviewedCollapseLookup::InheritedProperty(marker) => {
            out.set("kind", s("inherited"));
            out.set(
                "marker",
                s(match marker {
                    InheritedTableProperty::ObjectPrototype => "ObjectPrototype",
                    InheritedTableProperty::ObjectConstructor => "ObjectConstructor",
                    InheritedTableProperty::Function(name) => name,
                }),
            );
        }
        ReviewedCollapseLookup::Table(table) => {
            out.set("kind", s("table"));
            insert_record(
                &mut out,
                "families",
                &specs_value(&table.families.iter().cloned().map(Arc::new).collect::<Vec<_>>()),
            );
            if let Some(templates) = &table.templates {
                let mut values = Vec::new();
                for template in templates {
                    let mut value = VariantSpec::from_wire(W::Object(Vec::new()));
                    insert_record(&mut value, "family", &template.family);
                    if let Some(revision) = &template.revision {
                        value.set("revision", W::parse(&serde_json::to_string(revision).unwrap()).unwrap());
                    }
                    let regex = template.pattern.lock().unwrap();
                    value.set(
                        "pattern",
                        W::object(vec![
                            (
                                "sourceUnits",
                                W::Array(regex.source().units().iter().map(|n| W::Number(f64::from(*n))).collect()),
                            ),
                            ("flags", s(regex.flags())),
                            ("lastIndex", W::Number(regex.last_index())),
                        ]),
                    );
                    values.push(Arc::new(value));
                }
                insert_record(&mut out, "templates", &specs_value(&values));
            }
            if let Some(aliases) = &table.provider_aliases {
                out.set("providerAliases", aliases.clone());
            }
        }
    }
    out
}
enum Retained {
    Specs(Vec<SpecRef>),
    Raw(VariantSpec),
    Reverse(VariantAliasSources),
    Undefined,
}
impl Retained {
    fn encoded(&self) -> W {
        match self {
            Self::Specs(specs) => encode(&specs_value(specs)),
            Self::Raw(spec) => encode(spec),
            Self::Reverse(sources) => {
                encode(&VariantSpec::from_wire(W::Array(sources.snapshot().into_iter().map(W::String).collect())))
            }
            Self::Undefined => encode(&VariantSpec { value: W::Null, undefined_paths: vec![Vec::new()] }),
        }
    }
    fn items(&self) -> &[SpecRef] {
        if let Self::Specs(specs) = self { specs } else { &[] }
    }
    fn refs_len(&self) -> usize {
        match self {
            Self::Specs(specs) => specs.len(),
            Self::Reverse(sources) => sources.snapshot().len(),
            _ => 0,
        }
    }
}
fn invoke(
    runtime: &mut CollapseRuntime,
    step: &W,
    inputs: &[SpecRef],
    tables: &[Arc<VariantCollapseTable>],
    prior: &[Retained],
) -> Result<Retained, CollapseError> {
    let specs: Vec<_> = if let Some(from) = step.get("fromStep") {
        let mut out = prior[number(from)].items().to_vec();
        if let Some(indices) = step.get("appendInputIndices") {
            out.extend(array(indices).iter().map(|i| inputs[number(i)].clone()));
        }
        out
    } else {
        step.get("inputIndices").map_or_else(Vec::new, |v| array(v).iter().map(|i| inputs[number(i)].clone()).collect())
    };
    let table = step.get("tableIndex").map(|v| &tables[number(v)]);
    Ok(match text(field(step, "op")).as_str() {
        "reviewedCollapseTable" => {
            Retained::Raw(table_value(runtime.reviewed_collapse_table(&string(step, "provider"))))
        }
        "deriveThinkingPairFamilies" => Retained::Specs(
            runtime
                .derive_thinking_pair_families(&specs, table, step.get("provider").map(|v| v.as_string().unwrap()))?
                .into_iter()
                .map(Arc::new)
                .collect(),
        ),
        "isCollapsedVariantSpec" => {
            let spec = if let Some(from) = step.get("fromStep") {
                &prior[number(from)].items()[step.get("resultIndex").map_or(0, number)]
            } else {
                &inputs[number(field(step, "specIndex"))]
            };
            Retained::Raw(VariantSpec::from_wire(W::Bool(runtime.is_collapsed_variant_spec(spec)?)))
        }
        "collapseVariants" => Retained::Specs(runtime.collapse_variants(&specs, table)?),
        "collapseBuiltVariants" => Retained::Specs(runtime.collapse_built_variants(&specs)?),
        "resolveVariantSelector" => {
            match runtime.resolve_variant_selector(&string(step, "provider"), &string(step, "modelId"))? {
                Some(value) => Retained::Raw(VariantSpec::from_wire(W::String(value))),
                None => Retained::Undefined,
            }
        }
        "resolveBareVariantSelector" => match runtime.resolve_bare_variant_selector(&string(step, "modelId"))? {
            Some(value) => Retained::Raw(VariantSpec::from_wire(W::object(vec![
                ("id", W::String(value.id)),
                ("providers", W::Array(value.providers.into_iter().map(W::String).collect())),
            ]))),
            None => Retained::Undefined,
        },
        "getVariantAliasSources" => {
            Retained::Reverse(runtime.get_variant_alias_sources(&string(step, "provider"), &string(step, "modelId"))?)
        }
        _ => panic!("unknown original export"),
    })
}
fn replay(case: &W) -> W {
    let mut runtime = CollapseRuntime::new().expect("fixed tables");
    let inputs: Vec<_> = array(field(case, "inputs")).iter().map(|v| Arc::new(decode(v))).collect();
    let tables: Vec<_> = array(field(case, "tables")).iter().map(table).collect();
    let (mut prior, mut expected, mut held) = (Vec::<Retained>::new(), Vec::new(), Vec::new());
    for step in array(field(case, "steps")) {
        let mut out = W::Object(Vec::new());
        let result = invoke(&mut runtime, step, &inputs, &tables, &prior);
        let current = match result {
            Ok(value) => {
                out.insert("status", s("ok"));
                out.insert("value", value.encoded());
                let mut input_refs = Vec::new();
                let mut prior_refs = Vec::new();
                for index in 0..value.refs_len() {
                    let item = value.items().get(index);
                    input_refs.push(W::Number(
                        item.and_then(|v| inputs.iter().position(|x| Arc::ptr_eq(v, x))).map_or(-1.0, |i| i as f64),
                    ));
                    let found = item.and_then(|item| {
                        prior.iter().enumerate().find_map(|(step, v)| {
                            v.items()
                                .iter()
                                .position(|x| Arc::ptr_eq(x, item))
                                .map(|index| W::object(vec![("step", n(step)), ("index", n(index))]))
                        })
                    });
                    prior_refs.push(found.unwrap_or(W::Null));
                }
                out.insert("inputRefs", W::Array(input_refs));
                out.insert("priorRefs", W::Array(prior_refs));
                if let Retained::Reverse(sources) = &value {
                    held.push((prior.len(), sources.clone()));
                }
                value
            }
            Err(error) => {
                out.insert("status", s("error"));
                out.insert("name", s(error.name()));
                out.insert("message", W::String(error.message_wire()));
                Retained::Undefined
            }
        };
        out.insert(
            "heldReverse",
            W::Array(
                held.iter()
                    .map(|(step, sources)| {
                        let same = matches!(&current,Retained::Reverse(now) if now.shares_storage(sources));
                        W::object(vec![
                            ("step", n(*step)),
                            ("value", Retained::Reverse(sources.clone()).encoded()),
                            ("sameAsCurrent", W::Bool(same)),
                        ])
                    })
                    .collect(),
            ),
        );
        out.insert(
            "tableStates",
            W::Array(
                tables
                    .iter()
                    .map(|table| {
                        W::Array(table.templates.as_ref().map_or_else(Vec::new, |templates| {
                            templates
                                .iter()
                                .map(|template| W::Number(template.pattern.lock().unwrap().last_index()))
                                .collect()
                        }))
                    })
                    .collect(),
            ),
        );
        expected.push(out);
        prior.push(current);
    }
    W::Array(expected)
}
fn inventory(oracle: &W) -> Result<(), &'static str> {
    if oracle.get("schemaVersion") != Some(&n(1))
        || oracle.get("upstreamCommit") != Some(&s("596f2da7101178214aa27a753529d15e6b7ad91d"))
        || oracle.get("bunVersion") != Some(&s("1.4.0"))
        || oracle.get("catalogRowCount") != Some(&n(4776))
        || oracle.get("reviewedFamilyCount") != Some(&n(67))
        || oracle.get("reviewedTemplateCount") != Some(&n(2))
    {
        return Err("wrong source/inventory");
    }
    let cases = oracle.get("cases").and_then(W::as_array).ok_or("missing cases")?;
    let mut counts = BTreeMap::new();
    let mut exports = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for case in cases {
        let id = case.get("id").and_then(W::as_string).ok_or("missing id")?;
        if !ids.insert(id.to_utf8().map_err(|_| "bad id")?) {
            return Err("duplicate case");
        }
        *counts.entry(text(field(case, "family"))).or_insert(0usize) += 1;
        for step in array(field(case, "steps")) {
            *exports.entry(text(field(step, "op"))).or_insert(0usize) += 1;
        }
    }
    let expected = |items: &[(&str, usize)]| items.iter().map(|(k, v)| (k.to_string(), *v)).collect::<BTreeMap<_, _>>();
    if counts != expected(COUNTS) || exports != expected(EXPORTS) || cases.len() != 1995 {
        return Err("incomplete or replaced corpus");
    }
    Ok(())
}
fn load() -> W {
    let path = std::env::var_os("ARA_MODEL_COLLAPSE_ORACLE").expect("generate scripts/model_collapse_oracle.py first");
    verified(&std::fs::read(path).unwrap()).expect("complete frozen original corpus")
}
fn verified(bytes: &[u8]) -> Result<W, &'static str> {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    let actual = digest.as_ref().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    if actual != "23289a1a8fd647facbd375a756e3eeea6ecf9f1592bd0b336b152bef48dc8ba7" {
        return Err("replaced or incomplete source corpus");
    }
    W::parse(std::str::from_utf8(bytes).map_err(|_| "invalid corpus UTF-8")?).map_err(|_| "invalid wire corpus")
}
#[test]
#[ignore = "requires retained fixed OMP artifact in ARA_MODEL_COLLAPSE_ORACLE"]
fn native_collapse_compares_with_fixed_omp() {
    let oracle = load();
    inventory(&oracle).expect("complete mandatory corpus");
    let mut differences = Vec::new();
    for case in array(field(&oracle, "cases")) {
        let actual = replay(case);
        if !actual.deep_equal(field(case, "expected")) {
            differences.push(W::object(vec![
                ("id", field(case, "id").clone()),
                ("actual", actual),
                ("expected", field(case, "expected").clone()),
            ]));
        }
    }
    if let Some(path) = std::env::var_os("ARA_MODEL_COLLAPSE_MISMATCHES") {
        std::fs::write(path, W::Array(differences.clone()).stringify()).unwrap();
    }
    let ids = W::Array(differences.iter().map(|c| field(c, "id").clone()).collect());
    assert!(differences.is_empty(), "{} native collapse differences: {}", differences.len(), ids.stringify());
}
#[test]
#[ignore = "requires retained fixed OMP artifact in ARA_MODEL_COLLAPSE_ORACLE"]
fn malformed_collapse_corpus_is_rejected() {
    let mut oracle = load();
    inventory(&oracle).unwrap();
    assert!(verified(oracle.stringify().as_bytes()).is_err(), "re-encoded/replaced corpus is rejected");
    let W::Array(cases) = at_mut(&mut oracle, &["cases".into()]) else { panic!("cases") };
    let id = field(&cases[0], "id").clone();
    let duplicate = field(&cases[1], "id").clone();
    cases[0].insert("id", duplicate);
    assert!(inventory(&oracle).is_err());
    let W::Array(cases) = at_mut(&mut oracle, &["cases".into()]) else { panic!("cases") };
    cases[0].insert("id", id);
    cases.pop();
    assert!(inventory(&oracle).is_err());
}

#[test]
#[ignore = "requires retained fixed OMP artifact in ARA_MODEL_COLLAPSE_ORACLE"]
fn native_policy_preserves_lossless_overrides_and_original_errors() {
    use ara_cli::{model_collapse::CollapseModelPolicy, model_wire_policy::WireModelPolicy};
    let oracle = load();
    inventory(&oracle).expect("complete original collapse corpus");
    let cases = array(field(&oracle, "losslessPolicyCases"));
    assert_eq!(cases.len(), 26, "seven lossless records and six ambiguous IDs through both exports");
    let mut ids = BTreeSet::new();
    let mut differences = Vec::new();
    for case in cases {
        assert!(ids.insert(text(field(case, "id"))), "duplicate lossless case");
        let input = decode(field(case, "input"));
        let outcome = match text(field(case, "op")).as_str() {
            "resolveModelPolicy" => WireModelPolicy.resolve(&input),
            "buildModel" => WireModelPolicy.build(&input),
            _ => panic!("unknown policy export"),
        };
        let actual = match outcome {
            Ok(spec) => W::object(vec![("status", s("ok")), ("value", encode(&spec))]),
            Err(error) => W::object(vec![
                ("status", s("error")),
                ("name", s(error.name())),
                ("message", W::String(error.message_wire())),
            ]),
        };
        if !actual.deep_equal(field(case, "expected")) {
            differences.push(W::object(vec![
                ("id", field(case, "id").clone()),
                ("actual", actual),
                ("expected", field(case, "expected").clone()),
            ]));
        }
    }
    if let Some(path) = std::env::var_os("ARA_LOSSLESS_POLICY_MISMATCHES") {
        std::fs::write(path, W::Array(differences.clone()).stringify()).unwrap();
    }
    let ids = W::Array(differences.iter().map(|c| field(c, "id").clone()).collect());
    assert!(differences.is_empty(), "{} native lossless policy differences: {}", differences.len(), ids.stringify());
}
