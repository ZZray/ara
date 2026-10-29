//! One read adapter for the fixed Native checker, without a graph snapshot.
//! Wire values borrow their input; Graph values alias the original arena.
//! Property, method, callback and iterator effects use the current resolver.

use crate::graph_values::GraphExecution;
use ara_rpc::json::{WireString, WireValue};
use ara_rpc::value_heap::{
    BigIntDecimal, CompiledFunctionSpec, NodeId, PropertyKey, ValueError, ValueHandle, ValueKind, ValueRuntime,
    ValueSnapshot,
};
use std::cmp::Ordering;
use std::collections::HashSet;

static NULL: WireValue = WireValue::Null;
const OBJECT_FUNCTIONS: &[&str] = &[
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "toLocaleString",
];
const ARRAY_FUNCTIONS: &[&str] = &[
    "constructor",
    "at",
    "concat",
    "copyWithin",
    "fill",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "lastIndexOf",
    "pop",
    "push",
    "reverse",
    "shift",
    "unshift",
    "slice",
    "sort",
    "splice",
    "includes",
    "indexOf",
    "join",
    "keys",
    "entries",
    "values",
    "forEach",
    "filter",
    "flat",
    "flatMap",
    "map",
    "every",
    "some",
    "reduce",
    "reduceRight",
    "toReversed",
    "toSorted",
    "toSpliced",
    "with",
    "toLocaleString",
    "toString",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum NodeIdentity {
    Wire(usize),
    Graph(u64, NodeId),
    ObjectPrototype,
    ArrayPrototype,
}

#[derive(Clone, Debug)]
pub(super) enum ValueRef<'a> {
    Wire(&'a WireValue),
    String(WireString),
    Number(f64),
    Boolean(bool),
    Undefined,
    Function {
        owner: &'static str,
        name: &'static str,
    },
    ObjectPrototype,
    ArrayPrototype,
    EmptyObject,
    /// Scratch arrays exist only on the immutable Wire entrypoint.
    List(Vec<Self>),
    Graph(ValueHandle),
}

#[derive(Clone, Copy)]
pub(super) enum ReadContext<'r> {
    Wire,
    Graph { factory: GraphExecution<'r>, runtime: &'r dyn ValueRuntime },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LoopControl {
    Continue,
    Break,
}

impl ReadContext<'_> {
    pub(super) fn kind(&self, value: &ValueRef<'_>) -> Result<ValueKind, ValueError> {
        Ok(match value {
            ValueRef::Graph(value) => return self.graph()?.0.heap().kind(value),
            ValueRef::Wire(WireValue::Null) => ValueKind::Null,
            ValueRef::Wire(WireValue::Bool(_)) | ValueRef::Boolean(_) => ValueKind::Boolean,
            ValueRef::Wire(WireValue::Number(_)) | ValueRef::Number(_) => ValueKind::Number,
            ValueRef::Wire(WireValue::String(_)) | ValueRef::String(_) => ValueKind::String,
            ValueRef::Wire(WireValue::Object(_)) | ValueRef::ObjectPrototype | ValueRef::EmptyObject => {
                ValueKind::Object
            }
            ValueRef::Wire(WireValue::Array(_)) | ValueRef::ArrayPrototype | ValueRef::List(_) => ValueKind::Array,
            ValueRef::Function { .. } => ValueKind::Function,
            ValueRef::Undefined => ValueKind::Undefined,
        })
    }
    pub(super) fn is_object(&self, value: &ValueRef<'_>) -> Result<bool, ValueError> {
        Ok(self.kind(value)? == ValueKind::Object)
    }
    pub(super) fn is_array(&self, value: &ValueRef<'_>) -> Result<bool, ValueError> {
        Ok(self.kind(value)? == ValueKind::Array)
    }
    pub(super) fn is_undefined(&self, value: &ValueRef<'_>) -> Result<bool, ValueError> {
        Ok(self.kind(value)? == ValueKind::Undefined)
    }
    pub(super) fn is_null(&self, value: &ValueRef<'_>) -> Result<bool, ValueError> {
        Ok(self.kind(value)? == ValueKind::Null)
    }
    pub(super) fn boolean(&self, value: &ValueRef<'_>) -> Result<Option<bool>, ValueError> {
        Ok(match value {
            ValueRef::Wire(WireValue::Bool(value)) | ValueRef::Boolean(value) => Some(*value),
            ValueRef::Graph(value) => match self.graph()?.0.heap().snapshot(value)? {
                ValueSnapshot::Boolean(value) => Some(value),
                _ => None,
            },
            _ => None,
        })
    }
    pub(super) fn number(&self, value: &ValueRef<'_>) -> Result<Option<f64>, ValueError> {
        Ok(match value {
            ValueRef::Wire(WireValue::Number(value)) | ValueRef::Number(value) => Some(*value),
            ValueRef::Graph(value) => match self.graph()?.0.heap().snapshot(value)? {
                ValueSnapshot::Number(value) => Some(value),
                _ => None,
            },
            _ => None,
        })
    }
    pub(super) fn string(&self, value: &ValueRef<'_>) -> Result<Option<WireString>, ValueError> {
        Ok(match value {
            ValueRef::Wire(WireValue::String(value)) | ValueRef::String(value) => Some(value.clone()),
            ValueRef::Graph(value) => match self.graph()?.0.heap().snapshot(value)? {
                ValueSnapshot::String(value) => Some(value),
                _ => None,
            },
            _ => None,
        })
    }
    fn graph(&self) -> Result<(GraphExecution<'_>, &dyn ValueRuntime), ValueError> {
        match self {
            Self::Graph { factory, runtime } => Ok((*factory, *runtime)),
            Self::Wire => Err(ValueError::UnsupportedOperation("Graph value reached Wire-only checker reader")),
        }
    }
    pub(super) fn graph_handle(&self, value: &ValueRef<'_>) -> Result<ValueHandle, ValueError> {
        let (factory, _) = self.graph()?;
        match value {
            ValueRef::Graph(value) => factory.heap().adopt(value),
            ValueRef::String(value) => factory.heap().string(value.clone()),
            ValueRef::Number(value) => factory.heap().number(*value),
            ValueRef::Boolean(value) => factory.heap().boolean(*value),
            ValueRef::Undefined => factory.heap().undefined(),
            _ => Err(ValueError::UnsupportedOperation("Wire container cannot enter actual checker lineage")),
        }
    }
    pub(super) fn make_array<'a>(&self, values: Vec<ValueRef<'a>>) -> Result<ValueRef<'a>, ValueError> {
        match self {
            Self::Wire => Ok(ValueRef::List(values)),
            Self::Graph { factory, .. } => {
                let handles = values.iter().map(|value| self.graph_handle(value)).collect::<Result<Vec<_>, _>>()?;
                Ok(ValueRef::Graph(factory.heap().array(handles)?))
            }
        }
    }
    pub(super) fn empty_object<'a>(&self) -> Result<ValueRef<'a>, ValueError> {
        match self {
            Self::Wire => Ok(ValueRef::EmptyObject),
            Self::Graph { factory, .. } => Ok(ValueRef::Graph(factory.heap().object()?)),
        }
    }
    pub(super) fn get<'a>(&self, value: &ValueRef<'a>, name: &str) -> Result<ValueRef<'a>, ValueError> {
        self.property(value, &WireString::from(name))
    }
    pub(super) fn property<'a>(&self, value: &ValueRef<'a>, key: &WireString) -> Result<ValueRef<'a>, ValueError> {
        if let ValueRef::Graph(value) = value {
            let (factory, runtime) = self.graph()?;
            return factory.heap().get(value, &PropertyKey::String(key.clone()), runtime).map(ValueRef::Graph);
        }
        if let Some(own) = wire_own(value, key) {
            return Ok(own);
        }
        if key.equals_ascii("__proto__")
            && let Some(prototype) = wire_prototype(value)
        {
            return Ok(prototype);
        }
        let mut prototype = wire_prototype(value);
        while let Some(current) = prototype {
            if let Some(value) = wire_own(&current, key) {
                return Ok(value);
            }
            prototype = wire_prototype(&current);
        }
        Ok(ValueRef::Undefined)
    }
    pub(super) fn has_own(&self, value: &ValueRef<'_>, key: &WireString) -> Result<bool, ValueError> {
        if let ValueRef::Graph(value) = value {
            let (factory, runtime) = self.graph()?;
            return factory.heap().has_own(value, &PropertyKey::String(key.clone()), runtime);
        }
        Ok(wire_own(value, key).is_some())
    }
    pub(super) fn has(&self, value: &ValueRef<'_>, key: &WireString) -> Result<bool, ValueError> {
        if let ValueRef::Graph(value) = value {
            let (factory, runtime) = self.graph()?;
            return factory.heap().has(value, &PropertyKey::String(key.clone()), runtime);
        }
        if wire_own(value, key).is_some() {
            return Ok(true);
        }
        let mut prototype = wire_prototype(value);
        while let Some(current) = prototype {
            if wire_own(&current, key).is_some() {
                return Ok(true);
            }
            prototype = wire_prototype(&current);
        }
        Ok(false)
    }
    /// in converts its key independently from the original key stored in path.
    pub(super) fn has_value_key(&self, value: &ValueRef<'_>, key: &ValueRef<'_>) -> Result<bool, ValueError> {
        match self {
            Self::Graph { factory, runtime } => {
                let target = self.graph_handle(value)?;
                let key = self.graph_handle(key)?;
                let primitive = factory.heap().to_primitive(&key, true, *runtime)?;
                let property = if factory.heap().kind(&primitive)? == ValueKind::Symbol {
                    PropertyKey::Symbol(factory.heap().symbol_key(&primitive)?)
                } else {
                    PropertyKey::String(factory.heap().to_string(&primitive, *runtime)?)
                };
                factory.heap().has(&target, &property, *runtime)
            }
            Self::Wire => self.has(value, &self.implicit_string(key)?),
        }
    }
    pub(super) fn own_keys(&self, value: &ValueRef<'_>) -> Result<Vec<WireString>, ValueError> {
        if let ValueRef::Graph(value) = value {
            let (factory, runtime) = self.graph()?;
            return factory.heap().own_enumerable_string_keys(value, runtime);
        }
        Ok(match value {
            ValueRef::Wire(value @ WireValue::Object(_)) => {
                value.entries().expect("object").iter().map(|(key, _)| (*key).clone()).collect()
            }
            ValueRef::Wire(WireValue::Array(values)) => {
                (0..values.len()).map(|index| WireString::from(index.to_string())).collect()
            }
            ValueRef::List(values) => (0..values.len()).map(|index| WireString::from(index.to_string())).collect(),
            _ => Vec::new(),
        })
    }
    pub(super) fn for_in_keys(&self, value: &ValueRef<'_>) -> Result<Vec<WireString>, ValueError> {
        if let ValueRef::Graph(value) = value {
            let (factory, runtime) = self.graph()?;
            return factory.heap().for_in_string_keys(value, runtime);
        }
        self.own_keys(value)
    }
    pub(super) fn identity(&self, value: &ValueRef<'_>) -> Result<Option<NodeIdentity>, ValueError> {
        if !matches!(self.kind(value)?, ValueKind::Object | ValueKind::Array) {
            return Ok(None);
        }
        Ok(match value {
            ValueRef::Graph(value) => Some(NodeIdentity::Graph(value.heap().heap_token(), value.node_id())),
            ValueRef::Wire(value) => Some(NodeIdentity::Wire(*value as *const WireValue as usize)),
            ValueRef::ObjectPrototype => Some(NodeIdentity::ObjectPrototype),
            ValueRef::ArrayPrototype => Some(NodeIdentity::ArrayPrototype),
            ValueRef::List(values) => Some(NodeIdentity::Wire(values as *const Vec<ValueRef<'_>> as usize)),
            ValueRef::EmptyObject => None,
            _ => None,
        })
    }
    pub(super) fn received_type(&self, value: &ValueRef<'_>) -> Result<&'static str, ValueError> {
        Ok(match self.kind(value)? {
            ValueKind::Null => "null",
            ValueKind::Array => "array",
            ValueKind::String => "string",
            ValueKind::Number if self.number(value)?.is_some_and(is_integer) => "integer",
            ValueKind::Number => "number",
            ValueKind::Boolean => "boolean",
            ValueKind::Object => "object",
            ValueKind::Function => "function",
            ValueKind::BigInt => "bigint",
            ValueKind::Symbol => "symbol",
            ValueKind::Undefined => "undefined",
        })
    }
    pub(super) fn matches_type(&self, value: &ValueRef<'_>, kind: &ValueRef<'_>) -> Result<bool, ValueError> {
        let Some(kind) = self.string(kind)? else {
            return Ok(false);
        };
        Ok(if kind.equals_ascii("string") {
            self.kind(value)? == ValueKind::String
        } else if kind.equals_ascii("number") {
            self.number(value)?.is_some_and(f64::is_finite)
        } else if kind.equals_ascii("integer") {
            self.number(value)?.is_some_and(is_integer)
        } else if kind.equals_ascii("boolean") {
            self.kind(value)? == ValueKind::Boolean
        } else if kind.equals_ascii("object") {
            self.is_object(value)?
        } else if kind.equals_ascii("array") {
            self.is_array(value)?
        } else if kind.equals_ascii("null") {
            self.is_null(value)?
        } else {
            false
        })
    }
    pub(super) fn truthy(&self, value: &ValueRef<'_>) -> Result<bool, ValueError> {
        if let ValueRef::Graph(value) = value {
            return self.graph()?.0.heap().truthy(value);
        }
        Ok(match self.kind(value)? {
            ValueKind::Null | ValueKind::Undefined => false,
            ValueKind::Boolean => self.boolean(value)?.expect("boolean"),
            ValueKind::Number => self.number(value)?.is_some_and(|value| value != 0.0 && !value.is_nan()),
            ValueKind::String => self.string(value)?.is_some_and(|value| !value.units().is_empty()),
            _ => true,
        })
    }
    pub(super) fn implicit_string(&self, value: &ValueRef<'_>) -> Result<WireString, ValueError> {
        if let ValueRef::Graph(value) = value {
            let (factory, runtime) = self.graph()?;
            if factory.heap().kind(value)? == ValueKind::Symbol {
                return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
            }
            return factory.heap().to_string(value, runtime);
        }
        if let Some(value) = self.string(value)? {
            return Ok(value);
        }
        if let Some(value) = self.number(value)? {
            return Ok(number_message(value).into());
        }
        if let Some(value) = self.boolean(value)? {
            return Ok(if value { "true" } else { "false" }.into());
        }
        if self.is_null(value)? {
            return Ok("null".into());
        }
        if self.is_undefined(value)? {
            return Ok("undefined".into());
        }
        Err(ValueError::UnsupportedOperation("Wire scratch container implicit conversion is not used by fixed checker"))
    }
    pub(super) fn numeric_value(&self, value: &ValueRef<'_>) -> Result<f64, ValueError> {
        if let ValueRef::Graph(value) = value {
            let (factory, runtime) = self.graph()?;
            return factory.heap().to_number(value, runtime);
        }
        if let Some(number) = self.number(value)? {
            return Ok(number);
        }
        if let Some(boolean) = self.boolean(value)? {
            return Ok(if boolean { 1.0 } else { 0.0 });
        }
        if self.is_null(value)? {
            return Ok(0.0);
        }
        if self.is_undefined(value)? {
            return Ok(f64::NAN);
        }
        Err(ValueError::UnsupportedOperation("Wire nonnumber coercion is unreachable in fixed numeric checker"))
    }
    /// Abstract relational comparison, retaining numeric ToPrimitive effects.
    pub(super) fn compare(
        &self,
        left: &ValueRef<'_>,
        right: &ValueRef<'_>,
        operator: &str,
    ) -> Result<bool, ValueError> {
        let order = match self {
            Self::Wire => self.numeric_value(left)?.partial_cmp(&self.numeric_value(right)?),
            Self::Graph { factory, runtime } => {
                let left = factory.heap().to_primitive(&self.graph_handle(left)?, false, *runtime)?;
                let right = factory.heap().to_primitive(&self.graph_handle(right)?, false, *runtime)?;
                match (factory.heap().snapshot(&left)?, factory.heap().snapshot(&right)?) {
                    (ValueSnapshot::String(left), ValueSnapshot::String(right)) => {
                        Some(left.units().cmp(right.units()))
                    }
                    (ValueSnapshot::BigInt(left), ValueSnapshot::BigInt(right)) => {
                        Some(compare_bigints(left.as_str(), right.as_str()))
                    }
                    (ValueSnapshot::BigInt(left), ValueSnapshot::String(right)) => {
                        parse_string_bigint(&right).map(|right| compare_bigints(left.as_str(), &right))
                    }
                    (ValueSnapshot::String(left), ValueSnapshot::BigInt(right)) => {
                        parse_string_bigint(&left).map(|left| compare_bigints(&left, right.as_str()))
                    }
                    (ValueSnapshot::BigInt(left), _) => {
                        compare_number_bigint(factory.heap().to_number(&right, *runtime)?, left.as_str())
                            .map(Ordering::reverse)
                    }
                    (_, ValueSnapshot::BigInt(right)) => {
                        compare_number_bigint(factory.heap().to_number(&left, *runtime)?, right.as_str())
                    }
                    _ => factory
                        .heap()
                        .to_number(&left, *runtime)?
                        .partial_cmp(&factory.heap().to_number(&right, *runtime)?),
                }
            }
        };
        Ok(match operator {
            "<" => order == Some(Ordering::Less),
            ">" => order == Some(Ordering::Greater),
            "<=" => matches!(order, Some(Ordering::Less | Ordering::Equal)),
            ">=" => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
            _ => false,
        })
    }
    pub(super) fn index<'a>(&self, value: &ValueRef<'a>, index: usize) -> Result<ValueRef<'a>, ValueError> {
        self.property(value, &WireString::from(index.to_string()))
    }
    pub(super) fn length_less(&self, index: usize, value: &ValueRef<'_>) -> Result<bool, ValueError> {
        self.compare(&ValueRef::Number(index as f64), &self.get(value, "length")?, "<")
    }

    pub(super) fn predicate<'a, F>(
        &self,
        receiver: &ValueRef<'a>,
        name: &str,
        source: &'static CompiledFunctionSpec,
        captures: &[ValueRef<'a>],
        mut predicate: F,
    ) -> Result<ValueRef<'a>, ValueError>
    where
        F: FnMut(ValueRef<'a>, &ReadContext<'_>) -> Result<bool, ValueError> + Send,
    {
        let method = self.get(receiver, name)?;
        if let ValueRef::Graph(receiver) = receiver {
            let (factory, runtime) = self.graph()?;
            let method = self.graph_handle(&method)?;
            let captures = captures.iter().map(|value| self.graph_handle(value)).collect::<Result<Vec<_>, _>>()?;
            let callback = factory.heap().fresh_compiled_function(source, &captures)?;
            return factory.heap().call(&method, receiver, &[callback], runtime).map(ValueRef::Graph);
        }
        if !matches!(&method, ValueRef::Function { owner: "Array.prototype", name: selected } if *selected == name) {
            return Err(ValueError::TypeError("Array predicate method is not callable".into()));
        }
        let length = self.number(&self.get(receiver, "length")?)?.expect("Wire array length") as usize;
        let mut selected = Vec::new();
        for index in 0..length {
            let entry = self.index(receiver, index)?;
            let matched = predicate(entry.clone(), self)?;
            match name {
                "some" if matched => return Ok(ValueRef::Boolean(true)),
                "every" if !matched => return Ok(ValueRef::Boolean(false)),
                "filter" if matched => selected.push(entry),
                _ => {}
            }
        }
        match name {
            "some" => Ok(ValueRef::Boolean(false)),
            "every" => Ok(ValueRef::Boolean(true)),
            "filter" => self.make_array(selected),
            _ => Err(ValueError::UnsupportedOperation("Unknown fixed checker predicate method")),
        }
    }
    pub(super) fn call<'a>(
        &self,
        receiver: &ValueRef<'a>,
        name: &str,
        arguments: &[ValueRef<'a>],
    ) -> Result<ValueRef<'a>, ValueError> {
        let method = self.get(receiver, name)?;
        if let ValueRef::Graph(receiver) = receiver {
            let (factory, runtime) = self.graph()?;
            let method = self.graph_handle(&method)?;
            let arguments = arguments.iter().map(|value| self.graph_handle(value)).collect::<Result<Vec<_>, _>>()?;
            return factory.heap().call(&method, receiver, &arguments, runtime).map(ValueRef::Graph);
        }
        if !matches!(&method, ValueRef::Function { owner: "Array.prototype", name: selected } if *selected == name) {
            return Err(ValueError::TypeError("Array method is not callable".into()));
        }
        let length = self.number(&self.get(receiver, "length")?)?.expect("Wire array length") as usize;
        if name == "includes" {
            for index in 0..length {
                if self.same_value(&self.index(receiver, index)?, &arguments[0])? {
                    return Ok(ValueRef::Boolean(true));
                }
            }
            return Ok(ValueRef::Boolean(false));
        }
        if name == "join" {
            let separator = self.implicit_string(&arguments[0])?;
            let mut units = Vec::new();
            for index in 0..length {
                if index != 0 {
                    units.extend_from_slice(separator.units());
                }
                units.extend_from_slice(self.implicit_string(&self.index(receiver, index)?)?.units());
            }
            return Ok(ValueRef::String(WireString::from_units(units)));
        }
        Err(ValueError::UnsupportedOperation("Unknown fixed checker array method"))
    }
    pub(super) fn for_of<'a, F>(&self, value: &ValueRef<'a>, body: F) -> Result<(), ValueError>
    where
        F: FnMut(ValueRef<'a>) -> Result<LoopControl, ValueError>,
    {
        if let ValueRef::Graph(value) = value {
            return self.for_of_graph(value, body);
        }
        self.for_of_wire(value, body)
    }

    // Keep actual iterator and error/close temporaries off the Wire recursion path.
    #[inline(never)]
    fn for_of_graph<'a, F>(&self, value: &ValueHandle, mut body: F) -> Result<(), ValueError>
    where
        F: FnMut(ValueRef<'a>) -> Result<LoopControl, ValueError>,
    {
        let (factory, runtime) = self.graph()?;
        let mut iterator = factory.heap().get_iterator(value, runtime)?;
        while let Some(value) = factory.heap().iterator_next(&mut iterator, runtime)? {
            match body(ValueRef::Graph(value)) {
                Ok(LoopControl::Continue) => {}
                Ok(LoopControl::Break) => {
                    return factory.heap().iterator_close(&mut iterator, runtime);
                }
                Err(original) => {
                    return match factory.heap().iterator_close(&mut iterator, runtime) {
                        Ok(()) => Err(original),
                        Err(close) if original.is_native_exception() && close.is_native_exception() => Err(original),
                        Err(close) => {
                            Err(ValueError::IteratorClose { original: Box::new(original), close: Box::new(close) })
                        }
                    };
                }
            }
        }
        Ok(())
    }

    #[inline(never)]
    fn for_of_wire<'a, F>(&self, value: &ValueRef<'a>, mut body: F) -> Result<(), ValueError>
    where
        F: FnMut(ValueRef<'a>) -> Result<LoopControl, ValueError>,
    {
        let length = self
            .number(&self.get(value, "length")?)?
            .ok_or_else(|| ValueError::TypeError("Value is not iterable".into()))? as usize;
        for index in 0..length {
            if body(self.index(value, index)?)? == LoopControl::Break {
                break;
            }
        }
        Ok(())
    }
    pub(super) fn spread_append<'a>(
        &self,
        value: &ValueRef<'a>,
        tail: ValueRef<'a>,
    ) -> Result<ValueRef<'a>, ValueError> {
        match self {
            Self::Wire => {
                let mut values = Vec::new();
                self.for_of(value, |value| {
                    values.push(value);
                    Ok(LoopControl::Continue)
                })?;
                values.push(tail);
                self.make_array(values)
            }
            Self::Graph { factory, runtime } => {
                let result = factory.heap().array_with_length(0)?;
                let mut index = 0_usize;
                self.for_of(value, |value| {
                    factory.heap().define_own_data(
                        &result,
                        PropertyKey::String(index.to_string().into()),
                        self.graph_handle(&value)?,
                        *runtime,
                    )?;
                    index = index
                        .checked_add(1)
                        .ok_or(ValueError::UnsupportedOperation("Checker spread index overflow"))?;
                    Ok(LoopControl::Continue)
                })?;
                factory.heap().define_own_data(
                    &result,
                    PropertyKey::String(index.to_string().into()),
                    self.graph_handle(&tail)?,
                    *runtime,
                )?;
                Ok(ValueRef::Graph(result))
            }
        }
    }
    fn same_value(&self, left: &ValueRef<'_>, right: &ValueRef<'_>) -> Result<bool, ValueError> {
        if let (ValueRef::Graph(left), ValueRef::Graph(right)) = (left, right) {
            return self.graph()?.0.heap().object_is(left, right);
        }
        if let (Some(left), Some(right)) = (self.identity(left)?, self.identity(right)?)
            && left == right
        {
            return Ok(true);
        }
        if self.kind(left)? != self.kind(right)? {
            return Ok(false);
        }
        if let (Some(left), Some(right)) = (self.number(left)?, self.number(right)?) {
            return Ok((left.is_nan() && right.is_nan()) || left.to_bits() == right.to_bits());
        }
        if let (Some(left), Some(right)) = (self.string(left)?, self.string(right)?) {
            return Ok(left == right);
        }
        if let (Some(left), Some(right)) = (self.boolean(left)?, self.boolean(right)?) {
            return Ok(left == right);
        }
        if self.is_null(left)? || self.is_undefined(left)? {
            return Ok(true);
        }
        if let (ValueRef::Function { owner: a, name: b }, ValueRef::Function { owner: c, name: d }) = (left, right) {
            return Ok(a == c && b == d);
        }
        Ok(false)
    }
    fn strict_equal(&self, left: &ValueRef<'_>, right: &ValueRef<'_>) -> Result<bool, ValueError> {
        if let (Some(left), Some(right)) = (self.number(left)?, self.number(right)?) {
            return Ok(left == right);
        }
        self.same_value(left, right)
    }
    pub(super) fn equal(&self, left: &ValueRef<'_>, right: &ValueRef<'_>) -> Result<bool, ValueError> {
        self.equal_inner(left, right, &mut HashSet::new())
    }
    fn equal_inner(
        &self,
        left: &ValueRef<'_>,
        right: &ValueRef<'_>,
        active: &mut HashSet<(NodeIdentity, NodeIdentity)>,
    ) -> Result<bool, ValueError> {
        if self.same_value(left, right)? {
            return Ok(true);
        }
        let left_array = self.is_array(left)?;
        let right_array = self.is_array(right)?;
        if left_array || right_array {
            if !left_array || !right_array {
                return Ok(false);
            }
            let left_length = self.get(left, "length")?;
            let right_length = self.get(right, "length")?;
            if !self.strict_equal(&left_length, &right_length)? {
                return Ok(false);
            }
        } else if !self.is_object(left)? || !self.is_object(right)? {
            return Ok(false);
        }
        let pair = self.identity(left)?.zip(self.identity(right)?);
        if let Some(pair) = pair
            && !active.insert(pair)
        {
            return Err(ValueError::UnsupportedOperation("Fixed Native equality reached a distinct cyclic pair"));
        }
        let result = (|| {
            if left_array {
                let mut index = 0_usize;
                while self.length_less(index, left)? {
                    let left = self.index(left, index)?;
                    let right = self.index(right, index)?;
                    if !self.equal_inner(&left, &right, active)? {
                        return Ok(false);
                    }
                    index = index
                        .checked_add(1)
                        .ok_or(ValueError::UnsupportedOperation("Checker equality index overflow"))?;
                }
                return Ok(true);
            }
            let right_length = self.for_in_keys(right)?.len();
            let mut left_length = 0;
            for key in self.for_in_keys(left)? {
                left_length += 1;
                if !self.has(right, &key)? {
                    return Ok(false);
                }
                let left = self.property(left, &key)?;
                let right = self.property(right, &key)?;
                if !self.equal_inner(&left, &right, active)? {
                    return Ok(false);
                }
            }
            Ok(left_length == right_length)
        })();
        if let Some(pair) = pair {
            active.remove(&pair);
        }
        result
    }
    pub(super) fn wire_types(&self, value: ValueRef<'_>) -> Vec<WireString> {
        let ValueRef::List(values) = value else { unreachable!("Wire schemaTypes always returns a scratch list") };
        values
            .into_iter()
            .map(|value| self.string(&value).expect("Wire scalar read").expect("Wire effective type is string"))
            .collect()
    }
}

