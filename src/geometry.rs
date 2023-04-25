use std::collections::HashMap;

use anyhow::{bail, Result};
use nalgebra::{Matrix2, Vector2};
use ordered_float::NotNan;
use svg_composer::{
    document::Document as SvgDocument,
    element::{
        attributes::{Color, ColorName, Paint, Size},
        path::{
            command::{Arc as SvgArc, CoordinateType, End, LineTo, LineToOption, MoveTo},
            Command, Path as SvgPath,
        },
        Element,
    },
};
use uom::si::{
    length::{millimeter, Length},
    ratio::{percent, Ratio},
    velocity::{millimeter_per_second, Velocity},
};

use crate::{
    config::machine::JobConfig,
    gcode_generation::{GCodeFile, GCommand, MovementType, ToolSelection},
    parsing::gerber::{Polarity, UnitMode},
};

pub struct ShapeCollection {
    pub shapes: Vec<Shape>,
}

impl ShapeCollection {
    pub fn debug_render(&self, svg: &mut SvgDocument, include_outline: bool) -> Result<()> {
        for (index, shape) in self.shapes.iter().enumerate() {
            let mut commands = Vec::new();

            shape.debug_render(&mut commands)?;

            commands.push(Box::new(End {}));

            let color = match shape.polarity {
                Polarity::Clear => Color::from_rgba(0, (index % 255) as u8, 255, 128),
                Polarity::Dark => Color::from_rgba(255, (index % 255) as u8, 0, 128),
            };

            let path = if !include_outline {
                SvgPath::new()
                    .set_fill(Paint::from_color(color))
                    .add_commands(commands)
            } else {
                SvgPath::new()
                    .set_stroke(Paint::from_color(Color::from_name(ColorName::Blue)))
                    .set_stroke_width(Size::from_length(0.02))
                    .set_fill(Paint::from_color(color))
                    .add_commands(commands)
            };

            svg.add_element(Box::new(path));
        }

        Ok(())
    }

    pub fn calculate_svg_bounds(&self) -> (f32, f32, f32, f32) {
        if !self.shapes.is_empty() {
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;

            for shape in self.shapes.iter() {
                let (local_min_x, local_min_y, local_max_x, local_max_y) = shape.calculate_bounds();
                min_x = min_x.min(local_min_x);
                min_y = min_y.min(local_min_y);
                max_x = max_x.max(local_max_x);
                max_y = max_y.max(local_max_y);
            }

            (min_x, min_y, max_x - min_x, max_y - min_y)
        } else {
            (0.0, 0.0, 0.0, 0.0)
        }
    }

    pub fn calculate_bounds(&self) -> (f32, f32, f32, f32) {
        if !self.shapes.is_empty() {
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;

            for shape in self.shapes.iter() {
                let (local_min_x, local_min_y, local_max_x, local_max_y) = shape.calculate_bounds();
                min_x = min_x.min(local_min_x);
                min_y = min_y.min(local_min_y);
                max_x = max_x.max(local_max_x);
                max_y = max_y.max(local_max_y);
            }

            (min_x, min_y, max_x, max_y)
        } else {
            (0.0, 0.0, 0.0, 0.0)
        }
    }

    pub fn generate_gcode(
        &self,
        job_config: &JobConfig,
        tool_config: &ToolSelection,
    ) -> Result<GCodeFile> {
        match job_config.tool_power {
            crate::config::machine::ToolConfig::Laser {
                laser_power,
                work_speed,
            } => {
                let mut commands = vec![
                    GCommand::AbsoluteMode,
                    GCommand::UnitMode(UnitMode::Metric),
                    GCommand::SetRapidTransverseSpeed(Velocity::new::<millimeter_per_second>(
                        3000.0, // TODO this should come from the config file.
                    )),
                    GCommand::SetWorkSpeed(work_speed),
                    GCommand::SetPower(laser_power),
                    GCommand::SetFanPower {
                        index: 0,
                        power: Ratio::new::<percent>(100.0), // TODO fan configurations should come from the machine config.
                    },
                ];

                // Start by generating GCode for the outlines.
                // FIXME this needs to account for the beam diameter.
                for shape in self.shapes.iter() {
                    commands.push(GCommand::MoveTo {
                        target: (
                            Length::new::<millimeter>(shape.starting_point.x),
                            Length::new::<millimeter>(shape.starting_point.y),
                        ),
                    });

                    for segment in shape.segments.iter() {
                        match segment {
                            Segment::Line { end } => {
                                commands.push(GCommand::Cut {
                                    movement: MovementType::Linear,
                                    target: (
                                        Length::new::<millimeter>(end.x),
                                        Length::new::<millimeter>(end.y),
                                    ),
                                });
                            }
                            Segment::ClockwiseCurve { end, diameter } => {
                                commands.push(GCommand::Cut {
                                    movement: MovementType::ClockwiseCurve {
                                        diameter: Length::new::<millimeter>(*diameter),
                                    },
                                    target: (
                                        Length::new::<millimeter>(end.x),
                                        Length::new::<millimeter>(end.y),
                                    ),
                                });
                            }
                            Segment::CounterClockwiseCurve { end, diameter } => {
                                commands.push(GCommand::Cut {
                                    movement: MovementType::CounterClockwiseCurve {
                                        diameter: Length::new::<millimeter>(*diameter),
                                    },
                                    target: (
                                        Length::new::<millimeter>(end.x),
                                        Length::new::<millimeter>(end.y),
                                    ),
                                });
                            }
                        }
                    }
                }

                // TODO Now we generate the infill.
                let (min_x, min_y, max_x, max_y) = self.calculate_bounds();
                let (span_x, span_y) = (max_x - min_x, max_y - min_y);
                // let (delta_x, delta_y) = (span_x / tool_config)

                Ok(GCodeFile::new(laser_power, commands))
            }
            crate::config::machine::ToolConfig::Drill {
                spindle_rpm: _,
                plunge_speed: _,
            } => bail!("gerber files cannot be drilled"),
            crate::config::machine::ToolConfig::EndMill {
                spindle_rpm,
                max_cut_depth,
                plunge_speed,
                work_speed,
            } => bail!("milling gerber files is not yet supported"),
        }
    }
}

