use std::collections::HashMap;

use anyhow::Result;
use geo::{Coord, LineString, Polygon};
use nalgebra::{Matrix2, Rotation2, Vector2};
use ordered_float::NotNan;
use svg_composer::element::path::{
    command::{Arc as SvgArc, CoordinateType, LineTo, LineToOption, MoveTo},
    Command,
};

use crate::parsing::gerber::Polarity;

pub struct ShapeConfiguration<'a> {
    pub transform: Matrix2<f64>,
    pub shapes: &'a mut Vec<Shape>,
    pub polarity: Polarity,
}

#[derive(Debug)]
pub struct Shape {
    pub polarity: Polarity,
    pub starting_point: Vector2<f64>,
    pub segments: Vec<Segment>,
}

impl Shape {
    pub fn debug_render(&self, path: &mut Vec<Box<dyn Command>>) -> Result<()> {
        path.push(Box::new(MoveTo {
            point: (self.starting_point.x, self.starting_point.y),
            coordinate_type: CoordinateType::Absolute,
        }));

        let mut previous_end = None;

        for segment in self.segments.iter() {
            path.push(segment.debug_render(previous_end.unwrap_or(self.starting_point)));
            previous_end = Some(segment.end());
        }

        Ok(())
    }

    pub fn calculate_bounds(&self) -> (f64, f64, f64, f64) {
        let mut min_x = self.starting_point.x;
        let mut min_y = self.starting_point.y;
        let mut max_x = self.starting_point.x;
        let mut max_y = self.starting_point.y;

        for segment in self.segments.iter() {
            let (local_min_x, local_min_y, local_max_x, local_max_y) = segment.calculate_bounds();
            min_x = min_x.min(local_min_x);
            min_y = min_y.min(local_min_y);
            max_x = max_x.max(local_max_x);
            max_y = max_y.max(local_max_y);
        }

        (min_x, min_y, max_x, max_y)
    }

    pub fn convert_to_geo_line_string(&self, distance_per_step: f64) -> LineString<f64> {
        let mut points = Vec::new();

        let mut start_point = self.starting_point;
        points.push(Coord {
            x: start_point.x,
            y: start_point.y,
        });
        for segment in self.segments.iter() {
            segment.append_to_line_string(distance_per_step, start_point, &mut points);
            start_point = segment.end();
        }

        LineString(points)
    }

