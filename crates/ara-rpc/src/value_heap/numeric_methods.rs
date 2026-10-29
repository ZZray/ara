//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d, packages/ai/src/utils/validation.ts.
//! MIT: Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY.md.
//! Arithmetic and Number methods required by fixed validation.ts scanning.
//! Actual callers select native methods and values before these bodies run.

use super::*;

impl ValueHeap {
    fn default_primitive(
        self: &Arc<Self>,
        value: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        if is_primitive_kind(self.kind(value)?) {
            return self.adopt(value);
        }
        if let ValueSnapshot::Opaque { capability, .. } = self.snapshot(value)? {
            let result = self.runtime_value(runtime.resolve(RuntimeRequest::Opaque {
                capability,
                target: value,
                receiver: value,
                operation: OpaqueOperation::ToPrimitiveDefault,
            }))?;
            if !is_primitive_kind(self.kind(&result)?) {
                return Err(ValueError::TypeError("Opaque conversion returned an object".into()));
            }
            return Ok(result);
        }
        let exotic = self.get(value, &self.to_primitive_key(), runtime)?;
        if !matches!(self.kind(&exotic)?, ValueKind::Undefined | ValueKind::Null) {
            let result = self.call(&exotic, value, &[self.string("default".into())?], runtime)?;
            if is_primitive_kind(self.kind(&result)?) {
                return Ok(result);
            }
            return Err(ValueError::TypeError("Symbol.toPrimitive returned an object".into()));
        }
        // Ordinary default hint uses number order without reading @@toPrimitive
        // a second time. Each getter/call remains at its original position.
        for name in ["valueOf", "toString"] {
            let method = self.get(value, &name.into(), runtime)?;
            if self.kind(&method)? == ValueKind::Function {
                let result = self.call(&method, value, &[], runtime)?;
                if is_primitive_kind(self.kind(&result)?) {
                    return Ok(result);
                }
            }
        }
        Err(ValueError::TypeError("Cannot convert object to primitive value".into()))
    }

    pub fn add(
        self: &Arc<Self>,
        left: &ValueHandle,
        right: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.admit(left)?;
        self.admit(right)?;
        let left = self.default_primitive(left, runtime)?;
        let right = self.default_primitive(right, runtime)?;
        if self.kind(&left)? == ValueKind::String || self.kind(&right)? == ValueKind::String {
            let mut units = self.implicit_text(&left, runtime)?.units().to_vec();
            units.extend_from_slice(self.implicit_text(&right, runtime)?.units());
            return self.string(WireString::from_units(units));
        }
        match (self.snapshot(&left)?, self.snapshot(&right)?) {
            (ValueSnapshot::BigInt(left), ValueSnapshot::BigInt(right)) => {
                self.bigint_decimal(&add_decimal(left.as_str(), right.as_str()))
            }
            (ValueSnapshot::BigInt(_), _) | (_, ValueSnapshot::BigInt(_)) => {
                Err(ValueError::TypeError("Cannot mix BigInt and other types".into()))
            }
            _ => self.number(self.to_number(&left, runtime)? + self.to_number(&right, runtime)?),
        }
    }

