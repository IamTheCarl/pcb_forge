use anyhow::{anyhow, bail, Context, Result};
use geo::{BooleanOps, BoundingRect, Contains, Coord, MultiPolygon, Polygon};
use geo_offset::Offset;
use nalgebra::{Matrix2, Rotation2, Vector2};
use progress_bar::*;
use std::{collections::HashMap, fs, ops::Deref, path::Path};
use svg_composer::{
    document::Document as SvgDocument,
    element::{
        attributes::{Color, ColorName, Paint, Size},
        path::command::End,
        Element, Path as SvgPath,
    },
};
use uom::si::length::{mil, millimeter, Length};

use crate::{
    forge_file::LineSelection,
    gcode_generation::{GCodeConfig, GCommand, MovementType, Tool, ToolSelection},
    geometry::{ArchDirection, Segment, Shape, ShapeConfiguration},
    parsing::{
        gerber::{
            parse_gerber_file, ApertureTemplate, Attribute, GerberCommand, MacroContent,
            MirroringMode, Operation, Polarity, Span,
        },
        UnitMode,
    },
};

#[derive(Debug, Default)]
pub struct GerberFile {
    shapes: Vec<Shape>,
    aperture_macro_flashes: Vec<Vec<Shape>>,
}

impl GerberFile {
    fn iter_all_shapes(&self) -> impl Iterator<Item = &Shape> {
        self.shapes
            .iter()
            .chain(self.aperture_macro_flashes.iter().flatten())
    }

    pub fn generate_gcode(
        &self,
        config: GCodeConfig,
        generate_infill: bool,
        line_selection: LineSelection,
        invert: bool,
    ) -> Result<()> {
        log::info!("Simplifying geometry.");
        let distance_per_step = config.job_config.distance_per_step.get::<millimeter>();

        let mut polygon = Vec::new();

        // Iterate all our shapes *and* the macro flashes within.
        for shape in self.iter_all_shapes() {
            polygon.push(shape.convert_to_geo_polygon(distance_per_step));
        }

        let polygon = polygon
            .iter()
            .fold(MultiPolygon::new(vec![]), |previous, polygon| {
                let polygon = MultiPolygon::new(vec![polygon.clone()]);
                previous.union(&polygon)
            });
        // let polygon = MultiPolygon::new(polygon);

        let polygon = match line_selection {
            LineSelection::All => polygon,
            LineSelection::Inner => MultiPolygon::new(
                polygon
                    .0
                    .into_iter()
                    .flat_map(|polygon| {
                        polygon
                            .interiors()
                            .iter()
                            .cloned()
                            .map(|interior| Polygon::new(interior, vec![]))
                            .collect::<Vec<Polygon>>()
                    })
                    .collect(),
            ),
            LineSelection::Outer => polygon
                .0
                .into_iter()
                .map(|polygon| Polygon::new(polygon.exterior().clone(), vec![]))
                .collect(),
        };

        // Apply offsets from laser.
        let polygon = if invert {
            // No need for adjustment.
            polygon
        } else {
            polygon
                .offset(config.tool_config.diameter().get::<millimeter>() / 2.0)
                .map_err(|error| anyhow!("Failed to apply beam offset: {:?}", error))?
        };

        match config.job_config.tool_power {
            crate::config::machine::ToolConfig::Laser {
                laser_power,
                work_speed,
            } => {
                if let ToolSelection::Laser { laser } = config.tool_config {
                    config.commands.extend(
                        [
                            GCommand::EquipTool(Tool::Laser {
                                max_power: laser.max_power,
                            }),
                            GCommand::UnitMode(UnitMode::Metric),
                            GCommand::SetRapidTransverseSpeed(config.machine_config.jog_speed),
                            GCommand::SetWorkSpeed(work_speed),
                            GCommand::SetPower(laser_power),
                        ]
                        .iter()
                        .cloned(),
                    );
                } else {
                    bail!("Job was configured for a laser but selected tool is not a laser.");
                }
            }
            crate::config::machine::ToolConfig::EndMill {
                spindle_speed: spindle_rpm,
                max_cut_depth,
                plunge_speed,
                work_speed,
            } => {
                if let ToolSelection::Spindle { spindle, bit: _ } = config.tool_config {
                    config.commands.extend(
                        [
                            GCommand::EquipTool(Tool::Spindle {
                                max_spindle_speed: spindle.max_speed,
                                plunge_speed,
                                plunge_depth: max_cut_depth,
                            }),
                            GCommand::UnitMode(UnitMode::Metric),
                            GCommand::SetRapidTransverseSpeed(config.machine_config.jog_speed),
                            GCommand::SetWorkSpeed(work_speed),
                            GCommand::SetSpindleSpeed(spindle_rpm),
                        ]
                        .iter()
                        .cloned(),
                    );
                } else {
                    bail!("Job was configured for a spindle but selected tool is not a spindle.");
                }
            }
        }

        if let Some(init_gcode) = config.tool_config.init_gcode() {
            config.commands.push(GCommand::IncludeFile(
                config.include_file_search_directory.join(init_gcode),
            ));
        }

        // Start by generating GCode for the outlines.

        fn add_point_string<'a>(
            commands: &mut Vec<GCommand>,
            mut point_iter: impl Iterator<Item = &'a Coord<f64>>,
        ) {
            if let Some(first_point) = point_iter.next() {
                commands.push(GCommand::MoveTo {
                    target: (
                        Length::new::<millimeter>(first_point.x),
                        Length::new::<millimeter>(first_point.y),
                    ),
                })
            }

            for point in point_iter {
                commands.push(GCommand::Cut {
                    movement: MovementType::Linear,
                    target: (
                        Length::new::<millimeter>(point.x),
                        Length::new::<millimeter>(point.y),
                    ),
                })
            }
        }

