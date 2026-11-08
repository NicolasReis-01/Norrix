//! Text layout and portable glyph recording.

use std::{collections::BTreeMap, fmt::Write as _, sync::Arc};

use anyhow::Result;
use icu_segmenter::GraphemeClusterSegmenter;
use norrix_rasterizer::{
    PreparedGlyph, PreparedGlyphRun, PreparedResource, PreparedScene, PreparedSceneCommand,
    ResourceId,
};
use serde::Serialize;
use skrifa::{
    MetadataProvider,
    instance::Size,
    outline::{DrawSettings, OutlinePen},
};
use specta::Type;
use vello::kurbo::Affine;

use crate::{
    Error, FontStyle, HyphenationPolicy, LayoutRun, RenderBounds, RenderDiagnostic,
    Result as RenderResult, TextAlign, TextLayout, WritingMode,
    bubble::LayoutBox,
    fonts::{Fonts, font_key},
    script::is_chinese_or_japanese_text,
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextNodeDescriptor {
    pub(crate) entity: norrix_scene::EntityId,
    pub(crate) text: String,
    pub(crate) language: Option<norrix_scene::LanguageTag>,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) balloon_contour: Option<Vec<(f32, f32)>>,
    pub(crate) preferred_font: Option<String>,
    pub(crate) font_families: Vec<String>,
    pub(crate) font_weight: Option<u16>,
    pub(crate) font_style: Option<FontStyle>,
    pub(crate) font_size: Option<f32>,
    pub(crate) minimum_font_size: f32,
    pub(crate) auto_fit: bool,
    pub(crate) alignment: TextAlign,
    pub(crate) writing_mode: WritingMode,
    pub(crate) foreground_color: [u8; 4],
    pub(crate) stroke: Option<StrokeOptions>,
    pub(crate) line_height: f32,
    pub(crate) letter_spacing: f32,
    pub(crate) word_spacing: f32,
    pub(crate) text_inset: [f32; 4],
    pub(crate) point_text: bool,
}

pub(crate) struct RenderedTextNode {
    pub(crate) scene: Arc<PreparedScene>,
    pub(crate) resources: Arc<[PreparedResource]>,
    pub(crate) local_bounds: RenderBounds,
    pub(crate) metadata: RenderedTextMetadata,
    pub(crate) diagnostics: Vec<RenderDiagnostic>,
}

pub(crate) struct RenderedTextMetadata {
    pub(crate) rendered_bounds: RenderBounds,
    pub(crate) layout_bounds: RenderBounds,
    pub(crate) post_script_fonts: Vec<String>,
    pub(crate) font_size: f32,
    pub(crate) line_height: f32,
    pub(crate) line_breaks: Vec<usize>,
    pub(crate) path_data: String,
    pub(crate) caret_stops: Vec<TextCaretStop>,
    pub(crate) synthetic_bold_width: f32,
    pub(crate) color: [u8; 4],
    pub(crate) stroke_color: [u8; 4],
    pub(crate) stroke_width: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Type)]
