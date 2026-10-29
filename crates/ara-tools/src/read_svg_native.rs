// MIT License
//
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
// Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d; native-only ARA adaptation.

//! Safe SVG rasterization for terminal image previews.
//!
//! SVGs are parsed without loading file-backed image resources, capped to a
//! caller-provided canvas, and encoded as PNG for terminal graphics protocols.

use std::sync::{Arc, LazyLock};

use ara_agent::ToolError;
type Result<T> = std::result::Result<T, ToolError>;
use resvg::{tiny_skia, usvg};


const MAX_RENDER_PIXELS: u64 = 16 * 1024 * 1024;

static FONT_DB: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
	let mut database = usvg::fontdb::Database::new();
	database.load_system_fonts();
	Arc::new(database)
});

/// Rasterize immutable SVG/SVGZ bytes using the fixed native algorithm.
/// The async READ owner invokes this on its existing blocking worker.
pub fn rasterize_svg(input: &[u8], max_width_px: u32, max_height_px: u32) -> Result<Vec<u8>> {
	rasterize_svg_sync(input, max_width_px, max_height_px)
}

fn rasterize_svg_sync(input: &[u8], max_width_px: u32, max_height_px: u32) -> Result<Vec<u8>> {
	if max_width_px == 0 || max_height_px == 0 {
		return Err(ToolError("SVG render limits must be greater than zero".into()));
	}
	if u64::from(max_width_px) * u64::from(max_height_px) > MAX_RENDER_PIXELS {
		return Err(ToolError(format!(
			"SVG render limits exceed the {MAX_RENDER_PIXELS}-pixel safety cap"
		)));
	}

	let mut options = usvg::Options { fontdb: Arc::clone(&FONT_DB), ..usvg::Options::default() };
	// Repository-controlled SVGs must not read arbitrary host files through an
	// <image href="…"> reference. Embedded data URLs retain the default resolver.
	options.image_href_resolver.resolve_string = Box::new(|_, _| None);
	let tree = usvg::Tree::from_data(input, &options)
		.map_err(|error| ToolError(format!("Failed to parse SVG: {error}")))?;
	let source = tree.size();
	let scale = (max_width_px as f32 / source.width())
		.min(max_height_px as f32 / source.height())
		.min(1.0);
	let width = (source.width() * scale).ceil().max(1.0) as u32;
	let height = (source.height() * scale).ceil().max(1.0) as u32;
	let mut pixmap = tiny_skia::Pixmap::new(width, height)
		.ok_or_else(|| ToolError("Failed to allocate SVG render surface".into()))?;
	resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
	pixmap
		.encode_png()
		.map_err(|error| ToolError(format!("Failed to encode SVG preview: {error}")))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn rasterizes_svg_at_intrinsic_size() {
		let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="7"><rect width="12" height="7" fill="red"/></svg>"#;
		let png = rasterize_svg_sync(svg, 100, 100).expect("SVG should rasterize");
		let image = image::load_from_memory(&png).expect("result should be PNG");
		assert_eq!((image.width(), image.height()), (12, 7));
	}

	#[test]
	fn rejects_unbounded_render_surface() {
		let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#;
		let error = rasterize_svg_sync(svg, 8192, 8192).expect_err("oversized canvas should fail");
		assert!(error.0.contains("safety cap"));
	}
}