    pub fn convert_to_geo_polygon(&self, distance_per_step: f64) -> Polygon<f64> {
        // Start by separating the internal holes from the outer shape.
        #[derive(Debug, Eq, PartialEq, Hash, Clone, Copy)]
        enum SegmentInfo {
            Line,
            Clockwise { diameter: NotNan<f64> },
            CounterClockwise { diameter: NotNan<f64> },
        }
        impl SegmentInfo {
            fn inverse(&self) -> SegmentInfo {
                match self {
                    SegmentInfo::Line => SegmentInfo::Line,
                    SegmentInfo::Clockwise { diameter } => SegmentInfo::CounterClockwise {
                        diameter: *diameter,
                    },
                    SegmentInfo::CounterClockwise { diameter } => SegmentInfo::Clockwise {
                        diameter: *diameter,
                    },
                }
            }
        }
        let mut repeatable_segments = HashMap::new();
        let mut starting_point = self.starting_point;
        let mut collected_segments = Vec::new();
        let mut shapes = Vec::new();

        fn separator_function(
            polarity: Polarity,
            starting_point: &mut Vector2<f64>,
            repeatable_segments: &mut HashMap<(NotNan<f64>, NotNan<f64>, SegmentInfo), usize>,
            collected_segments: &mut Vec<Segment>,
            shapes: &mut Vec<Shape>,
            end: Vector2<f64>,
            segment_info: SegmentInfo,
        ) {
            let x = NotNan::new(starting_point.x).expect("Got NAN");
            let y = NotNan::new(starting_point.y).expect("Got NAN");

            if let Some(starting_index) =
                repeatable_segments.remove(&(x, y, segment_info.inverse()))
            {
                // There *must* be a point before us for this to work, so it shouldn't panic.
                // It would crash with a subtraction underflow if it somehow matched on the first vertex, which shouldn't be possible.
                let starting_point = match collected_segments[starting_index - 1] {
                    Segment::Line { end } => end,
                    Segment::ClockwiseCurve { end, center: _ } => end,
                    Segment::CounterClockwiseCurve { end, center: _ } => end,
                };

                let segments: Vec<_> = collected_segments.drain(starting_index..).collect();

                // The segments connecting us to the outer polygon are left behind and need to be ignored.
                if segments.len() > 2 {
                    shapes.push(Shape {
                        polarity: polarity.inverse(),
                        starting_point,
                        segments,
                    });
                }
            }
            repeatable_segments.insert((x, y, segment_info), collected_segments.len());

            *starting_point = end;
        }

        for segment in self.segments.iter() {
            match segment {
                Segment::Line { end } => {
                    // merger_function(*end, SegmentInfo::Line);
                    separator_function(
                        self.polarity,
                        &mut starting_point,
                        &mut repeatable_segments,
                        &mut collected_segments,
                        &mut shapes,
                        *end,
                        SegmentInfo::Line,
                    );
                }
                Segment::ClockwiseCurve { end, center } => {
                    let diameter = (center - end).norm();

                    separator_function(
                        self.polarity,
                        &mut starting_point,
                        &mut repeatable_segments,
                        &mut collected_segments,
                        &mut shapes,
                        *end,
                        SegmentInfo::Clockwise {
                            diameter: NotNan::new(diameter).expect("Got NAN"),
                        },
                    );
                }
                Segment::CounterClockwiseCurve { end, center } => {
                    let diameter = (center - end).norm();

                    separator_function(
                        self.polarity,
                        &mut starting_point,
                        &mut repeatable_segments,
                        &mut collected_segments,
                        &mut shapes,
                        *end,
                        SegmentInfo::CounterClockwise {
                            diameter: NotNan::new(diameter).expect("Got NAN"),
                        },
                    );
                }
            }
            collected_segments.push(segment.clone());
        }

        // The last shape should always be the outer line string.
        shapes.push(Shape {
            polarity: self.polarity,
            starting_point: self.starting_point,
            segments: collected_segments,
        });

        // Remove redundant vertices.
        for shape in shapes.iter_mut() {
            let starting_point = shape.starting_point;
            let segments =
                shape
                    .segments
                    .drain(..)
                    .fold(Vec::new(), |mut segments, next_segment| {
                        let first_line_starting_point = segments
                            .len()
                            .checked_sub(2)
                            .and_then(|index| segments.get(index))
                            .map(|segment: &Segment| segment.end())
                            .unwrap_or(starting_point);

                        if let Some(Segment::Line { end }) = segments.last_mut() {
                            if let Segment::Line { end: next_end } = &next_segment {
                                let first_line_direction =
                                    (*end - first_line_starting_point).normalize();
                                let second_line_direction = (*next_end - *end).normalize();

                                let dot_product = first_line_direction.dot(&second_line_direction);

                                if dot_product == 1.0 {
                                    // Hey, that's just an extension of the previous line!
                                    // We'll modify it to point to the new ending.
                                    *end = *next_end;
                                } else {
                                    // Different line. Just add it to the list.
                                    segments.push(next_segment);
                                }
                            } else {
                                // Different line. Just add it to the list.
                                segments.push(next_segment);
                            }
                        } else {
                            // TODO merge matching arcs too.
                            segments.push(next_segment);
                        }

                        segments
                    });

            shape.segments = segments;
        }

        let outer_shape = shapes.pop().unwrap();

        Polygon::new(
            outer_shape.convert_to_geo_line_string(distance_per_step),
            shapes
                .drain(..)
                .map(|shape| shape.convert_to_geo_line_string(distance_per_step))
                .collect(),
        )
    }

    pub fn line(
        shape_configuration: ShapeConfiguration,
        diameter: f64,
        start: Vector2<f64>,
        end: Vector2<f64>,
    ) {
        let start = shape_configuration.transform * start;
        let end = shape_configuration.transform * end;

        let radius = shape_configuration.transform[0] * diameter / 2.0;
        if let Some(direction) = (start - end).try_normalize(0.0) {
            let perpendicular = {
                let mut perpendicular = direction;
                perpendicular.swap_rows(0, 1);
                perpendicular.x *= -1.0;
                perpendicular
            } * radius;

            let starting_point = start + perpendicular;

            let segments = vec![
                Segment::ClockwiseCurve {
                    end: start - perpendicular,
                    center: start,
                },
                Segment::Line {
                    end: end - perpendicular,
                },
                Segment::ClockwiseCurve {
                    end: end + perpendicular,
                    center: end,
                },
                Segment::Line {
                    end: start + perpendicular,
                },
            ];

            shape_configuration.shapes.push(Shape {
                polarity: shape_configuration.polarity,
                starting_point,
                segments,
            });
        } else {
            // This in't a line, it's a dot.
            Self::circle(shape_configuration, start, diameter, None)
        }
    }

