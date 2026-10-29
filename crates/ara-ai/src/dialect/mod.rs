//! Fixed OMP dialect source candidates. Final parsing and the Harmony/generic/
//! fenced/strip scanners have UTF-16 kernels. Actual shared carrier/consumers,
//! actual owner binding and the remaining dialects remain required work.

pub mod actual_projection;
pub mod catalog;
pub mod coercion;
pub mod deepseek;
pub mod fenced_thinking;
pub mod fenced_thinking_units;
pub mod harmony;
pub mod harmony_units;
pub mod hermes;
pub mod history;
pub mod kimi;
pub mod owned_stream;
pub mod qwen3;
pub mod rendering;
pub mod thinking;
pub mod thinking_fence_strip;
pub mod thinking_fence_strip_units;
pub mod thinking_units;
pub mod types;
mod units;

pub use deepseek::{DeepSeekInbandScanner, DeepSeekInbandUnitsScanner, DeepSeekRenderer};
pub use harmony::{HarmonyInbandScanner, HarmonyRenderer};
pub use harmony_units::HarmonyInbandUnitsScanner;
pub use hermes::{HermesInbandScanner, HermesInbandUnitsScanner, HermesRenderer};
pub use history::encode_inband_tool_history;
pub use kimi::{KimiInbandScanner, KimiInbandUnitsScanner};
pub use qwen3::{Qwen3InbandScanner, Qwen3InbandUnitsScanner, Qwen3Renderer};
pub use thinking::{ThinkingInbandScanner, ThinkingInbandScannerOptions};
pub use thinking_fence_strip::ThinkingFenceStripper;
pub use thinking_fence_strip_units::ThinkingFenceUnitsStripper;
pub use thinking_units::ThinkingInbandUnitsScanner;
pub use types::{
    ActualInbandScanEvent, Dialect, DialectRenderOptions, DialectRenderer, DialectToolResult, InbandNameValue,
    InbandScanEvent, InbandScanner, InbandScannerUnits, ToolArgumentJson,
};

#[cfg(test)]
mod tests;
