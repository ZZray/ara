//! RegExp operations consumed by fixed OMP validation.ts (596f2da7101178214aa27a753529d15e6b7ad91d).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY.md.
//! This selected native surface accepts empty/g/i/s/u flags. Native @@split
//! constructs its default splitter with internal sticky state; general RegExp
//! construction/realms, custom species Construct and v remain unsupported.
//! Arena edges contain NodeIds; compile, matching and Host calls run unlocked.

use super::string_methods::{find_units, integer_or_infinity};
use super::*;

#[derive(Debug, Clone)]
pub(crate) struct RegExpData {
    pattern: WireString,
    compiled: Arc<regress::Regex>,
    global: bool,
    ignore_case: bool,
    dot_all: bool,
    unicode: bool,
    sticky: bool,
}

struct SubstitutionContext<'a> {
    matched: &'a WireString,
    text: &'a WireString,
    position: usize,
    captures: &'a [Option<WireString>],
    named: Option<&'a ValueHandle>,
}

fn compile_regexp(pattern: WireString, flags: &str) -> ValueResult<RegExpData> {
    compile_regexp_flags(pattern, flags, false)
}

// Public construction keeps its selected g/i/s/u domain. The y flag is generated
// by the real default @@split construction, rather than stripped from flags.
fn compile_regexp_flags(pattern: WireString, flags: &str, allow_sticky: bool) -> ValueResult<RegExpData> {
    let mut seen = HashSet::new();
    for flag in flags.chars() {
        if !matches!(flag, 'd' | 'g' | 'i' | 'm' | 's' | 'u' | 'v' | 'y') || !seen.insert(flag) {
            return Err(ValueError::SyntaxError("Invalid or duplicate RegExp flag".into()));
        }
    }
    if seen.contains(&'u') && seen.contains(&'v') {
        return Err(ValueError::SyntaxError("RegExp u and v flags are mutually exclusive".into()));
    }
    if seen.iter().any(|flag| !matches!(*flag, 'g' | 'i' | 's' | 'u') && !(allow_sticky && *flag == 'y')) {
        return Err(ValueError::UnsupportedOperation(
            "RegExp flags outside g/i/s/u (and internal split y), including v, are not connected",
        ));
    }
    let unicode = seen.contains(&'u');
    // Decode only paired raw UTF16 surrogates in u mode. Lone surrogates remain
    // their original code points; non-u patterns keep code-unit semantics.
    let mut units = pattern.units().iter().copied().peekable();
    let points = std::iter::from_fn(move || {
        let first = units.next()?;
        if unicode && (0xd800..=0xdbff).contains(&first) {
            if let Some(second) = units.next_if(|unit| (0xdc00..=0xdfff).contains(unit)) {
                return Some(0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00);
            }
        }
        Some(u32::from(first))
    });
    let compiled = regress::Regex::from_unicode(
        points,
        regress::Flags { icase: seen.contains(&'i'), dot_all: seen.contains(&'s'), unicode, ..Default::default() },
    )
    .map_err(|error| ValueError::SyntaxError(error.to_string()))?;
    Ok(RegExpData {
        pattern,
        compiled: Arc::new(compiled),
        global: seen.contains(&'g'),
        ignore_case: seen.contains(&'i'),
        dot_all: seen.contains(&'s'),
        unicode,
        sticky: seen.contains(&'y'),
    })
}