pub struct TextCaretStop {
    pub offset: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StrokeOptions {
    pub color: [u8; 4],
    pub width_px: f32,
}

/// Paint options used when recording one laid-out text run into a Vello scene.
#[derive(Debug, Clone)]
pub(crate) struct TextRenderOptions {
    pub color: [u8; 4],
    pub hint_glyphs: bool,
    pub padding: f32,
    pub baseline_shift: f32,
    pub stroke: Option<StrokeOptions>,
}

impl Default for TextRenderOptions {
    fn default() -> Self {
        Self {
            color: [0, 0, 0, 255],
            hint_glyphs: true,
            padding: 0.0,
            baseline_shift: 0.0,
            stroke: None,
        }
    }
}

/// Shapes text and records the resulting glyphs into vector scenes.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TextRenderer;

impl TextRenderer {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    pub(crate) fn layout<'a>(&self, builder: &TextLayout<'a>, text: &str) -> Result<LayoutRun<'a>> {
        builder.run(text)
    }

    pub(crate) fn render(
        &self,
        scene: &mut PreparedScene,
        resources: &mut Vec<PreparedResource>,
        layout: &LayoutRun<'_>,
        writing_mode: WritingMode,
        options: &TextRenderOptions,
        transform: Affine,
    ) {
        // The border is drawn first as an outward-only dilation of the glyph outline (see
        // `draw_layout`), then the ordinary fill is drawn on top in the foreground color. Unlike a
        // centered stroke, a dilation never grows inward, so it can't punch a hole through small
        // features (e.g. dots) once the border width exceeds their radius.
        if let Some(stroke) = options
            .stroke
            .filter(|stroke| stroke.width_px > 0.0 && stroke.color[3] > 0)
        {
            draw_layout(
                scene,
                resources,
                layout,
                writing_mode,
                options,
                transform,
                GlyphPaint {
                    color: stroke.color,
                    dilation_px: stroke.width_px,
                },
            );
        }
        draw_layout(
            scene,
            resources,
            layout,
            writing_mode,
            options,
            transform,
            GlyphPaint {
                color: options.color,
                dilation_px: 0.0,
            },
        );
    }

    pub(crate) fn render_descriptor(
        &self,
        descriptor: &TextNodeDescriptor,
        fonts: &Fonts,
    ) -> RenderResult<RenderedTextNode> {
        self.layout_descriptor(descriptor, fonts, true)
    }

    pub(crate) fn preview_descriptor(
        &self,
        descriptor: &TextNodeDescriptor,
        fonts: &Fonts,
    ) -> RenderResult<RenderedTextMetadata> {
        self.layout_descriptor(descriptor, fonts, false)
            .map(|rendered| rendered.metadata)
    }

    fn layout_descriptor(
        &self,
        descriptor: &TextNodeDescriptor,
        fonts: &Fonts,
        record_scene: bool,
    ) -> RenderResult<RenderedTextNode> {
        let is_bubble_text = descriptor.balloon_contour.is_some();
        let frame = LayoutBox {
            x: 0.0,
            y: 0.0,
            width: descriptor.width,
            height: descriptor.height,
        };
        let bounds = if is_bubble_text {
            inset(frame, descriptor.text_inset)
        } else {
            frame
        };
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return Err(Error::invalid(format!(
                "text inset leaves no layout area for entity {}",
                descriptor.entity
            )));
        }
        let fonts = fonts
            .resolve(
                descriptor.preferred_font.as_deref(),
                descriptor.font_weight,
                descriptor.font_style,
                &descriptor.font_families,
                &descriptor.text,
                descriptor
                    .language
                    .as_ref()
                    .map(norrix_scene::LanguageTag::as_str),
            )
            .map_err(|source| Error::Font {
                entity: descriptor.entity,
                source,
            })?;
        let maximum = maximum_font_size(descriptor, bounds, is_bubble_text);
        let minimum = descriptor.minimum_font_size.min(maximum);
        let mut layout = TextLayout::new(&fonts[0])
            .with_fallback_fonts(&fonts[1..])
            .with_writing_mode(descriptor.writing_mode)
            .with_alignment(descriptor.alignment)
            .with_line_height(descriptor.line_height)
            .with_spacing(descriptor.letter_spacing, descriptor.word_spacing)
            .with_cjk_punctuation_layout(
                is_chinese_or_japanese_text(&descriptor.text)
                    || descriptor
                        .language
                        .as_ref()
                        .is_some_and(|language| is_chinese_or_japanese_language(language.as_str())),
            );
        if !descriptor.point_text {
            layout = layout
                .with_max_width(bounds.width)
                .with_max_height(bounds.height);
        }
        if let Some(contour) = &descriptor.balloon_contour {
            let [top, _, _, left] = descriptor.text_inset;
            let contour = contour.iter().map(|&(x, y)| (x - left, y - top)).collect();
            layout = layout.with_comic_balloon(
                bounds.width,
                bounds.height,
                contour,
                descriptor.text_inset.into_iter().fold(0.0, f32::max),
            );
        }
        if let Some(language) = &descriptor.language {
            layout = layout.with_hyphenation_language_tag(language.as_str());
        }
        if is_bubble_text && descriptor.writing_mode == WritingMode::Horizontal {
            layout = layout.with_hyphenation_policy(HyphenationPolicy::LastResort);
        }
        let layout = if descriptor.auto_fit && !descriptor.point_text {
            layout
                .with_max_font_size(maximum)
                .with_min_font_size(minimum)
        } else {
            layout.with_font_size(descriptor.font_size.unwrap_or(maximum))
        };
        let layout = self
            .layout(&layout, &descriptor.text)
            .map_err(|source| Error::Layout {
                entity: descriptor.entity,
                source,
            })?;
        let (mut x, mut y) = if descriptor.point_text {
            (bounds.x, bounds.y)
        } else {
            placement(bounds, layout.width, layout.height)
        };
        x += layout.placement_offset_x();
        y += layout.placement_offset_y();
        let transform = Affine::translate((f64::from(x), f64::from(y)));
        let path_data = preview_path_data(&layout, x, y).map_err(|source| Error::Layout {
            entity: descriptor.entity,
            source,
        })?;
        let metrics = fonts[0]
            .skrifa_ref()
            .map_err(|source| Error::Font {
                entity: descriptor.entity,
                source,
            })?
            .metrics(Size::new(layout.font_size), fonts[0].location());
        let metric_height = (metrics.ascent - metrics.descent).max(0.0);
        let caret_height = layout.line_height.max(metric_height);
        let caret_ascent = metrics.ascent + (caret_height - metric_height) * 0.5;
        let caret_stops = preview_caret_stops(
            &layout,
            &descriptor.text,
            x,
            y,
            descriptor.writing_mode,
            caret_ascent,
            caret_height,
        );
        let synthetic_bold_width = layout
            .lines
            .iter()
            .flat_map(|line| &line.glyphs)
            .any(|glyph| glyph.font.synthetic_bold())
            .then_some(2.0)
            .unwrap_or_default();
        let color = descriptor.foreground_color;
        let mut options = TextRenderOptions {
            color,
            stroke: None,
            ..TextRenderOptions::default()
        };
        let mut scene = PreparedScene::default();
        let mut resources = Vec::new();
        if let Some(stroke) = descriptor.stroke {
            options.stroke = Some(stroke);
        }
        if record_scene {
            self.render(
                &mut scene,
                &mut resources,
                &layout,
                descriptor.writing_mode,
                &options,
                transform,
            );
        }
        let mut diagnostics = Vec::new();
        if layout.font_size + f32::EPSILON < descriptor.minimum_font_size {
            diagnostics.push(RenderDiagnostic::TextBelowReadableSize {
                entity: descriptor.entity,
                font_size: layout.font_size,
                minimum_font_size: descriptor.minimum_font_size,
            });
        }
        if layout.overflowed() {
            diagnostics.push(RenderDiagnostic::TextOverflow {
                entity: descriptor.entity,
                available: bounds.into(),
                actual_width: layout.width,
                actual_height: layout.height,
                font_size: layout.font_size,
            });
        }
        let rendered_bounds = RenderBounds {
            x,
            y,
            width: layout.width,
            height: layout.height,
        };
        let stroke_padding = descriptor
            .stroke
            .map_or(0.0, |stroke| stroke.width_px.max(0.0));
        Ok(RenderedTextNode {
            scene: Arc::new(scene),
            resources: resources.into(),
            local_bounds: RenderBounds {
                x: rendered_bounds.x - stroke_padding,
                y: rendered_bounds.y - stroke_padding,
                width: rendered_bounds.width + stroke_padding * 2.0,
                height: rendered_bounds.height + stroke_padding * 2.0,
            },
            metadata: RenderedTextMetadata {
                rendered_bounds,
                layout_bounds: if descriptor.point_text {
                    rendered_bounds
                } else {
                    bounds.into()
                },
                post_script_fonts: fonts
                    .iter()
                    .map(|font| font.post_script_name().to_owned())
                    .collect(),
                font_size: layout.font_size,
                line_height: layout.line_height,
                line_breaks: layout.lines.iter().map(|line| line.range.end).collect(),
                path_data,
                caret_stops,
                synthetic_bold_width,
                color,
                stroke_color: descriptor
                    .stroke
                    .map_or([0, 0, 0, 0], |stroke| stroke.color),
                stroke_width: descriptor.stroke.map_or(0.0, |stroke| stroke.width_px),
            },
            diagnostics,
        })
    }
}