    pub fn increment(self: &Arc<Self>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<ValueHandle> {
        let primitive = self.to_primitive(value, false, runtime)?;
        if let ValueSnapshot::BigInt(value) = self.snapshot(&primitive)? {
            self.bigint_decimal(&add_decimal(value.as_str(), "1"))
        } else {
            self.number(self.to_number(&primitive, runtime)? + 1.0)
        }
    }

    pub(super) fn number_to_string(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        args: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        let number = match self.snapshot(receiver)? {
            ValueSnapshot::Number(number) => number,
            ValueSnapshot::Opaque { kind: OpaqueKind::BoxedNumber, capability, .. } => {
                let unboxed = self.runtime_value(runtime.resolve(RuntimeRequest::Opaque {
                    capability,
                    target: receiver,
                    receiver,
                    operation: OpaqueOperation::Unbox,
                }))?;
                let ValueSnapshot::Number(number) = self.snapshot(&unboxed)? else {
                    return Err(ValueError::TypeError("Number.toString receiver is not a Number".into()));
                };
                number
            }
            _ => {
                return Err(ValueError::TypeError("Number.toString receiver is not a Number".into()));
            }
        };
        let radix = match args.first() {
            Some(radix) if self.kind(radix)? != ValueKind::Undefined => {
                super::string_methods::integer_or_infinity(self.to_number(radix, runtime)?)
            }
            _ => 10.0,
        };
        if !(2.0..=36.0).contains(&radix) {
            return Err(ValueError::RangeError("Number radix must be between 2 and 36".into()));
        }
        if radix == 10.0 || !number.is_finite() || number == 0.0 {
            return self.string(self.scalar_number_string(number));
        }
        let radix = radix as u32;
        let negative = number < 0.0;
        let number = number.abs();
        let mut text = if radix.is_power_of_two() {
            binary_radix(number, radix)
        } else if number.fract() == 0.0 && number <= 9_007_199_254_740_991.0 {
            let mut n = number as u64;
            let mut digits = Vec::new();
            while n > 0 {
                digits.push(digit((n % u64::from(radix)) as u32));
                n /= u64::from(radix);
            }
            digits.into_iter().rev().collect()
        } else {
            return Err(ValueError::UnsupportedOperation(
                "Fractional nonbinary Number radix formatting is not connected",
            ));
        };
        if negative {
            text.insert(0, '-');
        }
        self.string(text.into())
    }

    fn scalar_number_string(&self, number: f64) -> WireString {
        if number.is_nan() {
            "NaN".into()
        } else if number == f64::INFINITY {
            "Infinity".into()
        } else if number == f64::NEG_INFINITY {
            "-Infinity".into()
        } else {
            WireValue::Number(number).stringify().into()
        }
    }

    pub(super) fn bigint_to_string(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        args: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        let value = match self.snapshot(receiver)? {
            ValueSnapshot::BigInt(value) => value,
            ValueSnapshot::Opaque { kind: OpaqueKind::BoxedBigInt, capability, .. } => {
                let unboxed = self.runtime_value(runtime.resolve(RuntimeRequest::Opaque {
                    capability,
                    target: receiver,
                    receiver,
                    operation: OpaqueOperation::Unbox,
                }))?;
                let ValueSnapshot::BigInt(value) = self.snapshot(&unboxed)? else {
                    return Err(ValueError::TypeError("BigInt.toString receiver is not a BigInt".into()));
                };
                value
            }
            _ => {
                return Err(ValueError::TypeError("BigInt.toString receiver is not a BigInt".into()));
            }
        };
        let radix = match args.first() {
            Some(radix) if self.kind(radix)? != ValueKind::Undefined => {
                super::string_methods::integer_or_infinity(self.to_number(radix, runtime)?)
            }
            _ => 10.0,
        };
        if !(2.0..=36.0).contains(&radix) {
            return Err(ValueError::RangeError("BigInt radix must be between 2 and 36".into()));
        }
        if radix == 10.0 {
            return self.string(value.as_str().into());
        }
        let (negative, decimal) = value.as_str().strip_prefix('-').map_or((false, value.as_str()), |v| (true, v));
        let mut number = decimal.bytes().map(|v| u16::from(v - b'0')).collect::<Vec<_>>();
        let mut result = Vec::new();
        loop {
            let mut carry = 0;
            let mut quotient = Vec::new();
            for digit in number {
                let n = carry * 10 + digit;
                let q = n / radix as u16;
                carry = n % radix as u16;
                if q != 0 || !quotient.is_empty() {
                    quotient.push(q);
                }
            }
            result.push(digit(u32::from(carry)));
            if quotient.is_empty() {
                break;
            }
            number = quotient;
        }
        if negative {
            result.push('-');
        }
        self.string(result.into_iter().rev().collect::<String>().into())
    }
}

fn digit(value: u32) -> char {
    char::from_digit(value, 36).expect("radix digit")
}

fn binary_radix(number: f64, radix: u32) -> String {
    let bits = number.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = (bits & ((1_u64 << 52) - 1)) | if exponent == 0 { 0 } else { 1_u64 << 52 };
    let base_exponent = if exponent == 0 { -1074 } else { exponent - 1023 - 52 };
    let width = radix.trailing_zeros() as i32;
    let high = (63 - mantissa.leading_zeros()) as i32 + base_exponent;
    let low = mantissa.trailing_zeros() as i32 + base_exponent;
    let top = high.max(0).div_euclid(width);
    let bottom = low.min(0).div_euclid(width);
    let mut out = String::new();
    for group in (bottom..=top).rev() {
        if group == -1 {
            out.push('.');
        }
        let mut value = 0;
        for offset in (0..width).rev() {
            let bit = group * width + offset - base_exponent;
            value = (value << 1) | if (0..64).contains(&bit) { ((mantissa >> bit) & 1) as u32 } else { 0 };
        }
        out.push(digit(value));
    }
    while out.ends_with('0') && out.contains('.') {
        out.pop();
    }
    if out.ends_with('.') {
        out.pop();
    }
    out
}

fn add_decimal(left: &str, right: &str) -> String {
    let (ln, l) = left.strip_prefix('-').map_or((false, left), |v| (true, v));
    let (rn, r) = right.strip_prefix('-').map_or((false, right), |v| (true, v));
    if ln == rn {
        let mut result = Vec::new();
        let mut carry = 0;
        let mut l = l.bytes().rev();
        let mut r = r.bytes().rev();
        loop {
            let a = l.next();
            let b = r.next();
            if a.is_none() && b.is_none() && carry == 0 {
                break;
            }
            let n = a.map_or(0, |v| v - b'0') + b.map_or(0, |v| v - b'0') + carry;
            result.push(b'0' + n % 10);
            carry = n / 10;
        }
        if ln {
            result.push(b'-');
        }
        result.reverse();
        String::from_utf8(result).expect("decimal ASCII")
    } else {
        let order = l.len().cmp(&r.len()).then_with(|| l.cmp(r));
        if order.is_eq() {
            return "0".into();
        }
        let (large, small, negative) = if order.is_gt() { (l, r, ln) } else { (r, l, rn) };
        let mut small = small.bytes().rev();
        let mut borrow = 0_i16;
        let mut result = Vec::new();
        for large in large.bytes().rev() {
            let mut n = i16::from(large - b'0') - i16::from(small.next().map_or(0, |v| v - b'0')) - borrow;
            borrow = if n < 0 {
                n += 10;
                1
            } else {
                0
            };
            result.push(b'0' + n as u8);
        }
        while result.len() > 1 && result.last() == Some(&b'0') {
            result.pop();
        }
        if negative {
            result.push(b'-');
        }
        result.reverse();
        String::from_utf8(result).expect("decimal ASCII")
    }
}
