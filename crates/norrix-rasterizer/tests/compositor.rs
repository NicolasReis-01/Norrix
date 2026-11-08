#![cfg(feature = "native")]

use std::{collections::HashMap, sync::Arc};

use norrix_rasterizer::{
    Bounds, FillRule, LayerId, LayerKind, PathElement, Point, PreparedContent, PreparedFrame,
    PreparedFrameBundle, PreparedLayer, PreparedPath, PreparedRaster, PreparedRasterTile,
    PreparedResource, PreparedScene, PreparedSceneCommand, Presentation, RasterOptions, Rasterizer,
    Revision,
};

#[test]
fn linear_filtering_does_not_add_borders_to_any_raster_layer() {
    let background =
        PreparedResource::encoded_raster(3, 1, "image/png", Arc::from(&b"background"[..])).unwrap();
    let background_id = background.id();
    let resource =
        PreparedResource::encoded_raster(2, 1, "image/png", Arc::from(&b"foreground"[..])).unwrap();
    let resource_id = resource.id();
    let frame = PreparedFrameBundle {
        frame: PreparedFrame {
            revision: Revision::new(1),
            page: LayerId::from_bytes([1; 16]),
            width: 3,
            height: 1,
            origin: (0, 0),
            normalization: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            layers: vec![
                PreparedLayer {
                    id: LayerId::from_bytes([2; 16]),
                    geometry: vec![
                        Point { x: 0.0, y: 0.0 },
                        Point { x: 3.0, y: 0.0 },
                        Point { x: 3.0, y: 1.0 },
                        Point { x: 0.0, y: 1.0 },
                    ],
                    bounds: Bounds {
                        x: 0.0,
                        y: 0.0,
                        width: 3.0,
                        height: 1.0,
                    },
                    local_bounds: Bounds {
                        x: 0.0,
                        y: 0.0,
                        width: 3.0,
                        height: 1.0,
                    },
                    presentation: Presentation {
                        visible: true,
                        opacity: 1.0,
                    },
                    kind: LayerKind::Raster,
                    placement: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                    content: PreparedContent::Raster(PreparedRaster {
                        source: background_id,
                        width: 3,
                        height: 1,
                        tiles: vec![PreparedRasterTile {
                            x: 0,
                            y: 0,
                            width: 3,
                            height: 1,
                            gutter: [0; 4],
                        }],
                    }),
                    element_frame: None,
                    clip: None,
                },
                PreparedLayer {
                    id: LayerId::from_bytes([3; 16]),
                    geometry: vec![
                        Point { x: 0.5, y: 0.0 },
                        Point { x: 2.5, y: 0.0 },
                        Point { x: 2.5, y: 1.0 },
                        Point { x: 0.5, y: 1.0 },
                    ],
                    bounds: Bounds {
                        x: 0.5,
                        y: 0.0,
                        width: 2.0,
                        height: 1.0,
                    },
                    local_bounds: Bounds {
                        x: 0.0,
                        y: 0.0,
                        width: 2.0,
                        height: 1.0,
                    },
                    presentation: Presentation {
                        visible: true,
                        opacity: 1.0,
                    },
                    kind: LayerKind::Raster,
                    placement: [1.0, 0.0, 0.0, 1.0, 0.5, 0.0],
                    content: PreparedContent::Raster(PreparedRaster {
                        source: resource_id,
                        width: 2,
                        height: 1,
                        tiles: vec![PreparedRasterTile {
                            x: 0,
                            y: 0,
                            width: 2,
                            height: 1,
                            gutter: [0; 4],
                        }],
                    }),
                    element_frame: None,
                    clip: None,
                },
            ],
        },
        resources: vec![background, resource],
    }
    .into_frame_with_raster_sources(&HashMap::from([
        (background_id, Arc::from(&[255; 12][..])),
        (resource_id, Arc::from(&[255, 0, 0, 255, 0, 0, 0, 0][..])),
    ]))
    .unwrap();

    let raster = Rasterizer::new()
        .unwrap()
        .rasterize(&frame, RasterOptions::default())
        .unwrap();

    let edge = raster.image.get_pixel(1, 0).0;
    assert_eq!(edge[0], u8::MAX);
    assert!((126..=129).contains(&edge[1]));
    assert!((126..=129).contains(&edge[2]));
    assert_eq!(edge[3], u8::MAX);
}

