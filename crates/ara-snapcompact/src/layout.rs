//! Fixed OMP wide-cell/grid/doc pagination and bounded local rendering.
use crate::{
    DIM_OFF, DIM_ON, Geometry, HistoryBlock, NormalizeOptions, RenderManyOptions, RenderedFrame, Result, Shape,
};

pub(crate) fn is_wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x115f | 0x2e80..=0x2eff | 0x2f00..=0x2fdf | 0x3000..=0x303e | 0x3041..=0x33ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xa000..=0xa4cf | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe30..=0xfe4f | 0xff00..=0xff60 | 0xffe0..=0xffe6 | 0x20000..=0x2fffd | 0x30000..=0x3fffd)
}
fn char_cells(c: char, wide: bool) -> usize {
    if c == DIM_ON || c == DIM_OFF {
        0
    } else if wide && is_wide(c) {
        2
    } else {
        1
    }
}
fn cell_length(text: &str, wide: bool) -> usize {
    text.chars().map(|c| char_cells(c, wide)).sum()
}
fn slice_cells(text: &str, width: usize, wide: bool) -> usize {
    let mut cells = 0;
    let mut placed = false;
    let mut end = 0;
    for (at, c) in text.char_indices() {
        let count = char_cells(c, wide);
        if placed && cells + count > width {
            break;
        }
        cells += count;
        end = at + c.len_utf8();
        placed |= count > 0;
    }
    end
}
pub(crate) fn paginate_cells(text: &str, capacity: usize, cols: usize, wide: bool) -> Vec<String> {
    let mut pages = Vec::new();
    let mut start = 0;
    let mut cell = 0;
    let mut has_cell = false;
    for (at, c) in text.char_indices() {
        let count = char_cells(c, wide);
        if count == 0 {
            continue;
        }
        let mut target = cell;
        if count == 2 && cols >= 2 && target % cols == cols - 1 {
            target += 1;
        }
        if has_cell && target + count > capacity {
            pages.push(text[start..at].into());
            start = at;
            target = 0;
        }
        cell = target + count;
        has_cell = true;
    }
    if has_cell {
        pages.push(text[start..].into());
    }
    pages
}
pub fn wrap(text: &str, width: usize, wide: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_cells = 0;
    for token in text.split(ara_prompt::js::is_space).filter(|s| !s.is_empty()) {
        let mut word = token;
        let mut word_cells = cell_length(word, wide);
        while word_cells > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_cells = 0;
            }
            let end = slice_cells(word, width, wide);
            if end == 0 {
                break;
            }
            lines.push(word[..end].into());
            word = &word[end..];
            word_cells = cell_length(word, wide);
        }
        if current.is_empty() {
            current = word.into();
            current_cells = word_cells;
        } else if current_cells + 1 + word_cells <= width {
            current.push(' ');
            current.push_str(word);
            current_cells += 1 + word_cells;
        } else {
            lines.push(std::mem::replace(&mut current, word.into()));
            current_cells = word_cells;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}