pub(super) fn bootstrap_regexp(
    arena: &mut Arena,
    token: u64,
    object_proto: NodeId,
    function_proto: NodeId,
    symbol_split: NodeId,
) -> ValueResult<(NodeId, NodeId, NodeId)> {
    let prototype = arena.allocate(Node::Object(ObjectData::new(Some(object_proto))))?;
    let symbol_match = arena.allocate(Node::Symbol { description: Some("Symbol.match".into()) })?;
    let symbol_replace = arena.allocate(Node::Symbol { description: Some("Symbol.replace".into()) })?;
    fn add(arena: &mut Arena, owner: NodeId, key: PropertyKey, descriptor: StoredDescriptor) -> ValueResult<()> {
        let object = arena.get_mut(owner)?.object_mut().ok_or(ValueError::InvalidNode)?;
        object.properties.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        object.properties.push(PropertyEntry { key, descriptor });
        Ok(())
    }
    for (key, name, length, intrinsic) in [
        (PropertyKey::from("exec"), "exec", 1.0, Intrinsic::RegExpExec),
        (PropertyKey::from("test"), "test", 1.0, Intrinsic::RegExpTest),
        (PropertyKey::from("toString"), "toString", 0.0, Intrinsic::RegExpToString),
        (
            PropertyKey::Symbol(SymbolKey { heap_token: token, node: symbol_match }),
            "[Symbol.match]",
            1.0,
            Intrinsic::RegExpMatch,
        ),
        (
            PropertyKey::Symbol(SymbolKey { heap_token: token, node: symbol_replace }),
            "[Symbol.replace]",
            2.0,
            Intrinsic::RegExpReplace,
        ),
        (
            PropertyKey::Symbol(SymbolKey { heap_token: token, node: symbol_split }),
            "[Symbol.split]",
            2.0,
            Intrinsic::RegExpSplit,
        ),
    ] {
        let function = arena.allocate(Node::Function {
            object: ObjectData::new(Some(function_proto)),
            target: CallableTarget::Intrinsic(intrinsic),
        })?;
        let name = arena.allocate(Node::String(name.into()))?;
        let length = arena.allocate(Node::Number(length))?;
        for (key, value) in [("name", name), ("length", length)] {
            add(
                arena,
                function,
                key.into(),
                StoredDescriptor::Data { value, writable: false, enumerable: false, configurable: true },
            )?;
        }
        add(
            arena,
            prototype,
            key,
            StoredDescriptor::Data { value: function, writable: true, enumerable: false, configurable: true },
        )?;
    }
    for (name, intrinsic) in [
        ("global", Intrinsic::RegExpGlobalGet),
        ("unicode", Intrinsic::RegExpUnicodeGet),
        ("hasIndices", Intrinsic::RegExpFalseFlagGet),
        ("ignoreCase", Intrinsic::RegExpIgnoreCaseGet),
        ("multiline", Intrinsic::RegExpFalseFlagGet),
        ("dotAll", Intrinsic::RegExpDotAllGet),
        ("unicodeSets", Intrinsic::RegExpFalseFlagGet),
        ("sticky", Intrinsic::RegExpStickyGet),
        ("source", Intrinsic::RegExpSourceGet),
        ("flags", Intrinsic::RegExpFlagsGet),
    ] {
        add(
            arena,
            prototype,
            name.into(),
            StoredDescriptor::Accessor {
                get: Some(CallableTarget::Intrinsic(intrinsic)),
                set: None,
                enumerable: false,
                configurable: true,
            },
        )?;
    }
    Ok((prototype, symbol_match, symbol_replace))
}

fn regexp_node(data: RegExpData, prototype: NodeId, last_index: NodeId) -> Node {
    let mut object = ObjectData::new(Some(prototype));
    object.properties.push(PropertyEntry {
        key: "lastIndex".into(),
        descriptor: StoredDescriptor::Data {
            value: last_index,
            writable: true,
            enumerable: false,
            configurable: false,
        },
    });
    Node::RegExp { object, data }
}