    pub fn arch(
        shape_configuration: ShapeConfiguration,
        diameter: f64,
        center: Vector2<f64>,
        start: Vector2<f64>,
        end: Vector2<f64>,
        direction: ArchDirection,
    ) {
        let start = shape_configuration.transform * start;
        let end = shape_configuration.transform * end;
        let center = shape_configuration.transform * center;

        let radius = shape_configuration.transform[0] * (diameter / 2.0);

        let starting_angle = (start.y - center.y).atan2(start.x - center.x);
        let starting_direction = starting_angle.sin_cos();
        let starting_direction = Vector2::new(starting_direction.1, starting_direction.0);

        let ending_angle = (end.y - center.y).atan2(end.x - center.x);
        let ending_direction = ending_angle.sin_cos();
        let ending_direction = Vector2::new(ending_direction.1, ending_direction.0);

        let arc_diameter = (start - center).norm();

        let starting_point = center + starting_direction * (arc_diameter - radius);

        let segments = match direction {
            ArchDirection::Clockwise => vec![
                Segment::ClockwiseCurve {
                    end: center + starting_direction * (arc_diameter + radius),
                    center: center + starting_direction * arc_diameter,
                },
                Segment::ClockwiseCurve {
                    end: center + ending_direction * (arc_diameter + radius),
                    center,
                },
                Segment::ClockwiseCurve {
                    end: center + ending_direction * (arc_diameter - radius),
                    center: center + ending_direction * arc_diameter,
                },
                Segment::CounterClockwiseCurve {
                    end: starting_point,
                    center,
                },
            ],
            ArchDirection::CounterClockwise => vec![
                Segment::CounterClockwiseCurve {
                    end: center + starting_direction * (arc_diameter + radius),
                    center: center + starting_direction * arc_diameter,
                },
                Segment::CounterClockwiseCurve {
                    end: center + ending_direction * (arc_diameter + radius),
                    center,
                },
                Segment::CounterClockwiseCurve {
                    end: center + ending_direction * (arc_diameter - radius),
                    center: center + ending_direction * arc_diameter,
                },
                Segment::ClockwiseCurve {
                    end: starting_point,
                    center,
                },
            ],
        };

        shape_configuration.shapes.push(Shape {
            polarity: shape_configuration.polarity,
            starting_point,
            segments,
        });
    }

    pub fn square_line(
        shape_configuration: ShapeConfiguration,
        width: f64,
        start: Vector2<f64>,
        end: Vector2<f64>,
    ) {
        let start = shape_configuration.transform * start;
        let end = shape_configuration.transform * end;

        let half_width = shape_configuration.transform[0] * width / 2.0;

        let direction = (start - end).normalize();
        let perpendicular = {
            let mut perpendicular = direction;
            perpendicular.swap_rows(0, 1);
            perpendicular.x *= -1.0;
            perpendicular
        } * half_width;

        let starting_point = start + perpendicular;

        let segments = vec![
            Segment::Line {
                end: start - perpendicular,
            },
            Segment::Line {
                end: end - perpendicular,
            },
            Segment::Line {
                end: end + perpendicular,
            },
            Segment::Line {
                end: start + perpendicular,
            },
        ];

        shape_configuration.shapes.push(Shape {
            polarity: shape_configuration.polarity,
            starting_point,
            segments,
        });
    }

    pub fn add_hole(
        transform: Matrix2<f64>,
        shapes: &mut Vec<Shape>,
        center: Vector2<f64>,
        hole_diameter: Option<f64>,
    ) {
        if let Some(hole_diameter) = hole_diameter {
            let center = transform * center;
            let radius = transform[0] * hole_diameter / 2.0;
            let starting_point = center + Vector2::new(radius, 0.0);

            shapes.push(Shape {
                polarity: Polarity::Clear,
                starting_point,
                segments: vec![
                    Segment::ClockwiseCurve {
                        end: center - Vector2::new(radius, 0.0),
                        center,
                    },
                    Segment::ClockwiseCurve {
                        end: starting_point,
                        center,
                    },
                ],
            });
        }
    }