fn preview_path_data(layout: &LayoutRun<'_>, origin_x: f32, origin_y: f32) -> Result<String> {
    let mut path = String::new();
    for line in &layout.lines {
        let (baseline_x, baseline_y) = line.baseline;
        let mut pen_x = 0.0_f32;
        let mut pen_y = 0.0_f32;
        for glyph in &line.glyphs {
            let font = glyph.font.skrifa_ref()?;
            if let Some(outline) = font
                .outline_glyphs()
                .get(skrifa::GlyphId::new(glyph.glyph_id))
            {
                let mut pen = PreviewPathPen {
                    path: &mut path,
                    origin_x: origin_x + baseline_x + pen_x + glyph.x_offset,
                    origin_y: origin_y + baseline_y + pen_y - glyph.y_offset,
                    synthetic_skew: glyph
                        .font
                        .synthetic_skew()
                        .map_or(0.0, |angle| angle.to_radians().tan()),
                };
                outline.draw(
                    DrawSettings::unhinted(Size::new(layout.font_size), glyph.font.location()),
                    &mut pen,
                )?;
            }
            pen_x += glyph.x_advance;
            pen_y -= glyph.y_advance;
        }
    }
    Ok(path)
}

fn preview_caret_stops(
    layout: &LayoutRun<'_>,
    text: &str,
    origin_x: f32,
    origin_y: f32,
    writing_mode: WritingMode,
    caret_ascent: f32,
    caret_height: f32,
) -> Vec<TextCaretStop> {
    let vertical = writing_mode.is_vertical();
    let boundaries = GraphemeClusterSegmenter::new()
        .segment_str(text)
        .collect::<Vec<_>>();
    let utf16_offsets = boundaries
        .iter()
        .map(|&byte| (byte, text[..byte].encode_utf16().count() as u32))
        .collect::<BTreeMap<_, _>>();
    let mut stops = Vec::new();

    for (line_index, line) in layout.lines.iter().enumerate() {
        let (baseline_x, baseline_y) = line.baseline;
        let mut cluster_extents = BTreeMap::<u32, (f32, f32, bool)>::new();
        let mut pen_x = 0.0_f32;
        let mut pen_y = 0.0_f32;
        for glyph in &line.glyphs {
            let advance = if vertical {
                glyph.y_advance
            } else {
                glyph.x_advance
            };
            let start = if vertical { pen_y } else { pen_x };
            let end = start + advance.abs();
            cluster_extents
                .entry(glyph.cluster)
                .and_modify(|extent| {
                    extent.0 = extent.0.min(start);
                    extent.1 = extent.1.max(end);
                })
                .or_insert((start.min(end), start.max(end), glyph.is_rtl));
            pen_x += glyph.x_advance;
            pen_y -= glyph.y_advance;
        }

        if cluster_extents.is_empty() {
            let offset = *utf16_offsets.get(&line.range.start).unwrap_or(&0);
            stops.push(TextCaretStop {
                offset,
                x: origin_x + baseline_x,
                y: origin_y + baseline_y - if vertical { 0.0 } else { caret_ascent },
                width: if vertical { layout.line_height } else { 1.0 },
                height: if vertical {
                    layout.font_size
                } else {
                    caret_height
                },
                line: line_index as u32,
            });
            continue;
        }

        let clusters = cluster_extents.keys().copied().collect::<Vec<_>>();
        for (cluster_index, cluster) in clusters.iter().enumerate() {
            let cluster_start = *cluster as usize;
            let cluster_end = clusters
                .get(cluster_index + 1)
                .map_or(line.range.end, |next| *next as usize)
                .max(cluster_start)
                .min(text.len());
            let Some((left, right, rtl)) = cluster_extents.get(cluster) else {
                continue;
            };
            let logical_start = if *rtl { *right } else { *left };
            let logical_end = if *rtl { *left } else { *right };
            let cluster_boundaries = boundaries
                .iter()
                .copied()
                .filter(|boundary| *boundary >= cluster_start && *boundary <= cluster_end)
                .collect::<Vec<_>>();
            let denominator = cluster_boundaries.len().saturating_sub(1).max(1) as f32;
            for (index, byte_offset) in cluster_boundaries.iter().enumerate() {
                let fraction = index as f32 / denominator;
                let inline = logical_start + (logical_end - logical_start) * fraction;
                stops.push(TextCaretStop {
                    offset: *utf16_offsets.get(byte_offset).unwrap_or(&0),
                    x: origin_x + baseline_x + if vertical { 0.0 } else { inline },
                    y: origin_y + baseline_y + if vertical { inline } else { -caret_ascent },
                    width: if vertical { layout.line_height } else { 1.0 },
                    height: if vertical {
                        layout.font_size
                    } else {
                        caret_height
                    },
                    line: line_index as u32,
                });
            }
        }
    }

    if stops.is_empty() {
        stops.push(TextCaretStop {
            offset: text.encode_utf16().count() as u32,
            x: origin_x,
            y: origin_y,
            width: 1.0,
            height: layout.line_height,
            line: 0,
        });
    }
    stops.sort_by_key(|stop| (stop.line, stop.offset));
    stops.dedup_by(|left, right| {
        left.line == right.line && left.offset == right.offset && (left.x - right.x).abs() < 0.01
    });
    stops
}