fn wire_own<'a>(value: &ValueRef<'a>, key: &WireString) -> Option<ValueRef<'a>> {
    match value {
        ValueRef::Wire(WireValue::Object(entries)) => {
            entries.iter().find(|(candidate, _)| candidate == key).map(|(_, value)| ValueRef::Wire(value))
        }
        ValueRef::Wire(WireValue::Array(items)) => {
            if key.equals_ascii("length") {
                Some(ValueRef::Number(items.len() as f64))
            } else {
                array_index(key).and_then(|index| items.get(index)).map(ValueRef::Wire)
            }
        }
        ValueRef::List(items) => {
            if key.equals_ascii("length") {
                Some(ValueRef::Number(items.len() as f64))
            } else {
                array_index(key).and_then(|index| items.get(index)).cloned()
            }
        }
        ValueRef::ObjectPrototype => {
            if key.equals_ascii("__proto__") {
                Some(ValueRef::Wire(&NULL))
            } else {
                intrinsic(key, OBJECT_FUNCTIONS, "Object.prototype")
            }
        }
        ValueRef::ArrayPrototype => {
            if key.equals_ascii("length") {
                Some(ValueRef::Number(0.0))
            } else {
                intrinsic(key, ARRAY_FUNCTIONS, "Array.prototype")
            }
        }
        _ => None,
    }
}
fn wire_prototype<'a>(value: &ValueRef<'a>) -> Option<ValueRef<'a>> {
    match value {
        ValueRef::Wire(WireValue::Object(_)) | ValueRef::ArrayPrototype | ValueRef::EmptyObject => {
            Some(ValueRef::ObjectPrototype)
        }
        ValueRef::Wire(WireValue::Array(_)) | ValueRef::List(_) => Some(ValueRef::ArrayPrototype),
        _ => None,
    }
}
fn intrinsic<'a>(key: &WireString, names: &[&'static str], owner: &'static str) -> Option<ValueRef<'a>> {
    names.iter().copied().find(|name| key.equals_ascii(name)).map(|name| ValueRef::Function { owner, name })
}
fn array_index(key: &WireString) -> Option<usize> {
    let units = key.units();
    if units.is_empty() || (units.len() > 1 && units[0] == u16::from(b'0')) {
        return None;
    }
    let mut index = 0_u64;
    for &unit in units {
        if !(u16::from(b'0')..=u16::from(b'9')).contains(&unit) {
            return None;
        }
        index = index.checked_mul(10)?.checked_add(u64::from(unit - u16::from(b'0')))?;
    }
    if index >= u64::from(u32::MAX) {
        return None;
    }
    usize::try_from(index).ok()
}