        {
            let polygon = &polygon.0;
            for polygon in polygon.iter() {
                add_point_string(config.commands, polygon.exterior().0.iter());

                for interior in polygon.interiors() {
                    add_point_string(config.commands, interior.0.iter());
                }
            }
        }

        if generate_infill {
            // Now we generate the infill.
            log::info!("Generating infill.");
            let bounds = polygon
                .bounding_rect()
                .context("Could not compute bounds for PCB.")?;

            let (min_x, min_y, max_x, max_y) = (
                bounds.min().x + (config.tool_config.diameter() / 2.0).get::<millimeter>(),
                bounds.min().y + (config.tool_config.diameter() / 2.0).get::<millimeter>(),
                bounds.max().x,
                bounds.max().y,
            );

            struct InfillLine {
                start: Vector2<f64>,
                end: Vector2<f64>,
            }

            init_progress_bar(
                ((max_y - min_y) / (config.tool_config.diameter() / 2.0).get::<millimeter>()).ceil()
                    as usize,
            );
            set_progress_bar_action("Slicing", progress_bar::Color::Blue, Style::Bold);

            let mut lines = Vec::new();

            let mut y = min_y;
            while y < max_y {
                let mut x = min_x;
                let mut start = None;
                let mut end = None;

                while x < max_x {
                    let point = Coord { x, y };

                    if !polygon.contains(&point) ^ invert {
                        if start.is_none() {
                            start = Some(x);
                        }

                        end = Some(x);
                    } else if let (Some(start), Some(end)) = (start.take(), end.take()) {
                        lines.push(InfillLine {
                            start: Vector2::new(start, y),
                            end: Vector2::new(end, y),
                        });
                    }

                    x += (config.tool_config.diameter() / 2.0).get::<millimeter>();
                }

                y += (config.tool_config.diameter() / 2.0).get::<millimeter>();
                // println!("{}", (y - min_y) / (max_y - min_y));
                inc_progress_bar();
            }

            finalize_progress_bar();
            init_progress_bar(lines.len());
            set_progress_bar_action("Sorting", progress_bar::Color::Cyan, Style::Bold);

            enum LineSelection {
                None,
                Start(usize),
                End(usize),
            }

            let mut last_position = Vector2::new(min_x, min_y);

            while !lines.is_empty() {
                let mut last_distance = f64::INFINITY;
                let mut line_selection = LineSelection::None;

                for (line_index, line) in lines.iter().enumerate() {
                    let distance_to_start = (line.start - last_position).norm();
                    if distance_to_start < last_distance {
                        last_distance = distance_to_start;
                        line_selection = LineSelection::Start(line_index)
                    }

                    let distance_to_end = (line.end - last_position).norm();
                    if distance_to_end < last_distance {
                        last_distance = distance_to_end;
                        line_selection = LineSelection::End(line_index)
                    }
                }

                match line_selection {
                    LineSelection::None => unreachable!(),
                    LineSelection::Start(index) => {
                        let line = lines.remove(index);

                        config.commands.push(GCommand::MoveTo {
                            target: (
                                Length::new::<millimeter>(line.start.x),
                                Length::new::<millimeter>(line.start.y),
                            ),
                        });
                        config.commands.push(GCommand::Cut {
                            movement: MovementType::Linear,
                            target: (
                                Length::new::<millimeter>(line.end.x),
                                Length::new::<millimeter>(line.end.y),
                            ),
                        });

                        last_position = line.end;
                    }
                    LineSelection::End(index) => {
                        let line = lines.remove(index);

                        config.commands.push(GCommand::MoveTo {
                            target: (
                                Length::new::<millimeter>(line.end.x),
                                Length::new::<millimeter>(line.end.y),
                            ),
                        });
                        config.commands.push(GCommand::Cut {
                            movement: MovementType::Linear,
                            target: (
                                Length::new::<millimeter>(line.start.x),
                                Length::new::<millimeter>(line.start.y),
                            ),
                        });

                        last_position = line.start;
                    }
                }

                inc_progress_bar();
            }

            finalize_progress_bar();
        }