struct PreviewPathPen<'a> {
    path: &'a mut String,
    origin_x: f32,
    origin_y: f32,
    synthetic_skew: f32,
}

impl PreviewPathPen<'_> {
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.origin_x + x,
            self.origin_y - y + self.synthetic_skew * x,
        )
    }

    fn write_point(&mut self, command: char, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        let _ = write!(self.path, "{command}{x:.3} {y:.3}");
    }
}

impl OutlinePen for PreviewPathPen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.write_point('M', x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.write_point('L', x, y);
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (cx, cy) = self.point(cx, cy);
        let (x, y) = self.point(x, y);
        let _ = write!(self.path, "Q{cx:.3} {cy:.3} {x:.3} {y:.3}");
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (cx0, cy0) = self.point(cx0, cy0);
        let (cx1, cy1) = self.point(cx1, cy1);
        let (x, y) = self.point(x, y);
        let _ = write!(
            self.path,
            "C{cx0:.3} {cy0:.3} {cx1:.3} {cy1:.3} {x:.3} {y:.3}"
        );
    }

    fn close(&mut self) {
        self.path.push('Z');
    }
}

fn automatic_maximum(
    descriptor: &TextNodeDescriptor,
    bounds: LayoutBox,
    is_bubble_text: bool,
) -> f32 {
    if is_bubble_text {
        if descriptor.writing_mode.is_vertical() {
            bounds.height
        } else {
            bounds.width
        }
    } else {
        24.0
    }
}