    pub fn circle(
        shape_configuration: ShapeConfiguration,
        center: Vector2<f64>,
        diameter: f64,
        hole_diameter: Option<f64>,
    ) {
        let transformed_center = shape_configuration.transform * center;
        let radius = diameter / 2.0;
        let starting_point = transformed_center + Vector2::new(radius, 0.0);

        shape_configuration.shapes.push(Shape {
            polarity: shape_configuration.polarity,
            starting_point,
            segments: vec![
                Segment::ClockwiseCurve {
                    end: transformed_center - Vector2::new(radius, 0.0),
                    center: transformed_center,
                },
                Segment::ClockwiseCurve {
                    end: starting_point,
                    center: transformed_center,
                },
            ],
        });

        Self::add_hole(
            shape_configuration.transform,
            shape_configuration.shapes,
            center,
            hole_diameter,
        );
    }

    pub fn rectangle(
        shape_configuration: ShapeConfiguration,
        position: Vector2<f64>,
        width: f64,
        height: f64,
        hole_diameter: Option<f64>,
    ) {
        let half_width = width / 2.0;
        let half_height = height / 2.0;

        let left = position.x - half_width;
        let right = position.x + half_width;
        let bottom = position.y - half_height;
        let top = position.y + half_height;

        let starting_point = Vector2::new(right, bottom);

        shape_configuration.shapes.push(Shape {
            polarity: shape_configuration.polarity,
            starting_point,
            segments: vec![
                Segment::Line {
                    end: shape_configuration.transform * Vector2::new(right, top),
                },
                Segment::Line {
                    end: shape_configuration.transform * Vector2::new(left, top),
                },
                Segment::Line {
                    end: shape_configuration.transform * Vector2::new(left, bottom),
                },
                Segment::Line {
                    end: shape_configuration.transform * Vector2::new(right, bottom),
                },
            ],
        });

        Self::add_hole(
            shape_configuration.transform,
            shape_configuration.shapes,
            position,
            hole_diameter,
        );
    }

    pub fn obround(
        shape_configuration: ShapeConfiguration,
        position: Vector2<f64>,
        width: f64,
        height: f64,
        hole_diameter: Option<f64>,
    ) {
        let half_width = width / 2.0;
        let half_height = height / 2.0;

        let left = position.x - half_width;
        let right = position.x + half_width;
        let bottom = position.y - half_height + half_width;
        let top = position.y + half_height - half_width;

        let starting_point = Vector2::new(right, bottom);

        shape_configuration.shapes.push(Shape {
            polarity: shape_configuration.polarity,
            starting_point,
            segments: vec![
                Segment::Line {
                    end: shape_configuration.transform * Vector2::new(right, top),
                },
                Segment::CounterClockwiseCurve {
                    end: shape_configuration.transform * Vector2::new(left, top),
                    center: shape_configuration.transform * Vector2::new(position.x, top),
                },
                Segment::Line {
                    end: shape_configuration.transform * Vector2::new(left, bottom),
                },
                Segment::CounterClockwiseCurve {
                    end: shape_configuration.transform * Vector2::new(right, bottom),
                    center: shape_configuration.transform * Vector2::new(position.x, bottom),
                },
            ],
        });

        Self::add_hole(
            shape_configuration.transform,
            shape_configuration.shapes,
            position,
            hole_diameter,
        );
    }

    pub fn polygon(
        shape_configuration: ShapeConfiguration,
        position: Vector2<f64>,
        diameter: f64,
        num_vertices: u32,
        rotation: f64,
        hole_diameter: Option<f64>,
    ) {
        let rotation = rotation.to_radians();
        let angle_per_step = (std::f64::consts::PI * 2.0) / num_vertices as f64;

        let (direction_y, direction_x) = rotation.sin_cos();
        let direction = Vector2::new(direction_x, direction_y);
        let starting_point = position + direction * diameter;

        let mut segments = Vec::new();

        for index in 1..num_vertices as usize {
            let angle = rotation + angle_per_step * index as f64;
            let (direction_y, direction_x) = angle.sin_cos();
            let direction = Vector2::new(direction_x, direction_y);
            let point = position + direction * diameter;
            segments.push(Segment::Line { end: point });
        }

        shape_configuration.shapes.push(Shape {
            polarity: Polarity::Dark,
            starting_point,
            segments,
        });

        Self::add_hole(
            shape_configuration.transform,
            shape_configuration.shapes,
            position,
            hole_diameter,
        );
    }