#[test]
fn ghost_text_is_translated_clipped_to_the_overflowing_region_and_keeps_full_opacity() {
    let source = PreparedFrameBundle {
        frame: PreparedFrame {
            revision: Revision::new(1),
            page: LayerId::from_bytes([5; 16]),
            width: 10,
            height: 10,
            origin: (0, 0),
            normalization: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            layers: vec![PreparedLayer {
                id: LayerId::from_bytes([6; 16]),
                geometry: vec![
                    Point { x: 1.0, y: -6.0 },
                    Point { x: 9.0, y: -6.0 },
                    Point { x: 9.0, y: 2.0 },
                    Point { x: 1.0, y: 2.0 },
                ],
                bounds: Bounds {
                    x: 1.0,
                    y: -6.0,
                    width: 8.0,
                    height: 8.0,
                },
                local_bounds: Bounds {
                    x: 1.0,
                    y: -6.0,
                    width: 8.0,
                    height: 8.0,
                },
                presentation: Presentation {
                    visible: true,
                    opacity: 1.0,
                },
                kind: LayerKind::Text,
                placement: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                content: PreparedContent::Vector(PreparedScene {
                    commands: vec![PreparedSceneCommand::FillPath(PreparedPath {
                        fill: FillRule::NonZero,
                        transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                        color: [255, 0, 0, 255],
                        elements: vec![
                            PathElement::MoveTo([1.0, -6.0]),
                            PathElement::LineTo([9.0, -6.0]),
                            PathElement::LineTo([9.0, 2.0]),
                            PathElement::LineTo([1.0, 2.0]),
                            PathElement::Close,
                        ],
                    })],
                }),
                element_frame: None,
                clip: None,
            }],
        },
        resources: Vec::new(),
    };

    let image = render_ghost(&source, (0.0, 10.0));

    assert_eq!(image.get_pixel(4, 5).0, [255, 0, 0, 255]);
    assert_eq!(image.get_pixel(4, 1).0[3], 0);
    assert_eq!(image.get_pixel(4, 3).0[3], 0);
    assert_eq!(image.get_pixel(4, 9).0, [255, 0, 0, 255]);

    let mut inside = source.clone();
    inside.frame.layers[0].placement[5] = 6.0;
    assert!(
        render_ghost(&inside, (0.0, 10.0))
            .pixels()
            .all(|pixel| pixel.0[3] == 0)
    );

    let mut lower_overflow = source.clone();
    lower_overflow.frame.layers[0].placement[5] = 14.0;
    let image = render_ghost(&lower_overflow, (0.0, -10.0));
    assert_eq!(image.get_pixel(4, 1).0, [255, 0, 0, 255]);
    assert_eq!(image.get_pixel(4, 8).0[3], 0);

    let mut multi_page_overflow = source.clone();
    multi_page_overflow.frame.layers[0].placement[5] = -18.0;
    let image = render_ghost(&multi_page_overflow, (0.0, 20.0));
    assert_eq!(image.get_pixel(4, 1).0, [255, 0, 0, 255]);
    assert_eq!(image.get_pixel(4, 6).0[3], 0);

    let mut rotated = source;
    rotated.frame.layers[0].placement = [
        std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
        -std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
        0.0,
        0.0,
    ];
    let image = render_ghost(&rotated, (0.0, 10.0));
    assert_eq!(image.get_pixel(5, 8).0, [255, 0, 0, 255]);
    assert_eq!(image.get_pixel(5, 1).0[3], 0);
}

fn render_ghost(source: &PreparedFrameBundle, offset: (f64, f64)) -> image::RgbaImage {
    let mut target = PreparedFrameBundle {
        frame: PreparedFrame {
            revision: Revision::new(1),
            page: LayerId::from_bytes([4; 16]),
            width: 10,
            height: 10,
            origin: (0, 0),
            normalization: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            layers: Vec::new(),
        },
        resources: Vec::new(),
    };
    target.append_ghost_text(source, offset).unwrap();
    let frame = target
        .into_frame_with_raster_sources(&HashMap::new())
        .unwrap();
    Rasterizer::new()
        .unwrap()
        .rasterize(&frame, RasterOptions::default())
        .unwrap()
        .image
}
