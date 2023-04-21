use anyhow::{bail, Context, Result};
use nalgebra::{Matrix2, Rotation2, Vector2};
use std::{collections::HashMap, fs, ops::Deref, path::Path};
use svg_composer::{
    document::Document as SvgDocument,
    element::{
        attributes::{Color, Paint},
        path::{
            command::{Arc as SvgArc, CoordinateType, End, LineTo, LineToOption, MoveTo},
            Command,
        },
        Element, Path as SvgPath,
    },
};
use uom::si::length::{mil, millimeter, Length};

use crate::parsing::gerber::{
    parse_gerber_file, ApertureTemplate, Attribute, GerberCommand, MacroContent, MirroringMode,
    Operation, Polarity, Span, UnitMode,
};

#[derive(Debug, Default)]
pub struct GerberFile {
    shapes: Vec<Shape>,
    aperture_macro_flashes: Vec<Vec<Shape>>,
}

impl GerberFile {
    pub fn debug_render(&self, svg: &mut SvgDocument) -> Result<()> {
        for (index, shape) in self
            .shapes
            .iter()
            .chain(self.aperture_macro_flashes.iter().flatten())
            .enumerate()
        {
            let mut commands = Vec::new();

            shape.debug_render(&mut commands)?;

            commands.push(Box::new(End {}));

            let color = match shape.polarity {
                Polarity::Clear => Color::from_rgba(0, (index % 255) as u8, 255, 128),
                Polarity::Dark => Color::from_rgba(255, (index % 255) as u8, 0, 128),
            };

            let path = SvgPath::new()
                .set_fill(Paint::from_color(color))
                .add_commands(commands);

            svg.add_element(Box::new(path));
        }

        Ok(())
    }