        if let Some(shutdown_gcode) = config.tool_config.shutdown_gcode() {
            config.commands.push(GCommand::IncludeFile(
                config.include_file_search_directory.join(shutdown_gcode),
            ));
        }

        config.commands.push(GCommand::EquipTool(Tool::None));

        Ok(())
    }

    pub fn debug_render(&self, svg: &mut SvgDocument, include_outline: bool) -> Result<()> {
        for (index, shape) in self.iter_all_shapes().enumerate() {
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

    pub fn calculate_bounds(&self) -> (f64, f64, f64, f64) {
        if !self.shapes.is_empty() {
            let mut min_x = f64::MAX;
            let mut min_y = f64::MAX;
            let mut max_x = f64::MIN;
            let mut max_y = f64::MIN;

            for shape in self.iter_all_shapes() {
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

    pub fn calculate_svg_bounds(&self) -> (f64, f64, f64, f64) {
        let (min_x, min_y, max_x, max_y) = self.calculate_bounds();
        (min_x, min_y, max_x - min_x, max_y - min_y)
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
    fn internalize_coordinate_from_span(&self, coordinate: Span) -> Result<f64> {
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

        let sign = integer.signum();
        let integer = integer.abs();

        // Combine.
        let new_position = sign as f64
            * (integer as f64 + (decimal as f64 / (10.0f64.powi(self.decimal_digits as i32))));

        // Convert to mm for internal representation.
        Ok(self.internalize_coordinate_from_float(new_position))
    }

    fn internalize_coordinate_from_float(&self, coordinate: f64) -> f64 {
        // Convert to mm for internal representation.
        match self.unit_mode {
            UnitMode::Metric => Length::<uom::si::SI<f64>, f64>::new::<millimeter>(coordinate),
            UnitMode::Imperial => Length::<uom::si::SI<f64>, f64>::new::<mil>(coordinate),
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

    current_point: Vector2<f64>,
    current_aperture: u32,
    draw_mode: DrawMode,
    format: Format,

    polarity: Polarity,
    mirroring: MirroringMode,
    rotation: f64,
    scaling: f64,
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
                Operation::Plot { x, y, i, j } => {
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
                            match self.draw_mode {
                                DrawMode::Linear => Shape::line(
                                    ShapeConfiguration {
                                        transform: self.calculate_transformation_matrix(),
                                        shapes: &mut gerber_file.shapes,
                                        polarity: self.polarity,
                                    },
                                    *diameter,
                                    self.current_point,
                                    next_point,
                                ),
                                DrawMode::Clockwise => {
                                    let (i, j) = (
                                        self.format.internalize_coordinate_from_span(
                                            i.context("I parameter is needed for arcs.")?,
                                        )?,
                                        self.format.internalize_coordinate_from_span(
                                            j.context("J parameter is needed for arcs.")?,
                                        )?,
                                    );
                                    let center = self.current_point + Vector2::new(i, j);

                                    Shape::arch(
                                        ShapeConfiguration {
                                            transform: self.calculate_transformation_matrix(),
                                            shapes: &mut gerber_file.shapes,
                                            polarity: self.polarity,
                                        },
                                        *diameter,
                                        center,
                                        self.current_point,
                                        next_point,
                                        ArchDirection::Clockwise,
                                    )
                                }
                                DrawMode::CounterClockwise => {
                                    let (i, j) = (
                                        self.format.internalize_coordinate_from_span(
                                            i.context("I parameter is needed for arcs.")?,
                                        )?,
                                        self.format.internalize_coordinate_from_span(
                                            j.context("J parameter is needed for arcs.")?,
                                        )?,
                                    );
                                    let center = self.current_point + Vector2::new(i, j);

                                    Shape::arch(
                                        ShapeConfiguration {
                                            transform: self.calculate_transformation_matrix(),
                                            shapes: &mut gerber_file.shapes,
                                            polarity: self.polarity,
                                        },
                                        *diameter,
                                        center,
                                        self.current_point,
                                        next_point,
                                        ArchDirection::CounterClockwise,
                                    )
                                }
                            };

                            self.current_point = next_point;
                        } else {
                            bail!("Circles used for line draws cannot have a hole in them.")
                        }
                    } else {
                        bail!("Only circles are supported for line draws.")
                    }
                }
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
                            ShapeConfiguration {
                                transform: self.calculate_transformation_matrix(),
                                shapes: &mut gerber_file.shapes,
                                polarity: self.polarity,
                            },
                            self.current_point,
                            *diameter,
                            *hole_diameter,
                        ),
                        ApertureTemplate::Rectangle {
                            width,
                            height,
                            hole_diameter,
                        } => Shape::rectangle(
                            ShapeConfiguration {
                                transform: self.calculate_transformation_matrix(),
                                shapes: &mut gerber_file.shapes,
                                polarity: self.polarity,
                            },
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
                            ShapeConfiguration {
                                transform: self.calculate_transformation_matrix(),
                                shapes: &mut gerber_file.shapes,
                                polarity: self.polarity,
                            },
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
                            ShapeConfiguration {
                                transform: self.calculate_transformation_matrix(),
                                shapes: &mut gerber_file.shapes,
                                polarity: self.polarity,
                            },
                            self.current_point,
                            *diameter,
                            *num_vertices,
                            rotation.deref().unwrap_or(0.0),
                            *hole_diameter,
                        ),
                        ApertureTemplate::Macro { name, arguments } => {
                            let aperture_macro = self
                                .aperture_macros
                                .get(name.fragment())
                                .context("Macro was not defined.")?;

                            let mut shapes = Vec::new();

                            let result = shape_from_aperture_macro(
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
                    starting_point: self.current_point,
                    segments: Vec::new(),
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
                        center: self.current_point
                            + Vector2::new(
                                i.context("i parameter missing")?,
                                j.context("j parameter missing")?,
                            ),
                    }),
                    DrawMode::CounterClockwise => {
                        shape.segments.push(Segment::CounterClockwiseCurve {
                            end: next_point,
                            center: self.current_point
                                + Vector2::new(
                                    i.context("i parameter missing")?,
                                    j.context("j parameter missing")?,
                                ),
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

    fn calculate_transformation_matrix(&self) -> Matrix2<f64> {
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

fn shape_from_aperture_macro(
    transform: Matrix2<f64>,
    format: &Format,
    shapes: &mut Vec<Shape>,
    position: Vector2<f64>,
    aperture_macro: &[MacroContent],
    arguments: &[f64],
) -> Result<()> {
    let position = transform * position;
    let mut variables: HashMap<u32, f64> = arguments
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
                let transform =
                    Rotation2::new(angle.evaluate(&variables)?.to_radians()).matrix() * transform;

                let center_position = transform
                    * Vector2::new(x.evaluate(&variables)?, y.evaluate(&variables)?)
                    + position;
                let diameter = diameter.evaluate(&variables)?;

                Shape::circle(
                    ShapeConfiguration {
                        transform,
                        shapes,
                        polarity: *exposure,
                    },
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
                let transform =
                    Rotation2::new(angle.evaluate(&variables)?.to_radians()).matrix() * transform;

                Shape::square_line(
                    ShapeConfiguration {
                        transform,
                        shapes,
                        polarity: *exposure,
                    },
                    width.evaluate(&variables)?,
                    Vector2::new(start_x.evaluate(&variables)?, start_y.evaluate(&variables)?)
                        + position,
                    Vector2::new(end_x.evaluate(&variables)?, end_y.evaluate(&variables)?)
                        + position,
                );
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
                let transform =
                    Rotation2::new(angle.evaluate(&variables)?.to_radians()).matrix() * transform;

                let mut coordinate_iter =
                    coordinates.iter().map(|(x, y)| -> Result<Vector2<f64>> {
                        let x = format.internalize_coordinate_from_float(x.evaluate(&variables)?);
                        let y = format.internalize_coordinate_from_float(y.evaluate(&variables)?);
                        Ok(transform * Vector2::new(x, y) + position)
                    });

                let starting_point = coordinate_iter
                    .next()
                    .context("Outline must have at least one point.")??;

                let segments = {
                    let mut segments = Vec::new();
                    for end in coordinate_iter {
                        let end = end?;
                        segments.push(Segment::Line { end });
                    }

                    segments
                };

                shapes.push(Shape {
                    polarity: *exposure,
                    starting_point,
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