fn maximum_font_size(
    descriptor: &TextNodeDescriptor,
    bounds: LayoutBox,
    is_bubble_text: bool,
) -> f32 {
    descriptor
        .font_size
        .filter(|size| size.is_finite() && *size > 0.0)
        .unwrap_or_else(|| automatic_maximum(descriptor, bounds, is_bubble_text))
}

fn is_chinese_or_japanese_language(language: &str) -> bool {
    language
        .split(['-', '_'])
        .next()
        .is_some_and(|primary| matches!(primary.to_ascii_lowercase().as_str(), "ja" | "zh"))
}

fn inset(rect: LayoutBox, [top, right, bottom, left]: [f32; 4]) -> LayoutBox {
    LayoutBox {
        x: rect.x + left,
        y: rect.y + top,
        width: (rect.width - left - right).max(0.0),
        height: (rect.height - top - bottom).max(0.0),
    }
}

fn placement(rect: LayoutBox, width: f32, height: f32) -> (f32, f32) {
    let x = rect.x + (rect.width - width) * 0.5;
    let remaining = rect.height - height;
    let y = rect.y + remaining * 0.5;
    (x, y)
}

/// Color and outward dilation for one glyph draw pass (border or ordinary fill).
struct GlyphPaint {
    color: [u8; 4],
    dilation_px: f32,
}