fn compare_bigints(left: &str, right: &str) -> Ordering {
    let left_negative = left.starts_with('-');
    let right_negative = right.starts_with('-');
    if left_negative != right_negative {
        return if left_negative { Ordering::Less } else { Ordering::Greater };
    }
    let left = left.trim_start_matches('-');
    let right = right.trim_start_matches('-');
    let order = left.len().cmp(&right.len()).then_with(|| left.cmp(right));
    if left_negative { order.reverse() } else { order }
}
fn multiply_decimal(digits: &mut Vec<u8>, multiplier: u16, mut carry: u16) {
    for digit in digits.iter_mut() {
        let value = u16::from(*digit) * multiplier + carry;
        *digit = (value % 10) as u8;
        carry = value / 10;
    }
    while carry != 0 {
        digits.push((carry % 10) as u8);
        carry /= 10;
    }
}
fn compare_number_bigint(number: f64, bigint: &str) -> Option<Ordering> {
    if number.is_nan() {
        return None;
    }
    if number == f64::INFINITY {
        return Some(Ordering::Greater);
    }
    if number == f64::NEG_INFINITY {
        return Some(Ordering::Less);
    }
    let bits = number.abs().to_bits();
    let field = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = (bits & ((1_u64 << 52) - 1)) | if field == 0 { 0 } else { 1_u64 << 52 };
    let shift = if field == 0 { -1022 - 52 } else { field - 1023 - 52 };
    let mut integer = if shift < 0 { if -shift >= 64 { 0 } else { mantissa >> (-shift) } } else { mantissa };
    let mut digits = Vec::new();
    loop {
        digits.push((integer % 10) as u8);
        integer /= 10;
        if integer == 0 {
            break;
        }
    }
    for _ in 0..shift.max(0) {
        multiply_decimal(&mut digits, 2, 0);
    }
    let mut text: String = digits.iter().rev().map(|digit| char::from(b'0' + digit)).collect();
    if number.is_sign_negative() && text != "0" {
        text.insert(0, '-');
    }
    let order = compare_bigints(&text, bigint);
    Some(if order == Ordering::Equal && number.fract() != 0.0 {
        if number.is_sign_negative() { Ordering::Less } else { Ordering::Greater }
    } else {
        order
    })
}
fn parse_string_bigint(value: &WireString) -> Option<String> {
    let text = value.to_utf8().ok()?;
    let text = text.trim_matches(|ch: char| matches!(ch, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'));
    if text.is_empty() {
        return Some("0".into());
    }
    if let Ok(value) = BigIntDecimal::parse(text) {
        return Some(value.as_str().into());
    }
    let (radix, digits) = if let Some(digits) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        (16, digits)
    } else if let Some(digits) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
        (8, digits)
    } else {
        let digits = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B"))?;
        (2, digits)
    };
    if digits.is_empty() {
        return None;
    }
    let mut decimal = vec![0];
    for ch in digits.chars() {
        multiply_decimal(&mut decimal, radix as u16, ch.to_digit(radix)? as u16);
    }
    Some(decimal.iter().rev().map(|digit| char::from(b'0' + digit)).collect())
}

pub(super) fn is_integer(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0
}
/// ECMAScript String(number), distinct from JSON.stringify(nonfinite)=null.
pub(super) fn number_message(number: f64) -> String {
    if number.is_nan() {
        "NaN".into()
    } else if number == f64::INFINITY {
        "Infinity".into()
    } else if number == f64::NEG_INFINITY {
        "-Infinity".into()
    } else {
        WireValue::Number(number).stringify()
    }
}