impl ValueHeap {
    pub fn match_key(&self) -> PropertyKey {
        PropertyKey::Symbol(SymbolKey { heap_token: self.token, node: self.intrinsics.symbol_match })
    }
    pub fn replace_key(&self) -> PropertyKey {
        PropertyKey::Symbol(SymbolKey { heap_token: self.token, node: self.intrinsics.symbol_replace })
    }
    pub fn regexp(self: &Arc<Self>, pattern: WireString, flags: &str) -> ValueResult<ValueHandle> {
        let data = compile_regexp(pattern, flags)?;
        let mut arena = self.lock()?;
        arena.nodes.try_reserve(2).map_err(|_| ValueError::Allocation)?;
        let zero = arena.allocate(Node::Number(0.0))?;
        let node = arena.allocate(regexp_node(data, self.intrinsics.regexp_proto, zero))?;
        Ok(self.handle(node))
    }
    /// Native structuredClone uses internal slots, never source/flags getters.
    /// Own expando properties and lastIndex are not serialized for RegExp.
    pub(super) fn clone_regexp_data(self: &Arc<Self>, data: RegExpData) -> ValueResult<ValueHandle> {
        let mut arena = self.lock()?;
        arena.nodes.try_reserve(2).map_err(|_| ValueError::Allocation)?;
        let zero = arena.allocate(Node::Number(0.0))?;
        let node = arena.allocate(regexp_node(data, self.intrinsics.regexp_proto, zero))?;
        Ok(self.handle(node))
    }
    /// One module identity per exact static definition. Concurrent cold callers
    /// may compile independently, but only the winning NodeId is published.
    pub fn module_regexp(
        self: &Arc<Self>,
        module: &'static str,
        name: &'static str,
        pattern: &'static str,
        flags: &'static str,
    ) -> ValueResult<ValueHandle> {
        let key = (module, name);
        let definition = StaticModuleValue::RegExp { pattern, flags };
        {
            let arena = self.lock()?;
            if let Some((node, original)) = arena.module_values.get(&key) {
                return if *original == definition {
                    Ok(self.handle(*node))
                } else {
                    Err(ValueError::SourceIdentityCollision)
                };
            }
        }
        let data = compile_regexp(pattern.into(), flags)?;
        let mut arena = self.lock()?;
        if let Some((node, original)) = arena.module_values.get(&key) {
            return if *original == definition {
                Ok(self.handle(*node))
            } else {
                Err(ValueError::SourceIdentityCollision)
            };
        }
        arena.module_values.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        arena.nodes.try_reserve(2).map_err(|_| ValueError::Allocation)?;
        let zero = arena.allocate(Node::Number(0.0))?;
        let node = arena.allocate(regexp_node(data, self.intrinsics.regexp_proto, zero))?;
        arena.module_values.insert(key, (node, definition));
        Ok(self.handle(node))
    }
    pub(super) fn regexp_data(&self, value: &ValueHandle) -> ValueResult<RegExpData> {
        let node = self.admit(value)?;
        match self.lock()?.get(node)? {
            Node::RegExp { data, .. } => Ok(data.clone()),
            _ => Err(ValueError::TypeError("RegExp method receiver has no RegExp internal slots".into())),
        }
    }
    fn require_regexp_object(&self, value: &ValueHandle) -> ValueResult<()> {
        match self.kind(value)? {
            ValueKind::Object | ValueKind::Array | ValueKind::Function => Ok(()),
            _ => Err(ValueError::TypeError("RegExp method receiver is not an object".into())),
        }
    }
    pub fn is_regexp(self: &Arc<Self>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<bool> {
        if !matches!(self.kind(value)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            return Ok(false);
        }
        let matcher = self.get(value, &self.match_key(), runtime)?;
        if self.kind(&matcher)? != ValueKind::Undefined {
            return self.truthy(&matcher);
        }
        let node = self.admit(value)?;
        Ok(matches!(self.lock()?.get(node)?, Node::RegExp { .. }))
    }
    fn regexp_to_length(self: &Arc<Self>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<u64> {
        let number = self.to_number(value, runtime)?;
        Ok(if number.is_nan() || number <= 0.0 { 0 } else { number.floor().min(9_007_199_254_740_991.0) as u64 })
    }
    fn regexp_exec_result(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        text: &WireString,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        let exec = self.get(receiver, &"exec".into(), runtime)?;
        if self.kind(&exec)? == ValueKind::Function {
            let text = self.string(text.clone())?;
            let result = self.call(&exec, receiver, &[text], runtime)?;
            return if matches!(
                self.kind(&result)?,
                ValueKind::Null | ValueKind::Object | ValueKind::Array | ValueKind::Function
            ) {
                Ok(result)
            } else {
                Err(ValueError::TypeError("RegExp exec must return null or an object".into()))
            };
        }
        self.regexp_builtin_exec(receiver, text, runtime)
    }
    fn regexp_builtin_exec(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        text: &WireString,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        let data = self.regexp_data(receiver)?;
        let last_index = self.get(receiver, &"lastIndex".into(), runtime)?;
        let last_index = self.regexp_to_length(&last_index, runtime)?;
        let stateful = data.global || data.sticky;
        let start = if stateful { last_index } else { 0 };
        // Both engines return UTF16 code-unit indices. Only u consumes paired
        // surrogates together; neither replaces lone surrogates with U+FFFD.
        let found = if start > text.len() as u64 {
            None
        } else if data.sticky {
            // Full-input at-offset matching attempts only lastIndex. A suffix
            // search followed by a start filter would make split quadratic.
            if data.unicode {
                data.compiled.match_at_utf16(text.units(), start as usize)
            } else {
                data.compiled.match_at_ucs2(text.units(), start as usize)
            }
        } else if data.unicode {
            data.compiled.find_from_utf16(text.units(), start as usize).next()
        } else {
            data.compiled.find_from_ucs2(text.units(), start as usize).next()
        };
        let Some(found) = found else {
            if stateful {
                self.assign(receiver, "lastIndex".into(), self.number(0.0)?, runtime)?;
            }
            return self.null();
        };
        if stateful {
            self.assign(receiver, "lastIndex".into(), self.number(found.range.end as f64)?, runtime)?;
        }
        let mut values = Vec::new();
        values.try_reserve(found.captures.len() + 1).map_err(|_| ValueError::Allocation)?;
        values.push(self.string(WireString::from_units(text.units()[found.range.clone()].to_vec()))?);
        for capture in &found.captures {
            values.push(match capture {
                Some(range) => self.string(WireString::from_units(text.units()[range.clone()].to_vec()))?,
                None => self.undefined()?,
            });
        }
        let result = self.array(values)?;
        self.define_own_data(&result, "index".into(), self.number(found.range.start as f64)?, runtime)?;
        self.define_own_data(&result, "input".into(), self.string(text.clone())?, runtime)?;
        let named: Vec<_> = found.named_groups().collect();
        let groups = if named.is_empty() {
            self.undefined()?
        } else {
            let groups = self.object_with_prototype(None)?;
            for (name, capture) in named {
                let value = match capture {
                    Some(range) => self.string(WireString::from_units(text.units()[range].to_vec()))?,
                    None => self.undefined()?,
                };
                self.define_own_data(&groups, name.into(), value, runtime)?;
            }
            groups
        };
        self.define_own_data(&result, "groups".into(), groups, runtime)?;
        Ok(result)
    }
    pub(super) fn regexp_method(
        self: &Arc<Self>,
        method: Intrinsic,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        match method {
            Intrinsic::RegExpExec => {
                self.regexp_data(receiver)?; // Brand check precedes input coercion.
                let input = arguments.first().cloned().unwrap_or(self.undefined()?);
                let text = self.implicit_text(&input, runtime)?;
                self.regexp_builtin_exec(receiver, &text, runtime)
            }
            Intrinsic::RegExpTest => {
                self.require_regexp_object(receiver)?;
                let input = arguments.first().cloned().unwrap_or(self.undefined()?);
                let text = self.implicit_text(&input, runtime)?;
                let result = self.regexp_exec_result(receiver, &text, runtime)?;
                self.boolean(self.kind(&result)? != ValueKind::Null)
            }
            Intrinsic::RegExpToString => {
                self.require_regexp_object(receiver)?;
                let source = self.get(receiver, &"source".into(), runtime)?;
                let source = self.implicit_text(&source, runtime)?;
                let flags = self.get(receiver, &"flags".into(), runtime)?;
                let flags = self.implicit_text(&flags, runtime)?;
                let mut units = vec![0x2f];
                units.extend_from_slice(source.units());
                units.push(0x2f);
                units.extend_from_slice(flags.units());
                self.string(WireString::from_units(units))
            }
            Intrinsic::RegExpGlobalGet
            | Intrinsic::RegExpIgnoreCaseGet
            | Intrinsic::RegExpDotAllGet
            | Intrinsic::RegExpStickyGet
            | Intrinsic::RegExpUnicodeGet
            | Intrinsic::RegExpFalseFlagGet => {
                if self.admit(receiver)? == self.intrinsics.regexp_proto {
                    return self.undefined();
                }
                let data = self.regexp_data(receiver)?;
                self.boolean(
                    (method == Intrinsic::RegExpGlobalGet && data.global)
                        || (method == Intrinsic::RegExpIgnoreCaseGet && data.ignore_case)
                        || (method == Intrinsic::RegExpDotAllGet && data.dot_all)
                        || (method == Intrinsic::RegExpStickyGet && data.sticky)
                        || (method == Intrinsic::RegExpUnicodeGet && data.unicode),
                )
            }
            Intrinsic::RegExpSourceGet => {
                let pattern = if self.admit(receiver)? == self.intrinsics.regexp_proto {
                    WireString::from("")
                } else {
                    self.regexp_data(receiver)?.pattern
                };
                self.string(escaped_source(&pattern))
            }
            Intrinsic::RegExpFlagsGet => {
                self.require_regexp_object(receiver)?;
                let mut flags = String::new();
                for (name, flag) in [
                    ("hasIndices", 'd'),
                    ("global", 'g'),
                    ("ignoreCase", 'i'),
                    ("multiline", 'm'),
                    ("dotAll", 's'),
                    ("unicode", 'u'),
                    ("unicodeSets", 'v'),
                    ("sticky", 'y'),
                ] {
                    let value = self.get(receiver, &name.into(), runtime)?;
                    if self.truthy(&value)? {
                        flags.push(flag);
                    }
                }
                self.string(flags.into())
            }
            Intrinsic::RegExpReplace => self.regexp_replace(receiver, arguments, runtime),
            Intrinsic::RegExpSplit => self.regexp_split(receiver, arguments, runtime),
            Intrinsic::RegExpMatch => Err(ValueError::UnsupportedOperation(
                "Generic RegExp Symbol.match is outside the fixed validation consumers",
            )),
            _ => Err(ValueError::InvalidNode),
        }
    }
    /// Default RegExp @@split used by fixed grammar.ts. The runtime has no
    /// generic Construct request: custom species, general constructor/realm
    /// behavior and v are explicit gaps, never approximated with Call.
    fn regexp_split(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.require_regexp_object(receiver)?;
        let input = arguments.first().cloned().unwrap_or(self.undefined()?);
        let text = self.implicit_text(&input, runtime)?;

        // SpeciesConstructor runs before flags or limit conversion. Preserve
        // these original Gets and Throw receipts even for excluded Construct.
        let constructor = self.get(receiver, &"constructor".into(), runtime)?;
        let species = match self.kind(&constructor)? {
            ValueKind::Undefined => None,
            ValueKind::Object | ValueKind::Array | ValueKind::Function => {
                let species = self.get(&constructor, &self.species_key(), runtime)?;
                match self.kind(&species)? {
                    ValueKind::Null | ValueKind::Undefined => None,
                    ValueKind::Function => Some(species),
                    _ => return Err(ValueError::TypeError("RegExp split species is not a constructor".into())),
                }
            }
            _ => return Err(ValueError::TypeError("RegExp split constructor is not an object".into())),
        };
        let flags = self.get(receiver, &"flags".into(), runtime)?;
        let flags = self.implicit_text(&flags, runtime)?;
        let unicode = flags.units().contains(&u16::from(b'u'));
        let mut new_flags = flags.units().to_vec();
        if !new_flags.contains(&u16::from(b'y')) {
            new_flags.push(u16::from(b'y'));
        }
        if species.is_some() {
            return Err(ValueError::UnsupportedOperation("Custom RegExp split species Construct is not connected"));
        }

        // Default Construct(%RegExp%, [receiver, newFlags]) still observes
        // IsRegExp. Genuine internal slots provide the pattern even when an
        // own @@match returns false; generic receivers use source/ToString.
        let is_regexp = self.is_regexp(receiver, runtime)?;
        let native_pattern = {
            let node = self.admit(receiver)?;
            let arena = self.lock()?;
            match arena.get(node)? {
                Node::RegExp { data, .. } => Some(data.pattern.clone()),
                _ => None,
            }
        };
        let pattern = match native_pattern {
            Some(pattern) => pattern,
            None if is_regexp => {
                let source = self.get(receiver, &"source".into(), runtime)?;
                self.implicit_text(&source, runtime)?
            }
            None => self.implicit_text(receiver, runtime)?,
        };
        let new_flags = WireString::from_units(new_flags)
            .to_utf8()
            .map_err(|_| ValueError::SyntaxError("Invalid RegExp flag".into()))?;
        let data = compile_regexp_flags(pattern, &new_flags, true)?;
        // A separate native splitter, with the current RegExp prototype, zero
        // lastIndex and no copied expando/own exec. Prototype exec stays live.
        let splitter = self.clone_regexp_data(data)?;
        let result = self.array_with_length(0)?;
        let limit = match arguments.get(1) {
            Some(limit) if self.kind(limit)? != ValueKind::Undefined => {
                let number = self.to_number(limit, runtime)?;
                if !number.is_finite() || number == 0.0 { 0 } else { number.trunc().rem_euclid(4_294_967_296.0) as u32 }
            }
            _ => u32::MAX,
        };
        if limit == 0 {
            return Ok(result);
        }
        if text.is_empty() {
            let matched = self.regexp_exec_result(&splitter, &text, runtime)?;
            if self.kind(&matched)? == ValueKind::Null {
                self.define_own_data(&result, "0".into(), self.string(text)?, runtime)?;
            }
            return Ok(result);
        }
        let size = text.len() as u64;
        let mut p = 0u64;
        let mut q = 0u64;
        let mut count = 0u32;
        while q < size {
            self.assign(&splitter, "lastIndex".into(), self.number(q as f64)?, runtime)?;
            let matched = self.regexp_exec_result(&splitter, &text, runtime)?;
            if self.kind(&matched)? == ValueKind::Null {
                q = advance_string_index(text.units(), q, unicode);
                continue;
            }
            let last_index = self.get(&splitter, &"lastIndex".into(), runtime)?;
            let end = self.regexp_to_length(&last_index, runtime)?.min(size);
            if end == p {
                q = advance_string_index(text.units(), q, unicode);
                continue;
            }
            self.define_own_data(
                &result,
                WireString::from(count.to_string()).into(),
                self.string(WireString::from_units(text.units()[p as usize..q as usize].to_vec()))?,
                runtime,
            )?;
            count += 1;
            if count == limit {
                return Ok(result);
            }
            p = end;
            let length = self.get(&matched, &"length".into(), runtime)?;
            let captures = self.regexp_to_length(&length, runtime)?.saturating_sub(1);
            for index in 1..=captures {
                // Captures are original Values, including undefined or object
                // identities returned by a replaced prototype exec.
                let capture = self.get(&matched, &WireString::from(index.to_string()).into(), runtime)?;
                self.define_own_data(&result, WireString::from(count.to_string()).into(), capture, runtime)?;
                count += 1;
                if count == limit {
                    return Ok(result);
                }
            }
            q = p;
        }
        self.define_own_data(
            &result,
            WireString::from(count.to_string()).into(),
            self.string(WireString::from_units(text.units()[p as usize..].to_vec()))?,
            runtime,
        )?;
        Ok(result)
    }
    pub(super) fn string_replace(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        if matches!(self.kind(receiver)?, ValueKind::Undefined | ValueKind::Null) {
            return Err(ValueError::TypeError("String.replace receiver is null or undefined".into()));
        }
        let search = arguments.first().cloned().unwrap_or(self.undefined()?);
        let replacement = arguments.get(1).cloned().unwrap_or(self.undefined()?);
        if !matches!(self.kind(&search)?, ValueKind::Null | ValueKind::Undefined) {
            let replace = self.get(&search, &self.replace_key(), runtime)?;
            if !matches!(self.kind(&replace)?, ValueKind::Null | ValueKind::Undefined) {
                // Call supplies the uncoerced source and replacement. A present
                // noncallable @@replace throws instead of falling through.
                return self.call(&replace, &search, &[receiver.clone(), replacement], runtime);
            }
        }
        let text = self.implicit_text(receiver, runtime)?;
        let search = self.implicit_text(&search, runtime)?;
        let functional = self.kind(&replacement)? == ValueKind::Function;
        let template = if functional { None } else { Some(self.implicit_text(&replacement, runtime)?) };
        let Some(position) = find_units(text.units(), search.units(), 0) else {
            return self.string(text);
        };
        let substitute = if functional {
            let output = self.call(
                &replacement,
                &self.undefined()?,
                &[self.string(search.clone())?, self.number(position as f64)?, self.string(text.clone())?],
                runtime,
            )?;
            self.implicit_text(&output, runtime)?
        } else {
            self.regexp_substitution(
                SubstitutionContext { matched: &search, text: &text, position, captures: &[], named: None },
                template.as_ref().ok_or(ValueError::InvalidNode)?,
                runtime,
            )?
        };
        let mut output = text.units()[..position].to_vec();
        output.extend_from_slice(substitute.units());
        output.extend_from_slice(&text.units()[position + search.len()..]);
        self.string(WireString::from_units(output))
    }
    /// String.prototype.replaceAll at fixed sanitize-text.ts. General dynamic
    /// @@match/flags/@@replace, receiver and replacement conversions stay live.
    pub(super) fn string_replace_all(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        if matches!(self.kind(receiver)?, ValueKind::Undefined | ValueKind::Null) {
            return Err(ValueError::TypeError("String.replaceAll receiver is null or undefined".into()));
        }
        let search = arguments.first().cloned().unwrap_or(self.undefined()?);
        let replacement = arguments.get(1).cloned().unwrap_or(self.undefined()?);
        if !matches!(self.kind(&search)?, ValueKind::Null | ValueKind::Undefined) {
            if self.is_regexp(&search, runtime)? {
                let flags = self.get(&search, &"flags".into(), runtime)?;
                if matches!(self.kind(&flags)?, ValueKind::Null | ValueKind::Undefined) {
                    return Err(ValueError::TypeError("RegExp flags is null or undefined".into()));
                }
                let flags = self.implicit_text(&flags, runtime)?;
                if !flags.units().contains(&u16::from(b'g')) {
                    return Err(ValueError::TypeError("String.replaceAll requires a global RegExp".into()));
                }
            }
            let replace = self.get(&search, &self.replace_key(), runtime)?;
            if !matches!(self.kind(&replace)?, ValueKind::Null | ValueKind::Undefined) {
                return self.call(&replace, &search, &[receiver.clone(), replacement], runtime);
            }
        }
        let text = self.implicit_text(receiver, runtime)?;
        let search = self.implicit_text(&search, runtime)?;
        let functional = self.kind(&replacement)? == ValueKind::Function;
        let template = if functional { None } else { Some(self.implicit_text(&replacement, runtime)?) };
        let mut positions = Vec::new();
        let mut start = 0usize;
        while let Some(position) = find_units(text.units(), search.units(), start) {
            positions.push(position);
            start = position + search.len().max(1);
            if start > text.len() {
                break;
            }
        }
        let mut output = Vec::new();
        let mut end = 0usize;
        for position in positions {
            let substitute = if functional {
                let output = self.call(
                    &replacement,
                    &self.undefined()?,
                    &[self.string(search.clone())?, self.number(position as f64)?, self.string(text.clone())?],
                    runtime,
                )?;
                self.implicit_text(&output, runtime)?
            } else {
                self.regexp_substitution(
                    SubstitutionContext { matched: &search, text: &text, position, captures: &[], named: None },
                    template.as_ref().ok_or(ValueError::InvalidNode)?,
                    runtime,
                )?
            };
            output.try_reserve(position - end + substitute.len()).map_err(|_| ValueError::Allocation)?;
            output.extend_from_slice(&text.units()[end..position]);
            output.extend_from_slice(substitute.units());
            end = position + search.len();
        }
        output.try_reserve(text.len() - end).map_err(|_| ValueError::Allocation)?;
        output.extend_from_slice(&text.units()[end..]);
        self.string(WireString::from_units(output))
    }
    fn regexp_replace(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.require_regexp_object(receiver)?;
        let input = arguments.first().cloned().unwrap_or(self.undefined()?);
        let text = self.implicit_text(&input, runtime)?;
        let replacement = arguments.get(1).cloned().unwrap_or(self.undefined()?);
        let functional = self.kind(&replacement)? == ValueKind::Function;
        let template = if functional { None } else { Some(self.implicit_text(&replacement, runtime)?) };
        let global = self.get(receiver, &"global".into(), runtime)?;
        let global = self.truthy(&global)?;
        let unicode = if global {
            let unicode = self.get(receiver, &"unicode".into(), runtime)?;
            let unicode = if self.truthy(&unicode)? {
                true
            } else {
                let sets = self.get(receiver, &"unicodeSets".into(), runtime)?;
                self.truthy(&sets)?
            };
            self.assign(receiver, "lastIndex".into(), self.number(0.0)?, runtime)?;
            unicode
        } else {
            false
        };
        let mut results = Vec::new();
        loop {
            let result = self.regexp_exec_result(receiver, &text, runtime)?;
            if self.kind(&result)? == ValueKind::Null {
                break;
            }
            results.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            results.push(result.clone());
            if !global {
                break;
            }
            let matched = self.get(&result, &"0".into(), runtime)?;
            if self.implicit_text(&matched, runtime)?.is_empty() {
                let last_index = self.get(receiver, &"lastIndex".into(), runtime)?;
                let last_index = self.regexp_to_length(&last_index, runtime)?;
                let next = advance_string_index(text.units(), last_index, unicode);
                self.assign(receiver, "lastIndex".into(), self.number(next as f64)?, runtime)?;
            }
        }
        let mut next_source_position = 0usize;
        let mut output = Vec::new();
        for result in results {
            let length = self.get(&result, &"length".into(), runtime)?;
            let captures_length = self.regexp_to_length(&length, runtime)?.saturating_sub(1);
            let matched = self.get(&result, &"0".into(), runtime)?;
            let matched = self.implicit_text(&matched, runtime)?;
            let position = self.get(&result, &"index".into(), runtime)?;
            let position =
                integer_or_infinity(self.to_number(&position, runtime)?).clamp(0.0, text.len() as f64) as usize;
            let mut captures = Vec::new();
            let count = usize::try_from(captures_length).map_err(|_| ValueError::Allocation)?;
            captures.try_reserve(count).map_err(|_| ValueError::Allocation)?;
            for index in 1..=captures_length {
                let capture = self.get(&result, &WireString::from(index.to_string()).into(), runtime)?;
                captures.push(if self.kind(&capture)? == ValueKind::Undefined {
                    None
                } else {
                    Some(self.implicit_text(&capture, runtime)?)
                });
            }
            let groups = self.get(&result, &"groups".into(), runtime)?;
            let groups_present = self.kind(&groups)? != ValueKind::Undefined;
            let substitute = if functional {
                let mut args = Vec::new();
                args.try_reserve(captures.len() + 4).map_err(|_| ValueError::Allocation)?;
                args.push(self.string(matched.clone())?);
                for capture in &captures {
                    args.push(match capture {
                        Some(text) => self.string(text.clone())?,
                        None => self.undefined()?,
                    });
                }
                args.push(self.number(position as f64)?);
                args.push(self.string(text.clone())?);
                if groups_present {
                    args.push(groups);
                }
                let value = self.call(&replacement, &self.undefined()?, &args, runtime)?;
                self.implicit_text(&value, runtime)?
            } else {
                let groups = if !groups_present {
                    None
                } else {
                    match self.kind(&groups)? {
                        ValueKind::Null => {
                            return Err(ValueError::TypeError("Cannot convert null named captures to object".into()));
                        }
                        ValueKind::Object | ValueKind::Array | ValueKind::Function => Some(groups),
                        _ => {
                            return Err(ValueError::UnsupportedOperation(
                                "RegExp named capture primitive boxing is not connected",
                            ));
                        }
                    }
                };
                self.regexp_substitution(
                    SubstitutionContext {
                        matched: &matched,
                        text: &text,
                        position,
                        captures: &captures,
                        named: groups.as_ref(),
                    },
                    template.as_ref().ok_or(ValueError::InvalidNode)?,
                    runtime,
                )?
            };
            // Coercions and replacement callbacks above happen even for an
            // overlapping/out-of-order result excluded from the final output.
            if position >= next_source_position {
                output.extend_from_slice(&text.units()[next_source_position..position]);
                output.extend_from_slice(substitute.units());
                next_source_position = position.saturating_add(matched.len());
            }
        }
        if next_source_position < text.len() {
            output.extend_from_slice(&text.units()[next_source_position..]);
        }
        self.string(WireString::from_units(output))
    }
    fn regexp_substitution(
        self: &Arc<Self>,
        context: SubstitutionContext<'_>,
        replacement: &WireString,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<WireString> {
        let SubstitutionContext { matched, text, position, captures, named } = context;
        let mut output = Vec::new();
        let units = replacement.units();
        let mut index = 0;
        while index < units.len() {
            if units[index] != 0x24 || index + 1 == units.len() {
                output.push(units[index]);
                index += 1;
                continue;
            }
            let next = units[index + 1];
            match next {
                0x24 => {
                    output.push(0x24);
                    index += 2;
                }
                0x26 => {
                    output.extend_from_slice(matched.units());
                    index += 2;
                }
                0x60 => {
                    output.extend_from_slice(&text.units()[..position]);
                    index += 2;
                }
                0x27 => {
                    output.extend_from_slice(&text.units()[position.saturating_add(matched.len()).min(text.len())..]);
                    index += 2;
                }
                0x30..=0x39 => {
                    let first = usize::from(next - 0x30);
                    let second = units.get(index + 2).copied().filter(|unit| (0x30..=0x39).contains(unit));
                    let two = second.map(|unit| first * 10 + usize::from(unit - 0x30));
                    let (capture, consumed) =
                        if let Some(two) = two.filter(|value| *value > 0 && *value <= captures.len()) {
                            (two, 3)
                        } else if first > 0 && first <= captures.len() {
                            (first, 2)
                        } else {
                            output.push(0x24);
                            index += 1;
                            continue;
                        };
                    if let Some(capture) = &captures[capture - 1] {
                        output.extend_from_slice(capture.units());
                    }
                    index += consumed;
                }
                0x3c if named.is_some() => {
                    if let Some(end) = units[index + 2..].iter().position(|unit| *unit == 0x3e) {
                        let end = end + index + 2;
                        let name = WireString::from_units(units[index + 2..end].to_vec());
                        let capture = self.get(named.ok_or(ValueError::InvalidNode)?, &name.into(), runtime)?;
                        if self.kind(&capture)? != ValueKind::Undefined {
                            output.extend_from_slice(self.implicit_text(&capture, runtime)?.units());
                        }
                        index = end + 1;
                    } else {
                        output.push(0x24);
                        index += 1;
                    }
                }
                _ => {
                    output.push(0x24);
                    index += 1;
                }
            }
        }
        Ok(WireString::from_units(output))
    }
}

fn advance_string_index(text: &[u16], index: u64, unicode: bool) -> u64 {
    if unicode && index + 1 < text.len() as u64 {
        let start = index as usize;
        if (0xd800..=0xdbff).contains(&text[start]) && (0xdc00..=0xdfff).contains(&text[start + 1]) {
            return index + 2;
        }
    }
    index + 1
}

fn escaped_source(pattern: &WireString) -> WireString {
    if pattern.is_empty() {
        return "(?:)".into();
    }
    let mut output = Vec::new();
    let mut escaped = false;
    for &unit in pattern.units() {
        match unit {
            0x2f if !escaped => output.extend_from_slice(&[0x5c, 0x2f]),
            0x0a => output.extend_from_slice(&[0x5c, 0x6e]),
            0x0d => output.extend_from_slice(&[0x5c, 0x72]),
            0x2028 => output.extend("\\u2028".encode_utf16()),
            0x2029 => output.extend("\\u2029".encode_utf16()),
            _ => output.push(unit),
        }
        escaped = unit == 0x5c && !escaped;
    }
    WireString::from_units(output)
}
