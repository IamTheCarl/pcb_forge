use std::collections::HashMap;

use anyhow::{bail, Result};
use geo::{Coord, LineString, Polygon};
use nalgebra::{Matrix2, Vector2};
use ordered_float::NotNan;
use svg_composer::element::path::{
    command::{Arc as SvgArc, CoordinateType, LineTo, LineToOption, MoveTo},
    Command,
};

use crate::parsing::gerber::Polarity;

#[derive(Debug)]
pub struct Shape {
    pub polarity: Polarity,
    pub starting_point: Vector2<f64>,
    pub segments: Vec<Segment>,
}

impl Shape {
    pub fn debug_render(&self, path: &mut Vec<Box<dyn Command>>) -> Result<()> {
        path.push(Box::new(MoveTo {
            point: (self.starting_point.x as f64, self.starting_point.y as f64),
            coordinate_type: CoordinateType::Absolute,
        }));

        for segment in self.segments.iter() {
            path.push(segment.debug_render());
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

    fn convert_to_line_string(&self, distance_per_step: f64) -> LineString<f64> {
        let mut points = Vec::new();

        let mut start_point = self.starting_point;
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
                    Segment::ClockwiseCurve { end, diameter: _ } => end,
                    Segment::CounterClockwiseCurve { end, diameter: _ } => end,
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
                Segment::ClockwiseCurve { end, diameter } => {
                    separator_function(
                        self.polarity,
                        &mut starting_point,
                        &mut repeatable_segments,
                        &mut collected_segments,
                        &mut shapes,
                        *end,
                        SegmentInfo::Clockwise {
                            diameter: NotNan::new(*diameter).expect("Got NAN"),
                        },
                    );
                }
                Segment::CounterClockwiseCurve { end, diameter } => {
                    separator_function(
                        self.polarity,
                        &mut starting_point,
                        &mut repeatable_segments,
                        &mut collected_segments,
                        &mut shapes,
                        *end,
                        SegmentInfo::CounterClockwise {
                            diameter: NotNan::new(*diameter).expect("Got NAN"),
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
            outer_shape.convert_to_line_string(distance_per_step),
            shapes
                .drain(..)
                .map(|shape| shape.convert_to_line_string(distance_per_step))
                .collect(),
        )
    }

    pub fn line(
        transform: Matrix2<f64>,
        polarity: Polarity,
        diameter: f64,
        start: Vector2<f64>,
        end: Vector2<f64>,
    ) -> Self {
        let start = transform * start;
        let end = transform * end;

        let radius = transform[0] * diameter / 2.0;
        let direction = (start - end).normalize();

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
                diameter,
            },
            Segment::Line {
                end: end - perpendicular,
            },
            Segment::ClockwiseCurve {
                end: end + perpendicular,
                diameter,
            },
            Segment::Line {
                end: start + perpendicular,
            },
        ];

        Shape {
            polarity,
            starting_point,
            segments,
        }
    }

    pub fn square_line(
        transform: Matrix2<f64>,
        polarity: Polarity,
        width: f64,
        start: Vector2<f64>,
        end: Vector2<f64>,
    ) -> Self {
        let start = transform * start;
        let end = transform * end;

        let half_width = transform[0] * width / 2.0;

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

        Shape {
            polarity,
            starting_point,
            segments,
        }
    }

    pub fn add_hole(
        transform: Matrix2<f64>,
        shapes: &mut Vec<Shape>,
        position: Vector2<f64>,
        hole_diameter: Option<f64>,
    ) {
        if let Some(hole_diameter) = hole_diameter {
            let position = transform * position;
            let radius = transform[0] * hole_diameter / 2.0;
            let starting_point = position + Vector2::new(radius, 0.0);

            shapes.push(Shape {
                polarity: Polarity::Clear,
                starting_point,
                segments: vec![
                    Segment::ClockwiseCurve {
                        end: position - Vector2::new(radius, 0.0),
                        diameter: hole_diameter,
                    },
                    Segment::ClockwiseCurve {
                        end: starting_point,
                        diameter: hole_diameter,
                    },
                ],
            });
        }
    }

    pub fn circle(
        transform: Matrix2<f64>,
        shapes: &mut Vec<Shape>,
        polarity: Polarity,
        position: Vector2<f64>,
        diameter: f64,
        hole_diameter: Option<f64>,
    ) {
        let radius = diameter / 2.0;
        let starting_point = position + Vector2::new(radius, 0.0);

        shapes.push(Shape {
            polarity,
            starting_point,
            segments: vec![
                Segment::ClockwiseCurve {
                    end: position - Vector2::new(radius, 0.0),
                    diameter,
                },
                Segment::ClockwiseCurve {
                    end: starting_point,
                    diameter,
                },
            ],
        });

        Self::add_hole(transform, shapes, position, hole_diameter);
    }

    pub fn rectangle(
        transform: Matrix2<f64>,
        shapes: &mut Vec<Shape>,
        polarity: Polarity,
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

        shapes.push(Shape {
            polarity,
            starting_point,
            segments: vec![
                Segment::Line {
                    end: transform * Vector2::new(right, top),
                },
                Segment::Line {
                    end: transform * Vector2::new(left, top),
                },
                Segment::Line {
                    end: transform * Vector2::new(left, bottom),
                },
                Segment::Line {
                    end: transform * Vector2::new(right, bottom),
                },
            ],
        });

        Self::add_hole(transform, shapes, position, hole_diameter);
    }

    pub fn obround(
        transform: Matrix2<f64>,
        shapes: &mut Vec<Shape>,
        polarity: Polarity,
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

        shapes.push(Shape {
            polarity,
            starting_point,
            segments: vec![
                Segment::Line {
                    end: transform * Vector2::new(right, top),
                },
                Segment::CounterClockwiseCurve {
                    end: transform * Vector2::new(left, top),
                    diameter: transform[0] * half_width,
                },
                Segment::Line {
                    end: transform * Vector2::new(left, bottom),
                },
                Segment::CounterClockwiseCurve {
                    end: transform * Vector2::new(right, bottom),
                    diameter: transform[0] * half_width,
                },
            ],
        });

        Self::add_hole(transform, shapes, position, hole_diameter);
    }

    pub fn polygon(
        transform: Matrix2<f64>,
        shapes: &mut Vec<Shape>,
        polarity: Polarity,
        position: Vector2<f64>,
        diameter: f64,
        num_vertices: u32,
        rotation: f64,
        hole_diameter: Option<f64>,
    ) -> Result<()> {
        bail!("Unimplemented 1");

        // Self::add_hole(transform, shapes, position, hole_diameter);
    }
}

#[derive(Debug, Clone)]
pub enum Segment {
    Line { end: Vector2<f64> },
    ClockwiseCurve { end: Vector2<f64>, diameter: f64 },
    CounterClockwiseCurve { end: Vector2<f64>, diameter: f64 },
}

impl Segment {
    fn debug_render(&self) -> Box<dyn Command> {
        match self {
            Segment::Line { end } => Box::new(LineTo {
                point: (end.x as f64, end.y as f64),
                option: LineToOption::Default,
                coordinate_type: CoordinateType::Absolute,
            }),
            Segment::ClockwiseCurve { end, diameter } => Box::new(SvgArc {
                radius: (*diameter as f64 / 2.0, *diameter as f64 / 2.0),
                x_axis_rotation: 0.0,
                large_arc_flag: false,
                sweep_flag: false, // Clockwise
                point: (end.x as f64, end.y as f64),
                coordinate_type: CoordinateType::Absolute,
            }),
            Segment::CounterClockwiseCurve { end, diameter } => Box::new(SvgArc {
                radius: (*diameter as f64 / 2.0, *diameter as f64 / 2.0),
                x_axis_rotation: 0.0,
                large_arc_flag: false,
                sweep_flag: true, // CounterClockwise
                point: (end.x as f64, end.y as f64),
                coordinate_type: CoordinateType::Absolute,
            }),
        }
    }

    fn calculate_bounds(&self) -> (f64, f64, f64, f64) {
        match self {
            Segment::Line { end } => (end.x, end.y, end.x, end.y),
            Segment::ClockwiseCurve { end, diameter }
            | Segment::CounterClockwiseCurve { end, diameter } => (
                end.x - diameter * 2.0,
                end.y - diameter * 2.0,
                end.x + diameter * 2.0,
                end.y + diameter * 2.0,
            ),
        }
    }

    fn end(&self) -> Vector2<f64> {
        match self {
            Segment::Line { end } => *end,
            Segment::ClockwiseCurve { end, diameter: _ } => *end,
            Segment::CounterClockwiseCurve { end, diameter: _ } => *end,
        }
    }

    fn append_to_line_string(
        &self,
        distance_per_step: f64,
        start: Vector2<f64>,
        points: &mut Vec<Coord<f64>>,
    ) {
        enum ArchDirection {
            Clockwise,
            CounterClockwise,
        }

        fn arc_to_cords(
            distance_per_step: f64,
            start: Vector2<f64>,
            end: Vector2<f64>,
            diameter: f64,
            direction: ArchDirection,
            points: &mut Vec<Coord<f64>>,
        ) {
            let radius = diameter / 2.0;
            let chord = end - start;
            let chord_length = chord.norm();
            let chord_direction = chord.normalize();
            let chord_middle = start + (chord_length / 2.0) * chord_direction;
            let center_direction = Vector2::new(chord_direction.y, -chord_direction.x);
            let apothem = (radius.powi(2) - (chord_length.powi(2) / 4.0))
                .max(0.0)
                .sqrt();
            let center = if matches!(direction, ArchDirection::Clockwise) {
                chord_middle + center_direction * apothem
            } else {
                chord_middle - center_direction * apothem
            };

            let center_to_start = start - center;
            let center_to_end = end - center;

            let dot_product = center_to_start.dot(&center_to_end);

            let starting_radius = center_to_start.norm();
            let ending_radius = center_to_end.norm();
            let radius_delta = ending_radius - starting_radius;

            let angle = (dot_product / (starting_radius * ending_radius))
                .clamp(-1.0, 1.0)
                .acos();

            let starting_angle = (start.y - center.y).atan2(start.x - center.x);

            let arch_length = angle * starting_radius.max(ending_radius);
            let steps = (arch_length / distance_per_step).ceil();

            let angle_direction = if matches!(direction, ArchDirection::Clockwise) {
                -1.0
            } else {
                1.0
            };

            let angle_step = (angle / steps) * angle_direction;
            let radius_step = radius_delta / steps;

            let steps = steps as usize;

            for step_index in 0..steps {
                let angle = starting_angle + angle_step * step_index as f64;
                let radius = starting_radius + radius_step * step_index as f64;

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
            Segment::ClockwiseCurve { end, diameter } => arc_to_cords(
                distance_per_step,
                start,
                *end,
                *diameter,
                ArchDirection::Clockwise,
                points,
            ),
            Segment::CounterClockwiseCurve { end, diameter } => arc_to_cords(
                distance_per_step,
                start,
                *end,
                *diameter,
                ArchDirection::CounterClockwise,
                points,
            ),
        }
    }
}