    pub fn thermal(
        shape_configuration: ShapeConfiguration,
        position: Vector2<f64>,
        outer_diameter: f64,
        inner_diameter: f64,
        gap_thickness: f64,
        rotation: f64,
    ) {
        let half_gap = gap_thickness / 2.0;
        let center = shape_configuration.transform * position;
        let transform =
            Rotation2::new(rotation.to_radians()).matrix() * shape_configuration.transform;

        // Top right.
        let starting_point = transform
            * (position
                + Vector2::new((inner_diameter.powi(2) - half_gap.powi(2)).sqrt(), half_gap));
        shape_configuration.shapes.push(Shape {
            polarity: shape_configuration.polarity,
            starting_point,
            segments: vec![
                Segment::ClockwiseCurve {
                    end: transform
                        * (position
                            + Vector2::new(
                                half_gap,
                                (inner_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                            )),
                    center,
                },
                Segment::Line {
                    end: transform
                        * (position
                            + Vector2::new(
                                half_gap,
                                (outer_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                            )),
                },
                Segment::CounterClockwiseCurve {
                    end: transform
                        * (position
                            + Vector2::new(
                                (outer_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                                half_gap,
                            )),
                    center,
                },
                Segment::Line {
                    end: starting_point,
                },
            ],
        });

        // Top left.
        let starting_point = transform
            * (position
                + Vector2::new(
                    -(inner_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                    half_gap,
                ));
        shape_configuration.shapes.push(Shape {
            polarity: shape_configuration.polarity,
            starting_point,
            segments: vec![
                Segment::ClockwiseCurve {
                    end: transform
                        * (position
                            + Vector2::new(
                                -half_gap,
                                (inner_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                            )),
                    center,
                },
                Segment::Line {
                    end: transform
                        * (position
                            + Vector2::new(
                                -half_gap,
                                (outer_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                            )),
                },
                Segment::CounterClockwiseCurve {
                    end: transform
                        * (position
                            + Vector2::new(
                                -(outer_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                                half_gap,
                            )),
                    center,
                },
                Segment::Line {
                    end: starting_point,
                },
            ],
        });

        // Bottom right.
        let starting_point = transform
            * (position
                + Vector2::new(
                    (inner_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                    -half_gap,
                ));
        shape_configuration.shapes.push(Shape {
            polarity: shape_configuration.polarity,
            starting_point,
            segments: vec![
                Segment::ClockwiseCurve {
                    end: transform
                        * (position
                            + Vector2::new(
                                half_gap,
                                -(inner_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                            )),
                    center,
                },
                Segment::Line {
                    end: transform
                        * (position
                            + Vector2::new(
                                half_gap,
                                -(outer_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                            )),
                },
                Segment::CounterClockwiseCurve {
                    end: transform
                        * (position
                            + Vector2::new(
                                (outer_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                                -half_gap,
                            )),
                    center,
                },
                Segment::Line {
                    end: starting_point,
                },
            ],
        });

        // Bottom Left.
        let starting_point = transform
            * (position
                - Vector2::new((inner_diameter.powi(2) - half_gap.powi(2)).sqrt(), half_gap));
        shape_configuration.shapes.push(Shape {
            polarity: shape_configuration.polarity,
            starting_point,
            segments: vec![
                Segment::ClockwiseCurve {
                    end: transform
                        * (position
                            - Vector2::new(
                                half_gap,
                                (inner_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                            )),
                    center,
                },
                Segment::Line {
                    end: transform
                        * (position
                            - Vector2::new(
                                half_gap,
                                (outer_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                            )),
                },
                Segment::CounterClockwiseCurve {
                    end: transform
                        * (position
                            - Vector2::new(
                                (outer_diameter.powi(2) - half_gap.powi(2)).sqrt(),
                                half_gap,
                            )),
                    center,
                },
                Segment::Line {
                    end: starting_point,
                },
            ],
        });
    }
}

#[derive(Debug, Clone)]
pub enum Segment {
    Line {
        end: Vector2<f64>,
    },
    ClockwiseCurve {
        end: Vector2<f64>,
        center: Vector2<f64>,
    },
    CounterClockwiseCurve {
        end: Vector2<f64>,
        center: Vector2<f64>,
    },
}

impl Segment {
    fn debug_render(&self, start: Vector2<f64>) -> Box<dyn Command> {
        match self {
            Segment::Line { end } => Box::new(LineTo {
                point: (end.x, end.y),
                option: LineToOption::Default,
                coordinate_type: CoordinateType::Absolute,
            }),
            Segment::ClockwiseCurve { end, center } => {
                let diameter = (end - center).norm();
                Box::new(SvgArc {
                    radius: (diameter / 2.0, diameter / 2.0),
                    x_axis_rotation: 0.0,
                    large_arc_flag: *end == start,
                    sweep_flag: *end == start, // Clockwise
                    point: (end.x, end.y),
                    coordinate_type: CoordinateType::Absolute,
                })
            }
            Segment::CounterClockwiseCurve { end, center } => {
                let diameter = (end - center).norm();
                Box::new(SvgArc {
                    radius: (diameter / 2.0, diameter / 2.0),
                    x_axis_rotation: 0.0,
                    large_arc_flag: *end == start,
                    sweep_flag: *end != start, // CounterClockwise
                    point: (end.x, end.y),
                    coordinate_type: CoordinateType::Absolute,
                })
            }
        }
    }

    fn calculate_bounds(&self) -> (f64, f64, f64, f64) {
        match self {
            Segment::Line { end } => (end.x, end.y, end.x, end.y),
            Segment::ClockwiseCurve { end, center }
            | Segment::CounterClockwiseCurve { end, center } => {
                let diameter = (end - center).norm();
                let radius = diameter / 2.0;
                (
                    end.x - radius,
                    end.y - radius,
                    end.x + radius,
                    end.y + radius,
                )
            }
        }
    }

    fn end(&self) -> Vector2<f64> {
        match self {
            Segment::Line { end } => *end,
            Segment::ClockwiseCurve { end, center: _ } => *end,
            Segment::CounterClockwiseCurve { end, center: _ } => *end,
        }
    }

    fn append_to_line_string(
        &self,
        distance_per_step: f64,
        start: Vector2<f64>,
        points: &mut Vec<Coord<f64>>,
    ) {
        fn arc_to_cords(
            distance_per_step: f64,
            start: Vector2<f64>,
            end: Vector2<f64>,
            center: Vector2<f64>,
            direction: ArchDirection,
            points: &mut Vec<Coord<f64>>,
        ) {
            let center_to_start = start - center;
            let center_to_end = end - center;

            let dot_product = center_to_start.dot(&center_to_end);

            let radius = center_to_start.norm();

            let angle = (dot_product / radius.powi(2)).clamp(-1.0, 1.0).acos();
            let angle = if angle == 0.0 {
                // That means this is actually a circle and we need to make a full rotation.
                std::f64::consts::PI * 2.0
            } else {
                angle
            };

            let starting_angle = (start.y - center.y).atan2(start.x - center.x);

            let arch_length = angle * radius;
            let steps = (arch_length / distance_per_step).ceil();

            let angle_direction = if matches!(direction, ArchDirection::Clockwise) {
                -1.0
            } else {
                1.0
            };

            let angle_step = (angle / steps) * angle_direction;

            let steps = steps as usize;

            for step_index in 0..steps {
                let angle = starting_angle + angle_step * step_index as f64;

                let (sin, cos) = angle.sin_cos();
                let offset = Vector2::new(cos, sin) * radius;

                let new_position = center + offset;

                points.push(Coord {
                    x: new_position.x,
                    y: new_position.y,
                })
            }

            points.push(Coord { x: end.x, y: end.y });
        }

        match self {
            Segment::Line { end } => {
                points.push(Coord { x: end.x, y: end.y });
            }
            Segment::ClockwiseCurve { end, center } => arc_to_cords(
                distance_per_step,
                start,
                *end,
                *center,
                ArchDirection::Clockwise,
                points,
            ),
            Segment::CounterClockwiseCurve { end, center } => arc_to_cords(
                distance_per_step,
                start,
                *end,
                *center,
                ArchDirection::CounterClockwise,
                points,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ArchDirection {
    Clockwise,
    CounterClockwise,
}
