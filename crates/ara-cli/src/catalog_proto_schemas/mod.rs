//! Complete fixed Cursor/Devin public schema facade.
// Fixed upstream enum namespaces retain their public PascalCase export names.
#![allow(non_snake_case)]
pub mod cursor;
pub mod devin;
pub const IR: &str = include_str!("ir.json");