#[derive(Debug)]
pub struct Shape {
    pub polarity: Polarity,
    pub starting_point: Vector2<f32>,
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

    pub fn calculate_bounds(&self) -> (f32, f32, f32, f32) {
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

    pub fn simplify(&self) -> Vec<Shape> {
        // Start by separating the internal holes from the outer shape.
        #[derive(Debug, Eq, PartialEq, Hash, Clone, Copy)]
        enum SegmentInfo {
            Line,
            Clockwise { diameter: NotNan<f32> },
            CounterClockwise { diameter: NotNan<f32> },
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
            starting_point: &mut Vector2<f32>,
            repeatable_segments: &mut HashMap<(NotNan<f32>, NotNan<f32>, SegmentInfo), usize>,
            collected_segments: &mut Vec<Segment>,
            shapes: &mut Vec<Shape>,
            end: Vector2<f32>,
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

        shapes.insert(
            0,
            Shape {
                polarity: self.polarity,
                starting_point: self.starting_point,
                segments: collected_segments,
            },
        );

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

        shapes
    }

    pub fn merge(&self, shape: &Shape) -> Option<Shape> {
        // todo!()
        None
    }

    pub fn line(
        transform: Matrix2<f32>,
        polarity: Polarity,
        diameter: f32,
        start: Vector2<f32>,
        end: Vector2<f32>,
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
        transform: Matrix2<f32>,
        polarity: Polarity,
        width: f32,
        start: Vector2<f32>,
        end: Vector2<f32>,
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
        transform: Matrix2<f32>,
        shapes: &mut Vec<Shape>,
        position: Vector2<f32>,
        hole_diameter: Option<f32>,
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
        transform: Matrix2<f32>,
        shapes: &mut Vec<Shape>,
        polarity: Polarity,
        position: Vector2<f32>,
        diameter: f32,
        hole_diameter: Option<f32>,
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
        transform: Matrix2<f32>,
        shapes: &mut Vec<Shape>,
        polarity: Polarity,
        position: Vector2<f32>,
        width: f32,
        height: f32,
        hole_diameter: Option<f32>,
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
        transform: Matrix2<f32>,
        shapes: &mut Vec<Shape>,
        polarity: Polarity,
        position: Vector2<f32>,
        width: f32,
        height: f32,
        hole_diameter: Option<f32>,
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
        transform: Matrix2<f32>,
        shapes: &mut Vec<Shape>,
        polarity: Polarity,
        position: Vector2<f32>,
        diameter: f32,
        num_vertices: u32,
        rotation: f32,
        hole_diameter: Option<f32>,
    ) -> Result<()> {
        bail!("Unimplemented 1");

        // Self::add_hole(transform, shapes, position, hole_diameter);
    }
}

#[derive(Debug, Clone)]
pub enum Segment {
    Line { end: Vector2<f32> },
    ClockwiseCurve { end: Vector2<f32>, diameter: f32 },
    CounterClockwiseCurve { end: Vector2<f32>, diameter: f32 },
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

    fn calculate_bounds(&self) -> (f32, f32, f32, f32) {
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

    fn end(&self) -> Vector2<f32> {
        match self {
            Segment::Line { end } => *end,
            Segment::ClockwiseCurve { end, diameter: _ } => *end,
            Segment::CounterClockwiseCurve { end, diameter: _ } => *end,
        }
    }

    fn contains_point_test(&self, segment_start: Vector2<f32>, point: Vector2<f32>) -> bool {
        todo!()
    }
}
