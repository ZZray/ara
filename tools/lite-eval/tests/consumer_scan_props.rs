// Feature: ara-lite-core-capability, Property 41: 消费者引用扫描
//! For any synthetic consumer source referencing stable, experimental,
//! internal or unlisted Lite items in the forms the scanner reads (use trees,
//! aliases, qualified paths, trait impl methods, module globs, field access),
//! with look-alike text in comments and strings, the scan reports exactly
//! the references that are not stable, each with its name and location, and
//! the check fails iff there is at least one.

use std::collections::BTreeSet;

use lite_eval::gate::consumer::{scan_source, Class, StabilityTable};
use proptest::prelude::*;

const DOC: &str = "\
| 项 | 级别 | 说明 |
|---|---|---|
| `ara_agent_lite_core::Good0` | stable | s |
| `ara_agent_lite_core::Good1` | stable | s |
| `ara_agent_lite_core::Exp0` | experimental | e |
| `ara_agent_lite_core::Exp1` | experimental | e |
| `ara_agent_lite_core::Int0` | internal | i |
| `ara_agent_lite_core::Int1` | internal | i |
| `ara_agent_lite_core::Host` | stable | 必需方法 `good`；`bad` 为 experimental |
| `ara_agent_lite_core::exp_mod::*` | experimental | m |
| `ara_agent_lite_core::Req.open` | stable | f |
| `ara_agent_lite_core::Req.lease` | experimental | f |
";

const CORE: &str = "ara_agent_lite_core";

#[derive(Clone, Copy, Debug)]
enum Item {
    Good(u8),
    Exp(u8),
    Int(u8),
    Unlisted(u8),
}

impl Item {
    fn name(self) -> String {
        match self {
            Item::Good(n) => format!("Good{n}"),
            Item::Exp(n) => format!("Exp{n}"),
            Item::Int(n) => format!("Int{n}"),
            Item::Unlisted(n) => format!("Missing{n}"),
        }
    }

    fn class(self) -> Class {
        match self {
            Item::Good(_) => Class::Stable,
            Item::Exp(_) => Class::Experimental,
            Item::Int(_) => Class::Internal,
            Item::Unlisted(_) => Class::Unlisted,
        }
    }
}

fn item() -> impl Strategy<Value = Item> {
    (0u8..4, 0u8..2).prop_map(|(kind, n)| match kind {
        0 => Item::Good(n),
        1 => Item::Exp(n),
        2 => Item::Int(n),
        _ => Item::Unlisted(n),
    })
}

#[derive(Clone, Debug)]
enum Form {
    Use(Item),
    Nested(Item, Item),
    Alias(Item),
    Inline(Item),
    Impl(bool),
    Glob,
    Field(bool),
    Comment(Item),
    Str(Item),
}

fn form() -> impl Strategy<Value = Form> {
    prop_oneof![
        item().prop_map(Form::Use),
        (item(), item()).prop_map(|(a, b)| Form::Nested(a, b)),
        item().prop_map(Form::Alias),
        item().prop_map(Form::Inline),
        any::<bool>().prop_map(Form::Impl),
        Just(Form::Glob),
        any::<bool>().prop_map(Form::Field),
        item().prop_map(Form::Comment),
        item().prop_map(Form::Str),
    ]
}

/// One source line and the references on it that must be reported.
fn render(form: &Form, line: usize) -> (String, Vec<(String, Class)>) {
    let full = |item: Item| format!("{CORE}::{}", item.name());
    let report = |pairs: Vec<(String, Class)>| pairs.into_iter().filter(|(_, class)| *class != Class::Stable).collect();
    match form {
        Form::Use(item) => (format!("use {}::{};", CORE, item.name()), report(vec![(full(*item), item.class())])),
        Form::Nested(a, b) => (
            format!("use {CORE}::{{{}, {} as Alias{line}}};", a.name(), b.name()),
            report(vec![(full(*a), a.class()), (full(*b), b.class())]),
        ),
        Form::Alias(item) => (
            format!("use {CORE}::{} as A{line}; type B{line} = A{line}::Variant;", item.name()),
            report(vec![(full(*item), item.class()), (format!("{}::Variant", full(*item)), item.class())]),
        ),
        Form::Inline(item) => (
            format!("fn f{line}() {{ let _v = {}::Variant; }}", full(*item)),
            report(vec![(format!("{}::Variant", full(*item)), item.class())]),
        ),
        Form::Impl(good) => {
            let method = if *good { "good" } else { "bad" };
            let class = if *good { Class::Stable } else { Class::Experimental };
            (
                format!("impl {CORE}::Host for L{line} {{ fn {method}(&self) {{}} }}"),
                report(vec![(format!("{CORE}::Host::{method}"), class)]),
            )
        }
        Form::Glob => (
            format!("type T{line} = {CORE}::exp_mod::Thing;"),
            vec![(format!("{CORE}::exp_mod::Thing"), Class::Experimental)],
        ),
        Form::Field(open) => {
            let field = if *open { "open" } else { "lease" };
            let class = if *open { Class::Stable } else { Class::Experimental };
            (
                format!("fn g{line}(r: Req) {{ let _ = r.{field}; }}"),
                report(vec![(format!("{CORE}::Req.{field}"), class)]),
            )
        }
        Form::Comment(item) => (format!("// {} is only named here", full(*item)), Vec::new()),
        Form::Str(item) => (format!("const S{line}: &str = \"{}\";", full(*item)), Vec::new()),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(128) })]

    #[test]
    fn scan_reports_exactly_the_non_stable_references(forms in prop::collection::vec(form(), 0..12)) {
        let table = StabilityTable::parse(DOC).unwrap();
        // `Req` is stable through its stable field row; importing it is allowed.
        let mut lines = vec![format!("use {CORE}::Req;")];
        let mut expected = BTreeSet::new();
        for form in &forms {
            let line = lines.len() + 1;
            let (text, reported) = render(form, line);
            lines.push(text);
            expected.extend(reported.into_iter().map(|(name, class)| (name, line, class)));
        }
        let source = lines.join("\n");
        let findings = scan_source("src/lib.rs", &source, &table);

        let mut actual = BTreeSet::new();
        for finding in &findings {
            let mut parts = finding.location.rsplitn(3, ':');
            let column: usize = parts.next().unwrap().parse().unwrap();
            let line: usize = parts.next().unwrap().parse().unwrap();
            prop_assert_eq!(parts.next(), Some("src/lib.rs"));
            prop_assert!(column >= 1);
            prop_assert!(finding.class != Class::Stable);
            actual.insert((finding.name.clone(), line, finding.class));
        }
        prop_assert_eq!(&actual, &expected, "source:\n{}", source);
        prop_assert_eq!(findings.is_empty(), expected.is_empty());
    }
}