fn draw_layout(
    scene: &mut PreparedScene,
    resources: &mut Vec<PreparedResource>,
    layout: &LayoutRun<'_>,
    writing_mode: WritingMode,
    options: &TextRenderOptions,
    transform: Affine,
    paint: GlyphPaint,
) {
    for line in &layout.lines {
        let (baseline_x, baseline_y) = match writing_mode {
            WritingMode::Horizontal | WritingMode::VerticalRl => line.baseline,
        };
        let mut pen_x = 0.0;
        let mut pen_y = 0.0;
        let mut start = 0;

        while start < line.glyphs.len() {
            let font = line.glyphs[start].font;
            let key = font_key(font);
            let mut end = start + 1;
            while end < line.glyphs.len() && font_key(line.glyphs[end].font) == key {
                end += 1;
            }

            let mut glyphs = Vec::with_capacity(end - start);
            for glyph in &line.glyphs[start..end] {
                glyphs.push(PreparedGlyph {
                    id: glyph.glyph_id,
                    x: options.padding + baseline_x + pen_x + glyph.x_offset,
                    y: options.padding + baseline_y + pen_y
                        - glyph.y_offset
                        - options.baseline_shift,
                });
                pen_x += glyph.x_advance;
                pen_y -= glyph.y_advance;
            }

            let font_id = ResourceId::for_font(font.bytes());
            if !resources.iter().any(|candidate| candidate.id() == font_id) {
                resources.push(PreparedResource::font_shared(font.shared_bytes()));
            }
            let glyph_transform = font
                .synthetic_skew()
                .map(|angle| Affine::skew(-(angle.to_radians().tan() as f64), 0.0).as_coeffs());
            let synthetic_bold = if font.synthetic_bold() { 1.0 } else { 0.0 };
            scene
                .commands
                .push(PreparedSceneCommand::GlyphRun(PreparedGlyphRun {
                    font: font_id,
                    font_index: font.index(),
                    font_size: layout.font_size,
                    normalized_coords: font.normalized_coords().to_vec(),
                    transform: transform.as_coeffs(),
                    glyph_transform,
                    // Hinting folds a uniform zoom into `font_size` for crisp fill text, but a
                    // border needs the real transform so its dilation amount stays proportional
                    // to that same zoom instead of being rendered at a fixed pixel size.
                    hint: options.hint_glyphs && paint.dilation_px == 0.0,
                    embolden: [synthetic_bold + paint.dilation_px; 2],
                    color: paint.color,
                    glyphs,
                }));
            start = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use norrix_rasterizer::{
        Bounds, CompositionCommand, LayerId, LayerKind as PreparedLayerKind, Point,
        PreparedContent, PreparedFrame, PreparedFrameBundle, PreparedFrameManifest, PreparedLayer,
        PreparedResourcePacket, PreparedResourceStore, PreparedSceneCommand, Presentation,
        Revision,
    };
    use norrix_scene::EntityId;

    use super::*;
    use crate::fonts::FontSystem;

    #[test]
    fn letter_spacing_reaches_prepared_glyph_positions_at_full_range() {
        let font = FontSystem::new().first_font().unwrap();
        let glyph_positions = |letter_spacing: f32| {
            let layout = TextLayout::new(&font)
                .with_font_size(256.0)
                .with_max_width(2_000.0)
                .with_max_height(512.0)
                .with_spacing(letter_spacing, 0.0)
                .run("MMMM")
                .unwrap();
            assert_eq!(layout.lines.len(), 1);
            assert_eq!(layout.lines[0].glyphs.len(), 4);

            let mut scene = PreparedScene::default();
            let mut resources = Vec::new();
            TextRenderer::new().render(
                &mut scene,
                &mut resources,
                &layout,
                WritingMode::Horizontal,
                &TextRenderOptions::default(),
                Affine::IDENTITY,
            );
            scene
                .commands
                .iter()
                .find_map(|command| match command {
                    PreparedSceneCommand::GlyphRun(run) => {
                        Some(run.glyphs.iter().map(|glyph| glyph.x).collect::<Vec<_>>())
                    }
                    PreparedSceneCommand::FillPath(_) => None,
                })
                .unwrap()
        };

        let baseline = glyph_positions(0.0);
        for letter_spacing in [-60.0, -45.0, -30.0, -15.0, 15.0, 30.0, 45.0, 60.0] {
            let positions = glyph_positions(letter_spacing);
            for index in 0..3 {
                let baseline_gap = baseline[index + 1] - baseline[index];
                let actual_gap = positions[index + 1] - positions[index];
                assert!(
                    (actual_gap - (baseline_gap + letter_spacing)).abs() < 0.1,
                    "spacing {letter_spacing} should change rendered glyph gap {index} from \
                     {baseline_gap} to {}, got {actual_gap}",
                    baseline_gap + letter_spacing
                );
            }
        }
    }

    #[test]
    fn automatic_size_preserves_free_text_default_and_balloon_extent() {
        let mut descriptor = TextNodeDescriptor {
            entity: EntityId::new(),
            text: "Hi".to_owned(),
            language: None,
            width: 240.0,
            height: 120.0,
            balloon_contour: None,
            preferred_font: Some("Arial".to_owned()),
            font_families: vec!["Arial".to_owned()],
            font_weight: None,
            font_style: None,
            font_size: Some(6.0),
            minimum_font_size: 9.0,
            auto_fit: true,
            alignment: TextAlign::Center,
            writing_mode: WritingMode::Horizontal,
            foreground_color: [0, 0, 0, 255],
            stroke: None,
            line_height: 1.2,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            text_inset: [0.0; 4],
            point_text: false,
        };
        let bounds = LayoutBox {
            x: 0.0,
            y: 0.0,
            width: 240.0,
            height: 120.0,
        };

        assert_eq!(automatic_maximum(&descriptor, bounds, false), 24.0);
        assert_eq!(automatic_maximum(&descriptor, bounds, true), 240.0);
        assert_eq!(maximum_font_size(&descriptor, bounds, false), 6.0);
        descriptor.font_size = Some(120.0);
        assert_eq!(maximum_font_size(&descriptor, bounds, false), 120.0);
    }

    #[test]
    fn rendered_text_survives_prepared_packet_round_trip() {
        let font = FontSystem::new().first_font().unwrap();
        let layout = TextLayout::new(&font)
            .with_font_size(24.0)
            .run("Norrix")
            .unwrap();
        let mut scene = PreparedScene::default();
        let mut resources = Vec::new();
        TextRenderer::new().render(
            &mut scene,
            &mut resources,
            &layout,
            WritingMode::Horizontal,
            &TextRenderOptions::default(),
            Affine::IDENTITY,
        );
        assert_eq!(resources.len(), 1);
        assert!(matches!(
            &resources[0],
            PreparedResource::Font { bytes, .. } if !bytes.is_empty()
        ));

        let layer = LayerId::from_bytes([2; 16]);
        let bundle = PreparedFrameBundle {
            frame: PreparedFrame {
                revision: Revision::new(7),
                page: LayerId::from_bytes([1; 16]),
                width: 160,
                height: 48,
                origin: (0, 0),
                normalization: Affine::IDENTITY.as_coeffs(),
                layers: vec![PreparedLayer {
                    id: layer,
                    geometry: vec![
                        Point { x: 0.0, y: 0.0 },
                        Point { x: 160.0, y: 0.0 },
                        Point { x: 160.0, y: 48.0 },
                        Point { x: 0.0, y: 48.0 },
                    ],
                    bounds: Bounds {
                        x: 0.0,
                        y: 0.0,
                        width: 160.0,
                        height: 48.0,
                    },
                    local_bounds: Bounds {
                        x: 0.0,
                        y: 0.0,
                        width: 160.0,
                        height: 48.0,
                    },
                    presentation: Presentation {
                        visible: true,
                        opacity: 1.0,
                    },
                    kind: PreparedLayerKind::Text,
                    placement: Affine::IDENTITY.as_coeffs(),
                    content: PreparedContent::Vector(scene),
                    element_frame: None,
                    clip: None,
                }],
            },
            resources,
        };

        let encoded = bundle.manifest().unwrap().encode().unwrap();
        let manifest = PreparedFrameManifest::decode(&encoded).unwrap();
        let mut resources = PreparedResourceStore::default();
        for reference in manifest.required_resources() {
            let encoded = bundle
                .resource_packet(reference.id)
                .unwrap()
                .encode()
                .unwrap();
            resources.insert(PreparedResourcePacket::decode(&encoded).unwrap());
        }
        let compiled = manifest.compile(&resources).unwrap();
        assert_eq!(compiled.revision(), Revision::new(7));
        assert_eq!(compiled.layers()[0].id(), layer);
        assert!(matches!(
            compiled.composition_commands(1).as_slice(),
            [CompositionCommand::Vector(_)]
        ));
    }
}