    pub fn calculate_bounds(&self) -> (f32, f32, f32, f32) {
        if !self.shapes.is_empty() {
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;

            for shape in self
                .shapes
                .iter()
                .chain(self.aperture_macro_flashes.iter().flatten())
            {
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
}

#[derive(Debug)]
struct Shape {
    polarity: Polarity,
    segments: Vec<Segment>,
}

impl Shape {
    fn debug_render(&self, path: &mut Vec<Box<dyn Command>>) -> Result<()> {
        for segment in self.segments.iter() {
            path.push(segment.debug_render());
        }

        Ok(())
    }

    fn calculate_bounds(&self) -> (f32, f32, f32, f32) {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;

        for segment in self.segments.iter() {
            let (local_min_x, local_min_y, local_max_x, local_max_y) = segment.calculate_bounds();
            min_x = min_x.min(local_min_x);
            min_y = min_y.min(local_min_y);
            max_x = max_x.max(local_max_x);
            max_y = max_y.max(local_max_y);
        }

        (min_x, min_y, max_x, max_y)
    }

    fn line(
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
            Segment::Move {
                target: starting_point,
            },
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

        Shape { polarity, segments }
    }

    fn square_line(
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

        let segments = vec![
            Segment::Move {
                target: start + perpendicular,
            },
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

        Shape { polarity, segments }
    }

    fn add_hole(
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
                segments: vec![
                    Segment::Move {
                        target: starting_point,
                    },
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

    fn circle(
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
            segments: vec![
                Segment::Move {
                    target: starting_point,
                },
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

    fn rectangle(
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

        shapes.push(Shape {
            polarity,
            segments: vec![
                Segment::Move {
                    target: Vector2::new(right, bottom),
                },
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

    fn obround(
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

        shapes.push(Shape {
            polarity,
            segments: vec![
                Segment::Move {
                    target: Vector2::new(right, bottom),
                },
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

    fn polygon(
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

    fn aperture_macro(
        transform: Matrix2<f32>,
        format: &Format,
        shapes: &mut Vec<Shape>,
        position: Vector2<f32>,
        aperture_macro: &[MacroContent],
        arguments: &[f32],
    ) -> Result<()> {
        let position = transform * position;
        let mut variables: HashMap<u32, f32> = arguments
            .iter()
            .enumerate()
            .map(|(index, value)| (index as u32 + 1, *value))
            .collect();

        for command in aperture_macro {
            match command {
                MacroContent::Comment(_comment) => {}
                MacroContent::Circle {
                    exposure,
                    diameter,
                    center_position: (x, y),
                    angle,
                } => {
                    let transform = Rotation2::new(angle.evaluate(&variables)?.to_radians())
                        .matrix()
                        * transform;

                    let center_position = transform
                        * Vector2::new(x.evaluate(&variables)?, y.evaluate(&variables)?)
                        + position;
                    let diameter = diameter.evaluate(&variables)?;

                    Shape::circle(
                        transform,
                        shapes,
                        *exposure,
                        center_position,
                        diameter,
                        None,
                    );
                }
                MacroContent::VectorLine {
                    exposure,
                    width,
                    start: (start_x, start_y),
                    end: (end_x, end_y),
                    angle,
                } => {
                    let transform = Rotation2::new(angle.evaluate(&variables)?.to_radians())
                        .matrix()
                        * transform;

                    shapes.push(Shape::square_line(
                        transform,
                        *exposure,
                        width.evaluate(&variables)?,
                        Vector2::new(start_x.evaluate(&variables)?, start_y.evaluate(&variables)?)
                            + position,
                        Vector2::new(end_x.evaluate(&variables)?, end_y.evaluate(&variables)?)
                            + position,
                    ));
                }
                MacroContent::CenterLine {
                    exposure,
                    size,
                    center,
                    angle,
                } => bail!("Unimplemented 3.2"),
                MacroContent::Outline {
                    exposure,
                    coordinates,
                    angle,
                } => {
                    let transform = Rotation2::new(angle.evaluate(&variables)?.to_radians())
                        .matrix()
                        * transform;

                    let mut coordinate_iter =
                        coordinates.iter().map(|(x, y)| -> Result<Vector2<f32>> {
                            let x =
                                format.internalize_coordinate_from_float(x.evaluate(&variables)?);
                            let y =
                                format.internalize_coordinate_from_float(y.evaluate(&variables)?);
                            Ok(transform * Vector2::new(x, y) + position)
                        });

                    let starting_point = coordinate_iter
                        .next()
                        .context("Outline must have at least one point.")??;

                    let segments = {
                        let mut segments = vec![Segment::Move {
                            target: starting_point,
                        }];
                        for end in coordinate_iter {
                            let end = end?;
                            segments.push(Segment::Line { end });
                        }

                        segments
                    };

                    shapes.push(Shape {
                        polarity: *exposure,
                        segments,
                    });
                }
                MacroContent::Polygon {
                    exposure,
                    num_vertices,
                    center_position,
                    diameter,
                    angle,
                } => bail!("Unimplemented 3.4"),
                MacroContent::Thermal {
                    center_point,
                    outer_diameter,
                    inner_diameter,
                    gap_thickness,
                    angle,
                } => bail!("Unimplemented 3.5"),
                MacroContent::VariableDefinition {
                    variable,
                    expression,
                } => bail!("Unimplemented 3.6"),
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
enum Segment {
    Move { target: Vector2<f32> },
    Line { end: Vector2<f32> },
    ClockwiseCurve { end: Vector2<f32>, diameter: f32 },
    CounterClockwiseCurve { end: Vector2<f32>, diameter: f32 },
}

impl Segment {
    pub fn debug_render(&self) -> Box<dyn Command> {
        match self {
            Segment::Move { target } => Box::new(MoveTo {
                point: (target.x as f64, target.y as f64),
                coordinate_type: CoordinateType::Absolute,
            }),
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
            Segment::Move { target } => (target.x, target.y, target.x, target.y),
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
}

pub fn load(gerber_file: &mut GerberFile, path: &Path) -> Result<()> {
    // The only reason we don't just construct a gerber file ourselves is so that we can debug render the partial gerber file in the case of an error.
    assert!(gerber_file.shapes.is_empty());

    let file_content = fs::read_to_string(path).context("Failed to read file into memory.")?;
    let parsing_result = parse_gerber_file(Span::new(&file_content));

    match parsing_result {
        Ok((_unused_content, commands)) => {
            let mut context = PlottingContext {
                user_attributes: HashMap::new(),
                file_attributes: HashMap::new(),
                aperture_attributes: HashMap::new(),
                object_attributes: HashMap::new(),

                aperture_macros: HashMap::new(),
                aperture_definitions: HashMap::new(),

                current_point: Vector2::new(0.0, 0.0),
                current_aperture: 0,
                draw_mode: DrawMode::Linear,
                format: Format {
                    integer_digits: 3,
                    decimal_digits: 5,
                    unit_mode: UnitMode::Metric,
                },

                polarity: Polarity::Dark,
                mirroring: MirroringMode::None,
                rotation: 0.0,
                scaling: 1.0,
            };

            for command in commands {
                let location_info = command.location_info();

                context
                    .process_command(command.command, gerber_file, path)
                    .with_context(move || {
                        format!(
                            "error processing command: {}:{}",
                            path.to_string_lossy(),
                            location_info
                        )
                    })?;
            }

            Ok(())
        }
        Err(error) => match error {
            nom::Err::Error(error) | nom::Err::Failure(error) => {
                let _ = error;
                bail!(
                    "Failed to parse gerber file {}:{}:{} - {:?}",
                    path.to_string_lossy(),
                    error.input.location_line(),
                    error.input.get_utf8_column(),
                    error.code,
                )
            }
            nom::Err::Incomplete(_) => bail!("Failed to parse gerber file: Unexpected EOF"),
        },
    }
}

#[derive(Debug)]
struct Format {
    integer_digits: u32,
    decimal_digits: u32,
    unit_mode: UnitMode,
}

impl Format {
    fn internalize_coordinate_from_span(&self, coordinate: Span) -> Result<f32> {
        // Get decimal part.
        let decimal = coordinate
            .get(
                coordinate
                    .len()
                    .saturating_sub(self.decimal_digits as usize)..,
            )
            .context("Not enough digits available for decimal part of coordinate.")?;
        let decimal = decimal
            .parse::<i32>()
            .context("internal decimal parsing error")?;
        let sign = decimal.signum();
        let decimal = decimal.abs();

        // Get integer part.
        let integer = &coordinate[..coordinate
            .len()
            .saturating_sub(self.decimal_digits as usize)];
        let integer = if !integer.is_empty() {
            integer
                .parse::<i32>()
                .context("internal integer parsing error")?
        } else {
            0
        };

        // Combine.
        let new_position = sign as f32
            * (integer as f32 + (decimal as f32 / (10.0f32.powi(self.decimal_digits as i32))));

        // Convert to mm for internal representation.
        Ok(self.internalize_coordinate_from_float(new_position))
    }

    fn internalize_coordinate_from_float(&self, coordinate: f32) -> f32 {
        // Convert to mm for internal representation.
        match self.unit_mode {
            UnitMode::Metric => Length::<uom::si::SI<f32>, f32>::new::<millimeter>(coordinate),
            UnitMode::Imperial => Length::<uom::si::SI<f32>, f32>::new::<mil>(coordinate),
        }
        .get::<millimeter>()
    }
}

#[derive(Debug)]
enum DrawMode {
    Linear,
    Clockwise,
    CounterClockwise,
}

#[derive(Debug)]
struct PlottingContext<'a> {
    user_attributes: HashMap<&'a str, Vec<Span<'a>>>,
    file_attributes: HashMap<&'a str, Vec<Span<'a>>>,
    aperture_attributes: HashMap<&'a str, Vec<Span<'a>>>,
    object_attributes: HashMap<&'a str, Vec<Span<'a>>>,

    aperture_macros: HashMap<&'a str, Vec<MacroContent<'a>>>,
    aperture_definitions: HashMap<u32, ApertureTemplate<'a>>,

    current_point: Vector2<f32>,
    current_aperture: u32,
    draw_mode: DrawMode,
    format: Format,

    polarity: Polarity,
    mirroring: MirroringMode,
    rotation: f32,
    scaling: f32,
}

impl<'a> PlottingContext<'a> {
    fn process_command(
        &mut self,
        command: GerberCommand<'a>,
        gerber_file: &mut GerberFile,
        path: &Path,
    ) -> Result<()> {
        match command {
            GerberCommand::Attribute(attribute) => match attribute {
                Attribute::User { name, values } => {
                    self.user_attributes.insert(name.fragment(), values);
                }
                Attribute::File { name, values } => {
                    self.file_attributes.insert(name.fragment(), values);
                }
                Attribute::Aperture { name, values } => {
                    self.aperture_attributes.insert(name.fragment(), values);
                }
                Attribute::Object { name, values } => {
                    self.object_attributes.insert(name.fragment(), values);
                }
                Attribute::Delete { name } => {
                    if let Some(name) = name {
                        self.user_attributes.remove(name.fragment());
                        self.aperture_attributes.remove(name.fragment());
                        self.object_attributes.remove(name.fragment());
                    } else {
                        self.user_attributes.clear();
                        self.aperture_attributes.clear();
                        self.object_attributes.clear();
                    }
                }
            },
            GerberCommand::Comment(_comment) => {}
            GerberCommand::SetAperture(index) => {
                if !self.aperture_definitions.contains_key(&index) {
                    bail!("Attempt to equip undefined or invalid aperture.");
                }
                self.current_aperture = index;
            }
            GerberCommand::Operation(operation) => match operation {
                Operation::Plot { x, y, i: _, j: _ } => match self.draw_mode {
                    DrawMode::Linear => {
                        let mut next_point = self.current_point;

                        if let Some(x) = x {
                            next_point.x = self.format.internalize_coordinate_from_span(x)?;
                        }

                        if let Some(y) = y {
                            next_point.y = self.format.internalize_coordinate_from_span(y)?;
                        }

                        let aperture = self
                            .aperture_definitions
                            .get(&self.current_aperture)
                            .context("Aperture was never equipped.")?;

                        if let ApertureTemplate::Circle {
                            diameter,
                            hole_diameter,
                        } = aperture
                        {
                            if hole_diameter.is_none() {
                                let shape = Shape::line(
                                    self.calculate_transformation_matrix(),
                                    self.polarity,
                                    *diameter,
                                    self.current_point,
                                    next_point,
                                );
                                self.current_point = next_point;

                                gerber_file.shapes.push(shape);
                            } else {
                                bail!("Circles used for line draws cannot have a hole in them.")
                            }
                        } else {
                            bail!("Only circles are supported for line draws.")
                        }
                    }
                    DrawMode::Clockwise => bail!("Unimplemented 1."),
                    DrawMode::CounterClockwise => bail!("Unimplemented 2."),
                },
                Operation::Move { x, y } => {
                    if let Some(x) = x {
                        self.current_point.x = self.format.internalize_coordinate_from_span(x)?;
                    }

                    if let Some(y) = y {
                        self.current_point.y = self.format.internalize_coordinate_from_span(y)?;
                    }
                }
                Operation::Flash { x, y } => {
                    if let Some(x) = x {
                        self.current_point.x = self.format.internalize_coordinate_from_span(x)?;
                    }

                    if let Some(y) = y {
                        self.current_point.y = self.format.internalize_coordinate_from_span(y)?;
                    }

                    let aperture = self
                        .aperture_definitions
                        .get(&self.current_aperture)
                        .context("Aperture was never equipped.")?;

                    match aperture {
                        ApertureTemplate::Circle {
                            diameter,
                            hole_diameter,
                        } => Shape::circle(
                            self.calculate_transformation_matrix(),
                            &mut gerber_file.shapes,
                            self.polarity,
                            self.current_point,
                            *diameter,
                            *hole_diameter,
                        ),
                        ApertureTemplate::Rectangle {
                            width,
                            height,
                            hole_diameter,
                        } => Shape::rectangle(
                            self.calculate_transformation_matrix(),
                            &mut gerber_file.shapes,
                            self.polarity,
                            self.current_point,
                            *width,
                            *height,
                            *hole_diameter,
                        ),
                        ApertureTemplate::Obround {
                            width,
                            height,
                            hole_diameter,
                        } => Shape::obround(
                            self.calculate_transformation_matrix(),
                            &mut gerber_file.shapes,
                            self.polarity,
                            self.current_point,
                            *width,
                            *height,
                            *hole_diameter,
                        ),
                        ApertureTemplate::Polygon {
                            diameter,
                            num_vertices,
                            rotation,
                            hole_diameter,
                        } => Shape::polygon(
                            self.calculate_transformation_matrix(),
                            &mut gerber_file.shapes,
                            self.polarity,
                            self.current_point,
                            *diameter,
                            *num_vertices,
                            rotation.deref().unwrap_or(0.0),
                            *hole_diameter,
                        )?,
                        ApertureTemplate::Macro { name, arguments } => {
                            let aperture_macro = self
                                .aperture_macros
                                .get(name.fragment())
                                .context("Macro was not defined.")?;

                            let mut shapes = Vec::new();

                            let result = Shape::aperture_macro(
                                self.calculate_transformation_matrix(),
                                &self.format,
                                &mut shapes,
                                self.current_point,
                                aperture_macro,
                                arguments,
                            );

                            // Deferring the error handling until after we push the shape lets us get more into the debug render.
                            gerber_file.aperture_macro_flashes.push(shapes);
                            result?;
                        }
                    }
                }
                Operation::LinearMode => self.draw_mode = DrawMode::Linear,
                Operation::ClockwiseMode => self.draw_mode = DrawMode::Clockwise,
                Operation::CounterClockwiseMode => self.draw_mode = DrawMode::CounterClockwise,
            },
            GerberCommand::MultiQuadrantMode => {
                // We don't support any other arc mode so this doesn't need to actually do anything.
            }
            GerberCommand::Region(operations) => {
                let mut operations = operations.into_iter();

                if let Some(Operation::Move { x, y }) =
                    operations.next().map(|context| context.operation)
                {
                    if let Some(x) = x {
                        self.current_point.x = self.format.internalize_coordinate_from_span(x)?;
                    }

                    if let Some(y) = y {
                        self.current_point.y = self.format.internalize_coordinate_from_span(y)?;
                    }
                } else {
                    bail!("Region must start with a move command.");
                }

                let mut shape = Shape {
                    polarity: self.polarity,
                    segments: vec![Segment::Move {
                        target: self.current_point,
                    }],
                };

                for operation in operations {
                    let location_info = operation.location_info();
                    self.process_operation(operation.operation, &mut shape)
                        .with_context(move || {
                            format!(
                                "error processing operation: {}:{}",
                                path.to_string_lossy(),
                                location_info
                            )
                        })
                        .context("error processing region")?;
                }

                gerber_file.shapes.push(shape);
            }
            GerberCommand::StepAndRepeat {
                iterations,
                delta,
                commands,
            } => bail!("Unimplemented 6."),
            GerberCommand::UnitMode(new_mode) => {
                self.format.unit_mode = new_mode;
            }
            GerberCommand::FormatSpecification {
                integer_digits,
                decimal_digits,
            } => {
                self.format.integer_digits = integer_digits;
                self.format.decimal_digits = decimal_digits;
            }
            GerberCommand::ApertureDefine { identity, template } => {
                if identity >= 10 {
                    self.aperture_definitions.insert(identity, template);
                } else {
                    bail!("Aperiture identities ")
                }
            }
            GerberCommand::ApertureMacro { name, content } => {
                self.aperture_macros.insert(name.fragment(), content);
            }

            GerberCommand::LoadPolarity(polarity) => self.polarity = polarity,
            GerberCommand::LoadMirroring(mirroring) => self.mirroring = mirroring,
            GerberCommand::LoadRotation(rotation) => self.rotation = rotation,
            GerberCommand::LoadScaling(scaling) => self.scaling = scaling,
            GerberCommand::ApertureBlock(_, _) => bail!("Unimplemented 7."),
        }

        Ok(())
    }

    fn process_operation(&mut self, operation: Operation, shape: &mut Shape) -> Result<()> {
        match operation {
            Operation::Plot { x, y, i, j } => {
                let mut next_point = self.current_point;

                if let Some(x) = x {
                    next_point.x = self.format.internalize_coordinate_from_span(x)?;
                }

                if let Some(y) = y {
                    next_point.y = self.format.internalize_coordinate_from_span(y)?;
                }

                let i = if let Some(i) = i {
                    Some(self.format.internalize_coordinate_from_span(i)?)
                } else {
                    None
                };

                let j = if let Some(j) = j {
                    Some(self.format.internalize_coordinate_from_span(j)?)
                } else {
                    None
                };

                match self.draw_mode {
                    DrawMode::Linear => {
                        shape.segments.push(Segment::Line { end: next_point });
                    }
                    DrawMode::Clockwise => shape.segments.push(Segment::ClockwiseCurve {
                        end: next_point,
                        diameter: Vector2::new(
                            i.context("i parameter missing")?,
                            j.context("j parameter missing")?,
                        )
                        .norm()
                            * 2.0,
                    }),
                    DrawMode::CounterClockwise => {
                        shape.segments.push(Segment::CounterClockwiseCurve {
                            end: next_point,
                            diameter: Vector2::new(
                                i.context("i parameter missing")?,
                                j.context("j parameter missing")?,
                            )
                            .norm()
                                * 2.0,
                        })
                    }
                }

                self.current_point = next_point;
            }
            Operation::Move { x, y } => {
                if let Some(x) = x {
                    self.current_point.x = self.format.internalize_coordinate_from_span(x)?;
                }

                if let Some(y) = y {
                    self.current_point.y = self.format.internalize_coordinate_from_span(y)?;
                }
            }
            Operation::LinearMode => self.draw_mode = DrawMode::Linear,
            Operation::ClockwiseMode => self.draw_mode = DrawMode::Clockwise,
            Operation::CounterClockwiseMode => self.draw_mode = DrawMode::CounterClockwise,
            _ => bail!("Illegal operation in region."),
        }

        Ok(())
    }

    fn calculate_transformation_matrix(&self) -> Matrix2<f32> {
        // Apply mirroring
        let matrix = match self.mirroring {
            MirroringMode::None => Matrix2::identity(),
            MirroringMode::X => Matrix2::from_diagonal(&Vector2::new(-1.0, 1.0)),
            MirroringMode::Y => Matrix2::from_diagonal(&Vector2::new(1.0, -1.0)),
            MirroringMode::XAndY => Matrix2::from_diagonal(&Vector2::new(-1.0, -1.0)),
        };

        let matrix = matrix * Rotation2::new(self.rotation.to_radians()).matrix();

        matrix * self.scaling
    }
}