pub(crate) fn doc_pages(text: &str, geo: Geometry, wide: bool) -> Vec<String> {
    let per_page = 2 * geo.rows;
    if per_page == 0 {
        return Vec::new();
    }
    wrap(text, geo.cols, wide).chunks(per_page).map(|lines| lines.join("\n")).collect()
}
pub fn geometry(shape: &Shape, size: Option<u32>) -> Geometry {
    let size = size.map(f64::from).unwrap_or(shape.frame_size);
    let grid_cols = (size / shape.cell_width).floor() as usize;
    let rows = (size / shape.cell_height / shape.line_repeat).floor() as usize;
    if shape.columns == Some(2) {
        let cols = grid_cols.saturating_sub(3) / 2;
        Geometry { cols, rows, capacity: 2 * cols * rows }
    } else {
        Geometry { cols: grid_cols, rows, capacity: grid_cols * rows }
    }
}
fn rendered_chars(text: &str, shape: &Shape, geo: Geometry) -> usize {
    if shape.columns == Some(2) {
        return text.chars().filter(|&c| c != DIM_ON && c != DIM_OFF && c != '\n').count().min(geo.capacity);
    }
    let mut cell = 0;
    let mut count = 0;
    for c in text.chars() {
        let width = char_cells(c, shape.font != "silver");
        if width == 0 {
            continue;
        }
        let mut target = cell;
        if width == 2 && geo.cols >= 2 && target % geo.cols == geo.cols - 1 {
            target += 1;
        }
        if target + width > geo.capacity {
            break;
        }
        cell = target + width;
        count += 1;
    }
    count
}
pub fn render(text: &str, shape: &Shape, size: Option<u32>) -> Result<RenderedFrame> {
    let geo = geometry(shape, size);
    let data = crate::native::render_snapcompact_png(
        text.into(),
        crate::native::SnapcompactRenderOptions {
            size: size.unwrap_or(shape.frame_size as u32),
            font: Some(shape.font.clone()),
            cell_width: Some(shape.cell_width as u32),
            cell_height: Some(shape.cell_height as u32),
            variant: Some(shape.variant.clone()),
            line_repeat: Some(shape.line_repeat as u32),
            stretch: shape.stretch,
            columns: shape.columns,
        },
    )?;
    Ok(RenderedFrame { data, cols: geo.cols, rows: geo.rows, chars: rendered_chars(text, shape, geo) })
}
pub(crate) fn dim_open(text: &str) -> bool {
    text.rfind(DIM_ON) > text.rfind(DIM_OFF)
}
pub(crate) fn render_pages(pages: &[(String, Shape)], size: Option<u32>) -> Result<Vec<RenderedFrame>> {
    // Intentional Host resource limit: at most two concurrent frame renders.
    // Order and error propagation retain the fixed Promise.all result contract.
    let mut frames = Vec::with_capacity(pages.len());
    for pair in pages.chunks(2) {
        let rendered = std::thread::scope(|scope| {
            let jobs: Vec<_> =
                pair.iter().map(|(text, shape)| scope.spawn(move || render(text, shape, size))).collect();
            jobs.into_iter()
                .map(|job| job.join().map_err(|_| crate::Error::from_reason("snapcompact renderer panicked"))?)
                .collect::<Result<Vec<_>>>()
        })?;
        frames.extend(rendered);
    }
    Ok(frames)
}
pub fn render_many(text: &str, options: RenderManyOptions) -> Result<Vec<HistoryBlock>> {
    let shape = match options.shape {
        Some(s) => s,
        None => crate::resolve_shape_for_text(text, options.model.as_ref(), None)?,
    };
    let normalized = crate::normalize(text, NormalizeOptions { shape: Some(shape.clone()), ..Default::default() });
    let geo = geometry(&shape, options.frame_size);
    let pages = if shape.columns == Some(2) {
        doc_pages(&normalized, geo, shape.font != "silver")
    } else {
        paginate_cells(&normalized, geo.capacity, geo.cols, shape.font != "silver")
    };
    let mut dim = false;
    let mut planned = Vec::new();
    for page in pages {
        if options.max_frames.is_some_and(|cap| planned.len() as f64 >= cap) {
            break;
        }
        let mut page = if shape.columns == Some(2) && dim { format!("{DIM_ON}{page}") } else { page };
        if shape.columns == Some(2) {
            dim = dim_open(&page);
        }
        if shape.stopword_dim == Some(true) {
            page = crate::dim_stopwords(&page);
        }
        planned.push((page, shape.clone()));
    }
    Ok(render_pages(&planned, options.frame_size)?
        .into_iter()
        .map(|frame| HistoryBlock::Image {
            data: frame.data,
            mime_type: "image/png".into(),
            detail: shape.image_detail.as_ref().map(|s| serde_json::Value::String(s.clone())),
        })
        .collect())
}
pub fn frames(text: &str, options: RenderManyOptions) -> usize {
    let shape = match options.shape {
        Some(s) => s,
        None => match crate::resolve_shape_for_text(text, options.model.as_ref(), None) {
            Ok(s) => s,
            Err(_) => return 0,
        },
    };
    let geo = geometry(&shape, options.frame_size);
    let normalized = crate::normalize(text, NormalizeOptions { shape: Some(shape.clone()), ..Default::default() });
    if shape.columns == Some(2) {
        doc_pages(&normalized, geo, shape.font != "silver").len()
    } else {
        paginate_cells(&normalized, geo.capacity, geo.cols, shape.font != "silver").len()
    }
}
