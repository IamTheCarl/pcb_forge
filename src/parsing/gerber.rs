use std::collections::HashMap;

use nalgebra::Vector2;
/// The Gerber specification can be found [here](https://www.ucamco.com/en/guest/downloads/gerber-format). The copy
/// that was used to create this parser is located [here](../../specifications/gerber-layer-format-specification-revision-2023-03_en.pdf)
///
/// Several structures and functions in this file will state page numbers referencing that document.
///
use nom::{
    branch::alt,
    bytes::complete::{tag, take_while, take_while1},
    character::complete::{char as nom_char, one_of},
    combinator::{cut, map, map_res, opt, value},
    error::ErrorKind,
    multi::{fold_many0, length_count, many0, separated_list1},
    sequence::{delimited, pair, preceded, separated_pair, terminated, tuple},
    IResult,
};
use nom_locate::LocatedSpan;
use thiserror::Error;

pub type Span<'a> = LocatedSpan<&'a str>;

#[derive(Clone)]
pub struct GerberCommandContext<'a> {
    pub command: GerberCommand<'a>,
    pub span: Span<'a>,
}

impl<'a> GerberCommandContext<'a> {
    pub fn location_info(&self) -> LocationInfo {
        LocationInfo {
            line: self.span.location_line(),
            column: self.span.get_utf8_column(),
        }
    }
}

impl<'a> std::fmt::Debug for GerberCommandContext<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let location_info = self.location_info();

        f.debug_struct("GerberCommandContext")
            .field("command", &self.command)
            .field("span", &location_info)
            .finish()
    }
}

#[derive(Debug)]
pub struct LocationInfo {
    pub line: u32,
    pub column: usize,
}

impl std::fmt::Display for LocationInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// Section 2.8
#[derive(Debug, Clone)]
pub enum GerberCommand<'a> {
    // Attributes.
    Attribute(Attribute<'a>),

    // Normal commands.
    Comment(Span<'a>), // G04 4.1
    SetAperture(u32),  // Dnn (nn≥10) 4.6

    Operation(Operation<'a>), // 4.7 and 4.8
    MultiQuadrantMode,        // G75 4.7.2

    Region(Vec<OperationContext<'a>>), // G36 4.10

    StepAndRepeat {
        // SR 4.12
        iterations: Vector2<u32>,
        delta: Vector2<f32>,
        commands: Vec<GerberCommandContext<'a>>,
    },

    // Extended commands.
    UnitMode(UnitMode), // MO 4.2.1
    FormatSpecification {
        // FS 4.2.2
        integer_digits: u32, // 1-6
        decimal_digits: u32, // 5-6
    },
    ApertureDefine {
        // AD 4.3
        identity: u32, // Min value of 10.
        template: ApertureTemplate<'a>,
    },
    ApertureMacro {
        // AM 4.5
        name: Span<'a>,
        content: Vec<MacroContent<'a>>,
    },

    LoadPolarity(Polarity),       // LP 4.9.2
    LoadMirroring(MirroringMode), // LM 4.9.3
    LoadRotation(f32),            // LR 4.9.4
    LoadScaling(f32),             // LS 4.9.5

    ApertureBlock(u32, Vec<GerberCommandContext<'a>>), // AB 4.11
}

#[derive(Debug, Clone)]
pub enum Operation<'a> {
    Plot {
        // D01 4.8.2
        x: Option<Span<'a>>,
        y: Option<Span<'a>>,
        i: Option<Span<'a>>,
        j: Option<Span<'a>>,
    },
    Move {
        // D02 4.8.3
        x: Option<Span<'a>>,
        y: Option<Span<'a>>,
    },
    Flash {
        // D03 4.8.4
        x: Option<Span<'a>>,
        y: Option<Span<'a>>,
    },

    LinearMode,           // G01 4.7.1
    ClockwiseMode,        // G02 4.7.2
    CounterClockwiseMode, // G03 4.7.2
}

#[derive(Clone)]
pub struct OperationContext<'a> {
    pub operation: Operation<'a>,
    pub span: Span<'a>,
}

impl<'a> OperationContext<'a> {
    pub fn location_info(&self) -> LocationInfo {
        LocationInfo {
            line: self.span.location_line(),
            column: self.span.get_utf8_column(),
        }
    }
}

impl<'a> std::fmt::Debug for OperationContext<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let location_info = self.location_info();

        f.debug_struct("OperationContext")
            .field("operation", &self.operation)
            .field("span", &location_info)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum Attribute<'a> {
    User {
        // Tf 5.1
        name: Span<'a>,
        values: Vec<Span<'a>>,
    },
    File {
        // TF 5.2
        name: Span<'a>,
        values: Vec<Span<'a>>,
    },
    Aperture {
        // TA 5.3
        name: Span<'a>,
        values: Vec<Span<'a>>,
    },
    Object {
        // TO 5.4
        name: Span<'a>,
        values: Vec<Span<'a>>,
    },
    Delete {
        // TD 5.5
        name: Option<Span<'a>>, // Setting to none means to delete all non-file attributes.
    },
}

#[derive(Debug, Clone, Copy)]
pub enum UnitMode {
    Metric,
    Imperial,
}

// Section 4.4
#[derive(Debug, Clone)]
pub enum ApertureTemplate<'a> {
    Circle {
        diameter: f32,
        hole_diameter: Option<f32>,
    },
    Rectangle {
        width: f32,
        height: f32,
        hole_diameter: Option<f32>,
    },
    Obround {
        width: f32,
        height: f32,
        hole_diameter: Option<f32>,
    },
    Polygon {
        diameter: f32,
        num_vertices: u32,
        rotation: Option<f32>,
        hole_diameter: Option<f32>,
    },
    Macro {
        name: Span<'a>,
        arguments: Vec<f32>,
    },
}

// Section 4.5
#[derive(Debug, Clone)]
pub enum MacroContent<'a> {
    Comment(Span<'a>),
    Circle {
        exposure: Polarity,
        diameter: MacroExpression,
        center_position: (MacroExpression, MacroExpression),
        angle: MacroExpression,
    },
    VectorLine {
        exposure: Polarity,
        width: MacroExpression,
        start: (MacroExpression, MacroExpression),
        end: (MacroExpression, MacroExpression),
        angle: MacroExpression,
    },
    CenterLine {
        exposure: Polarity,
        size: (MacroExpression, MacroExpression),
        center: (MacroExpression, MacroExpression),
        angle: MacroExpression,
    },
    Outline {
        exposure: Polarity,
        coordinates: Vec<(MacroExpression, MacroExpression)>,
        angle: MacroExpression,
    },
    Polygon {
        exposure: Polarity,
        num_vertices: u32, // 3..=12
        center_position: (MacroExpression, MacroExpression),
        diameter: MacroExpression,
        angle: MacroExpression,
    },
    Thermal {
        center_point: (MacroExpression, MacroExpression),
        outer_diameter: MacroExpression,
        inner_diameter: MacroExpression,
        gap_thickness: MacroExpression, // < sqrt(outer_diameter)
        angle: MacroExpression,
    },
    VariableDefinition {
        variable: u32,
        expression: MacroExpression,
    },
}

#[derive(Debug, Error)]
pub enum MacroExpressionEvaluationError {
    #[error("Undefined variable: {0}")]
    UndefinedVariable(u32),
}

/// Section 4.5.4.2
#[derive(Debug, Clone)]
pub enum MacroExpression {
    UnaryPlus(MacroTerm),
    UnaryMinus(MacroTerm),
    Addition(Box<Self>, MacroTerm),
    Subtraction(Box<Self>, MacroTerm),
    Term(MacroTerm),
}

impl MacroExpression {
    pub fn evaluate(
        &self,
        arguments: &HashMap<u32, f32>,
    ) -> Result<f32, MacroExpressionEvaluationError> {
        match self {
            MacroExpression::UnaryPlus(term) => term.evaluate(arguments),
            MacroExpression::UnaryMinus(term) => Ok(-term.evaluate(arguments)?),
            MacroExpression::Addition(a, b) => Ok(a.evaluate(arguments)? + b.evaluate(arguments)?),
            MacroExpression::Subtraction(a, b) => {
                Ok(a.evaluate(arguments)? - b.evaluate(arguments)?)
            }
            MacroExpression::Term(term) => term.evaluate(arguments),
        }
    }
}

#[derive(Debug, Clone)]
pub enum MacroTerm {
    Multiply(Box<Self>, MacroFactor),
    Divide(Box<Self>, MacroFactor),
    Factor(MacroFactor),
}

impl MacroTerm {
    pub fn evaluate(
        &self,
        arguments: &HashMap<u32, f32>,
    ) -> Result<f32, MacroExpressionEvaluationError> {
        match self {
            MacroTerm::Multiply(a, b) => Ok(a.evaluate(arguments)? * b.evaluate(arguments)?),
            MacroTerm::Divide(a, b) => Ok(a.evaluate(arguments)? / b.evaluate(arguments)?),
            MacroTerm::Factor(factor) => factor.evaluate(arguments),
        }
    }
}

#[derive(Debug, Clone)]
pub enum MacroFactor {
    Const(f32), // Should never be negative.
    Variable(u32),
    Parenthesis(Box<MacroExpression>),
}

impl MacroFactor {
    pub fn evaluate(
        &self,
        arguments: &HashMap<u32, f32>,
    ) -> Result<f32, MacroExpressionEvaluationError> {
        match self {
            MacroFactor::Const(value) => Ok(*value),
            MacroFactor::Variable(index) => arguments
                .get(index)
                .copied()
                .ok_or(MacroExpressionEvaluationError::UndefinedVariable(*index)),
            MacroFactor::Parenthesis(block) => block.evaluate(arguments),
        }
    }
}

/// Section 4.9.2
#[derive(Debug, Clone, Copy)]
pub enum Polarity {
    Clear,
    Dark,
}
impl Polarity {
    pub(crate) fn inverse(&self) -> Polarity {
        match self {
            Polarity::Clear => Polarity::Dark,
            Polarity::Dark => Polarity::Clear,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum MirroringMode {
    None,
    X,
    Y,
    XAndY,
}

pub fn parse_gerber_file(input: Span) -> IResult<Span, Vec<GerberCommandContext>> {
    terminated(many0(delimited(space, parse_command, space)), end_of_file)(input)
}

pub fn end_of_file(input: Span) -> IResult<Span, Span> {
    // M02 4.13
    tag("M02*")(input)
}

fn parse_command(input: Span) -> IResult<Span, GerberCommandContext> {
    map(
        alt((parse_extended_command, parse_normal_command)),
        |command| GerberCommandContext {
            command,
            span: input,
        },
    )(input)
}

// Normal commands.

fn parse_normal_command(input: Span) -> IResult<Span, GerberCommand> {
    alt((
        parse_comment,
        parse_set_aperture,
        map(parse_operation, GerberCommand::Operation),
        parse_multi_quadrant_mode,
        parse_region,
        parse_step_and_repeat,
    ))(input)
}

fn parse_comment(input: Span) -> IResult<Span, GerberCommand> {
    map(
        delimited(tag("G04"), parse_string, nom_char('*')),
        GerberCommand::Comment,
    )(input)
}

fn parse_set_aperture(input: Span) -> IResult<Span, GerberCommand> {
    map(
        delimited(nom_char('D'), parse_unsigned_integer, nom_char('*')),
        GerberCommand::SetAperture,
    )(input)
}

fn parse_operation(input: Span) -> IResult<Span, Operation> {
    alt((
        parse_set_linear_mode,
        parse_set_clockwise_mode,
        parse_set_counter_clockwise_mode,
        parse_plot,
        parse_move,
        parse_flash,
    ))(input)
}

fn parse_operation_with_context(input: Span) -> IResult<Span, OperationContext> {
    map(parse_operation, |operation| OperationContext {
        operation,
        span: input,
    })(input)
}

fn parse_plot(input: Span) -> IResult<Span, Operation> {
    map(
        terminated(
            tuple((
                opt(preceded(nom_char('X'), parse_integer)),
                opt(preceded(nom_char('Y'), parse_integer)),
                opt(preceded(nom_char('I'), parse_integer)),
                opt(preceded(nom_char('J'), parse_integer)),
            )),
            tag("D01*"),
        ),
        |(x, y, i, j)| Operation::Plot { x, y, i, j },
    )(input)
}

fn parse_move(input: Span) -> IResult<Span, Operation> {
    map(
        terminated(
            tuple((
                opt(preceded(nom_char('X'), parse_integer)),
                opt(preceded(nom_char('Y'), parse_integer)),
            )),
            tag("D02*"),
        ),
        |(x, y)| Operation::Move { x, y },
    )(input)
}

fn parse_flash(input: Span) -> IResult<Span, Operation> {
    map(
        terminated(
            tuple((
                opt(preceded(nom_char('X'), parse_integer)),
                opt(preceded(nom_char('Y'), parse_integer)),
            )),
            tag("D03*"),
        ),
        |(x, y)| Operation::Flash { x, y },
    )(input)
}

fn parse_set_linear_mode(input: Span) -> IResult<Span, Operation> {
    value(Operation::LinearMode, terminated(tag("G01"), nom_char('*')))(input)
}

fn parse_set_clockwise_mode(input: Span) -> IResult<Span, Operation> {
    value(
        Operation::ClockwiseMode,
        terminated(tag("G02"), nom_char('*')),
    )(input)
}

fn parse_set_counter_clockwise_mode(input: Span) -> IResult<Span, Operation> {
    value(
        Operation::CounterClockwiseMode,
        terminated(tag("G03"), nom_char('*')),
    )(input)
}

fn parse_multi_quadrant_mode(input: Span) -> IResult<Span, GerberCommand> {
    value(
        GerberCommand::MultiQuadrantMode,
        terminated(tag("G75"), nom_char('*')),
    )(input)
}

fn parse_region(input: Span) -> IResult<Span, GerberCommand> {
    map(
        delimited(
            tag("G36*"),
            many0(delimited(space, parse_operation_with_context, space)),
            tag("G37*"),
        ),
        GerberCommand::Region,
    )(input)
}

fn parse_step_and_repeat(input: Span) -> IResult<Span, GerberCommand> {
    map(
        terminated(
            pair(
                delimited(
                    tag("SR"),
                    tuple((
                        preceded(nom_char('X'), parse_unsigned_integer),
                        preceded(nom_char('Y'), parse_unsigned_integer),
                        preceded(nom_char('I'), parse_decimal),
                        preceded(nom_char('J'), parse_decimal),
                    )),
                    tag("*%"),
                ),
                many0(delimited(space, parse_command, space)),
            ),
            tag("%SR*"),
        ),
        |((x, y, i, j), commands)| GerberCommand::StepAndRepeat {
            iterations: Vector2::new(x, y),
            delta: Vector2::new(i, j),
            commands,
        },
    )(input)
}

// Extended commands.

fn parse_extended_command(input: Span) -> IResult<Span, GerberCommand> {
    delimited(
        nom_char('%'),
        cut(alt((
            parse_unit_mode,
            parse_format_specification,
            parse_aperture_define,
            parse_aperture_macro,
            parse_load_polarity,
            parse_load_mirroring,
            parse_load_rotation,
            parse_load_scaling,
            parse_aperture_block,
            parse_delete_attribute,
            parse_attribute,
        ))),
        cut(nom_char('%')),
    )(input)
}

fn parse_unit_mode(input: Span) -> IResult<Span, GerberCommand> {
    delimited(
        tag("MO"),
        cut(alt((
            value(GerberCommand::UnitMode(UnitMode::Metric), tag("MM")),
            value(GerberCommand::UnitMode(UnitMode::Imperial), tag("IN")),
        ))),
        cut(nom_char('*')),
    )(input)
}

fn parse_attribute(input: Span) -> IResult<Span, GerberCommand> {
    fn parse_attribute(input: Span) -> IResult<Span, (Span, Vec<Span>)> {
        pair(parse_field, many0(preceded(nom_char(','), parse_field)))(input)
    }

    let parse_file_attribute = map(
        delimited(tag("TF."), parse_attribute, cut(nom_char('*'))),
        |(name, values)| GerberCommand::Attribute(Attribute::File { name, values }),
    );
    let parse_aperture_attribute = map(
        delimited(tag("TA."), parse_attribute, cut(nom_char('*'))),
        |(name, values)| GerberCommand::Attribute(Attribute::Aperture { name, values }),
    );
    let parse_object_attribute = map(
        delimited(tag("TO."), parse_attribute, cut(nom_char('*'))),
        |(name, values)| GerberCommand::Attribute(Attribute::Object { name, values }),
    );
    let parse_user_attribute = map(
        terminated(parse_attribute, cut(nom_char('*'))),
        |(name, values)| GerberCommand::Attribute(Attribute::User { name, values }),
    );

    alt((
        parse_file_attribute,
        parse_aperture_attribute,
        parse_object_attribute,
        parse_user_attribute,
    ))(input)
}

fn parse_delete_attribute(input: Span) -> IResult<Span, GerberCommand> {
    map(
        delimited(tag("TD"), opt(parse_field), cut(nom_char('*'))),
        |name| GerberCommand::Attribute(Attribute::Delete { name }),
    )(input)
}

fn parse_format_specification(input: Span) -> IResult<Span, GerberCommand> {
    fn parse_coordinate_digits(input: Span) -> IResult<Span, (u32, u32)> {
        let (input, digit) = map_res(one_of("123456"), |digit: char| {
            digit.to_string().parse::<u32>()
        })(input)?;

        let (input, fraction) =
            map_res(one_of("56"), |digit: char| digit.to_string().parse::<u32>())(input)?;

        Ok((input, (digit, fraction)))
    }

    let (input, (x, y)) = delimited(
        tag("FSLA"),
        cut(tuple((
            preceded(nom_char('X'), parse_coordinate_digits),
            preceded(nom_char('Y'), parse_coordinate_digits),
        ))),
        cut(nom_char('*')),
    )(input)?;

    if x == y {
        Ok((
            input,
            GerberCommand::FormatSpecification {
                integer_digits: x.0,
                decimal_digits: x.1,
            },
        ))
    } else {
        nom::error::context(
            "X and Y settings for format specification do not match.",
            |input| {
                Err(nom::Err::Failure(nom::error::Error::new(
                    input,
                    ErrorKind::AlphaNumeric,
                )))
            },
        )(input)
    }
}

fn parse_aperture_define(input: Span) -> IResult<Span, GerberCommand> {
    map(
        delimited(
            tag("AD"),
            tuple((
                preceded(nom_char('D'), parse_unsigned_integer),
                alt((
                    map(
                        preceded(
                            tag("C,"),
                            pair(parse_decimal, opt(preceded(nom_char('X'), parse_decimal))),
                        ),
                        |(diameter, hole_diameter)| ApertureTemplate::Circle {
                            diameter,
                            hole_diameter,
                        },
                    ),
                    map(
                        preceded(
                            tag("R,"),
                            tuple((
                                parse_decimal,
                                preceded(nom_char('X'), parse_decimal),
                                opt(preceded(nom_char('X'), parse_decimal)),
                            )),
                        ),
                        |(width, height, hole_diameter)| ApertureTemplate::Rectangle {
                            width,
                            height,
                            hole_diameter,
                        },
                    ),
                    map(
                        preceded(
                            tag("O,"),
                            tuple((
                                parse_decimal,
                                preceded(nom_char('X'), parse_decimal),
                                opt(preceded(nom_char('X'), parse_decimal)),
                            )),
                        ),
                        |(width, height, hole_diameter)| ApertureTemplate::Obround {
                            width,
                            height,
                            hole_diameter,
                        },
                    ),
                    map(
                        preceded(
                            tag("P,"),
                            tuple((
                                parse_decimal,
                                preceded(nom_char('X'), parse_unsigned_integer),
                                opt(preceded(nom_char('X'), parse_decimal)), // If the first one fails, there's no way the second one will succeed.
                                opt(preceded(nom_char('X'), parse_decimal)), // That's okay.
                            )),
                        ),
                        |(diameter, num_vertices, rotation, hole_diameter)| {
                            ApertureTemplate::Polygon {
                                diameter,
                                num_vertices,
                                rotation,
                                hole_diameter,
                            }
                        },
                    ),
                    map(
                        pair(
                            terminated(parse_name, nom_char(',')),
                            separated_list1(nom_char('X'), parse_decimal),
                        ),
                        |(name, arguments)| ApertureTemplate::Macro { name, arguments },
                    ),
                )),
            )),
            nom_char('*'),
        ),
        |(identity, template)| GerberCommand::ApertureDefine { identity, template },
    )(input)
}

fn parse_aperture_macro(input: Span) -> IResult<Span, GerberCommand> {
    fn parse_body(input: Span) -> IResult<Span, Vec<MacroContent>> {
        fn parse_block(input: Span) -> IResult<Span, MacroContent> {
            fn parse_variable(input: Span) -> IResult<Span, u32> {
                // ` not 100% sure but I think this may accept some values that are not allowed. It doesn't quite match up with the PEG document.
                preceded(nom_char('$'), cut(parse_unsigned_integer))(input)
            }

            fn parse_expression(input: Span) -> IResult<Span, MacroExpression> {
                fn parse_term(input: Span) -> IResult<Span, MacroTerm> {
                    let (input, first_factor) = parse_factor(input)?;

                    #[derive(Clone)]
                    enum Operator {
                        Multiply,
                        Divide,
                    }

                    fold_many0(
                        pair(
                            alt((
                                value(Operator::Multiply, nom_char('*')),
                                value(Operator::Divide, nom_char('/')),
                            )),
                            parse_factor,
                        ),
                        move || MacroTerm::Factor(first_factor.clone()),
                        |term, (operator, factor)| match operator {
                            Operator::Multiply => MacroTerm::Multiply(Box::new(term), factor),
                            Operator::Divide => MacroTerm::Divide(Box::new(term), factor),
                        },
                    )(input)
                }

                fn parse_factor(input: Span) -> IResult<Span, MacroFactor> {
                    alt((
                        map(
                            delimited(nom_char('('), parse_expression, nom_char(')')),
                            |expression| MacroFactor::Parenthesis(Box::new(expression)),
                        ),
                        map(parse_variable, MacroFactor::Variable),
                        map(parse_unsigned_decimal, |decimal| {
                            debug_assert!(decimal >= 0.0);
                            MacroFactor::Const(decimal)
                        }),
                    ))(input)
                }

                // FIXME if there is a unary operator at the start, this will probably malfunction.
                let (input, first_term) = parse_term(input)?;

                #[derive(Clone)]
                enum Operator {
                    Addition,
                    Subtraction,
                }

                alt((
                    map(
                        preceded(nom_char('-'), parse_term),
                        MacroExpression::UnaryMinus,
                    ),
                    map(
                        preceded(nom_char('+'), parse_term),
                        MacroExpression::UnaryPlus,
                    ),
                    fold_many0(
                        pair(
                            alt((
                                value(Operator::Addition, nom_char('+')),
                                value(Operator::Subtraction, nom_char('-')),
                            )),
                            parse_term,
                        ),
                        move || MacroExpression::Term(first_term.clone()),
                        |term, (operator, factor)| match operator {
                            Operator::Addition => MacroExpression::Addition(Box::new(term), factor),
                            Operator::Subtraction => {
                                MacroExpression::Subtraction(Box::new(term), factor)
                            }
                        },
                    ),
                    map(parse_term, MacroExpression::Term),
                ))(input)
            }

            fn parse_primitive(input: Span) -> IResult<Span, MacroContent> {
                fn parse_exposure(input: Span) -> IResult<Span, Polarity> {
                    alt((
                        value(Polarity::Dark, nom_char('1')),
                        value(Polarity::Clear, nom_char('0')),
                    ))(input)
                }

                fn parse_comment(input: Span) -> IResult<Span, MacroContent> {
                    map(
                        preceded(nom_char('0'), take_while(|c| c != '*')),
                        MacroContent::Comment,
                    )(input)
                }

                fn comma(input: Span) -> IResult<Span, char> {
                    nom_char(',')(input)
                }

                fn parse_circle(input: Span) -> IResult<Span, MacroContent> {
                    map(
                        preceded(
                            nom_char('1'),
                            tuple((
                                preceded(comma, parse_exposure),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                opt(preceded(comma, parse_expression)),
                            )),
                        ),
                        |(exposure, diameter, x, y, rotation)| MacroContent::Circle {
                            exposure,
                            diameter,
                            center_position: (x, y),
                            angle: rotation.unwrap_or(MacroExpression::Term(MacroTerm::Factor(
                                MacroFactor::Const(0.0),
                            ))),
                        },
                    )(input)
                }

                fn parse_line(input: Span) -> IResult<Span, MacroContent> {
                    map(
                        preceded(
                            tag("20"),
                            tuple((
                                preceded(comma, parse_exposure),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                opt(preceded(comma, parse_expression)),
                            )),
                        ),
                        |(exposure, width, start_x, start_y, end_x, end_y, rotation)| {
                            MacroContent::VectorLine {
                                exposure,
                                width,
                                start: (start_x, start_y),
                                end: (end_x, end_y),
                                angle: rotation.unwrap_or(MacroExpression::Term(
                                    MacroTerm::Factor(MacroFactor::Const(0.0)),
                                )),
                            }
                        },
                    )(input)
                }

                fn parse_center_line(input: Span) -> IResult<Span, MacroContent> {
                    map(
                        preceded(
                            tag("21"),
                            tuple((
                                preceded(comma, parse_exposure),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                opt(preceded(comma, parse_expression)),
                            )),
                        ),
                        |(exposure, width, height, x, y, rotation)| MacroContent::CenterLine {
                            exposure,
                            size: (width, height),
                            center: (x, y),
                            angle: rotation.unwrap_or(MacroExpression::Term(MacroTerm::Factor(
                                MacroFactor::Const(0.0),
                            ))),
                        },
                    )(input)
                }

                fn parse_outline(input: Span) -> IResult<Span, MacroContent> {
                    map(
                        preceded(
                            nom_char('4'),
                            tuple((
                                preceded(comma, parse_exposure),
                                length_count(
                                    map(preceded(comma, parse_unsigned_integer), |count| count + 1),
                                    pair(
                                        preceded(comma, parse_expression),
                                        preceded(comma, parse_expression),
                                    ),
                                ),
                                opt(preceded(comma, parse_expression)),
                            )),
                        ),
                        |(exposure, coordinates, rotation)| MacroContent::Outline {
                            exposure,
                            coordinates,
                            angle: rotation.unwrap_or(MacroExpression::Term(MacroTerm::Factor(
                                MacroFactor::Const(0.0),
                            ))),
                        },
                    )(input)
                }

                fn parse_polygon(input: Span) -> IResult<Span, MacroContent> {
                    map(
                        preceded(
                            nom_char('5'),
                            tuple((
                                preceded(comma, parse_exposure),
                                preceded(comma, parse_unsigned_integer),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                opt(preceded(comma, parse_expression)),
                            )),
                        ),
                        |(exposure, num_vertices, x, y, diameter, rotation)| {
                            MacroContent::Polygon {
                                exposure,
                                num_vertices,
                                center_position: (x, y),
                                diameter,
                                angle: rotation.unwrap_or(MacroExpression::Term(
                                    MacroTerm::Factor(MacroFactor::Const(0.0)),
                                )),
                            }
                        },
                    )(input)
                }

                fn parse_thermal(input: Span) -> IResult<Span, MacroContent> {
                    map(
                        preceded(
                            nom_char('7'),
                            tuple((
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                preceded(comma, parse_expression),
                                opt(preceded(comma, parse_expression)),
                            )),
                        ),
                        |(x, y, outer_diameter, inner_diameter, gap_thickness, rotation)| {
                            MacroContent::Thermal {
                                center_point: (x, y),
                                outer_diameter,
                                inner_diameter,
                                gap_thickness,
                                angle: rotation.unwrap_or(MacroExpression::Term(
                                    MacroTerm::Factor(MacroFactor::Const(0.0)),
                                )),
                            }
                        },
                    )(input)
                }

                terminated(
                    alt((
                        parse_comment,
                        parse_circle,
                        parse_line,
                        parse_center_line,
                        parse_outline,
                        parse_polygon,
                        parse_thermal,
                    )),
                    nom_char('*'),
                )(input)
            }

            fn parse_variable_define(input: Span) -> IResult<Span, MacroContent> {
                map(
                    separated_pair(parse_variable, nom_char('='), parse_expression),
                    |(variable, expression)| MacroContent::VariableDefinition {
                        variable,
                        expression,
                    },
                )(input)
            }

            alt((parse_primitive, parse_variable_define))(input)
        }

        many0(terminated(parse_block, space))(input)
    }

    preceded(
        tag("AM"),
        cut(map(
            pair(
                terminated(parse_name, pair(nom_char('*'), space)),
                parse_body,
            ),
            |(name, content)| GerberCommand::ApertureMacro { name, content },
        )),
    )(input)
}

fn parse_load_polarity(input: Span) -> IResult<Span, GerberCommand> {
    map(
        terminated(
            preceded(
                tag("LP"),
                cut(alt((
                    value(Polarity::Clear, nom_char('C')),
                    value(Polarity::Dark, nom_char('D')),
                ))),
            ),
            nom_char('*'),
        ),
        GerberCommand::LoadPolarity,
    )(input)
}

fn parse_load_mirroring(input: Span) -> IResult<Span, GerberCommand> {
    map(
        terminated(
            preceded(
                tag("LM"),
                cut(alt((
                    value(MirroringMode::None, nom_char('N')),
                    value(MirroringMode::XAndY, tag("XY")),
                    value(MirroringMode::X, nom_char('X')),
                    value(MirroringMode::Y, nom_char('Y')),
                ))),
            ),
            nom_char('*'),
        ),
        GerberCommand::LoadMirroring,
    )(input)
}

fn parse_load_rotation(input: Span) -> IResult<Span, GerberCommand> {
    map(
        terminated(preceded(tag("LR"), parse_decimal), nom_char('*')),
        GerberCommand::LoadRotation,
    )(input)
}

fn parse_load_scaling(input: Span) -> IResult<Span, GerberCommand> {
    map(
        terminated(preceded(tag("LS"), parse_decimal), nom_char('*')),
        GerberCommand::LoadScaling,
    )(input)
}

fn parse_aperture_block(input: Span) -> IResult<Span, GerberCommand> {
    map(
        terminated(
            pair(
                delimited(tag("ABD"), parse_unsigned_integer, nom_char('*')),
                many0(delimited(space, parse_command, space)),
            ),
            tag("AB*"),
        ),
        |(block_id, content)| GerberCommand::ApertureBlock(block_id, content),
    )(input)
}

// Primitive parsing.

fn parse_unsigned_integer(input: Span) -> IResult<Span, u32> {
    map_res(take_while1(|c: char| c.is_ascii_digit()), |digits: Span| {
        digits.fragment().parse::<u32>()
    })(input)
}

// positive_integer =       /[0-9]*[1-9][0-9]*/;

fn parse_integer(input: Span) -> IResult<Span, Span> {
    // integer          =  /[+-]?[0-9]+/;
    take_while1(|c: char| c.is_ascii_digit() | matches!(c, '+' | '-'))(input)
}

fn parse_unsigned_decimal(input: Span) -> IResult<Span, f32> {
    // unsigned_decimal =      /((([0-9]+)(\.[0-9]*)?)|(\.[0-9]+))/;
    map_res(
        take_while(|c| matches!(c, '.' | '0'..='9')), // Intentionally no + or - sign in there.
        move |number: Span| number.fragment().parse::<f32>(),
    )(input)
}

fn parse_decimal(input: Span) -> IResult<Span, f32> {
    // decimal          = /[+-]?((([0-9]+)(\.[0-9]*)?)|(\.[0-9]+))/;

    // Get the sign of the number..
    let (input, sign) = map(
        opt(alt((value(1.0, nom_char('+')), value(-1.0, nom_char('-'))))),
        |sign| sign.unwrap_or(1.0),
    )(input)?;

    // Now we can parse the digits.
    map_res(
        take_while(|c| matches!(c, '.' | '0'..='9')),
        move |number: Span| number.fragment().parse::<f32>().map(|value| value * sign),
    )(input)
}

fn parse_name(input: Span) -> IResult<Span, Span> {
    // name      = /[._a-zA-Z$][._a-zA-Z0-9]*/;

    // let first_char = map_parser(
    //     take(1usize),
    //     take_while1(|c| matches!(c, '.' | '_' | '$' | 'a'..='z' | 'A'..='Z')),
    // );

    // let rest_of_name = take_while(|c| matches!(c, '.' | '_' | 'a'..='z' | 'A'..='Z' | '0'..='9'));

    // Almost works but I need to figure out how to concat these two as a single span.
    // let (input, (first_char, rest_of_name)) = tuple((first_char, rest_of_name))(input)?;

    // FIXME this will accept incorrect strings.
    // TODO Use Verify to accomplish that: https://docs.rs/nom/7.1.3/nom/combinator/fn.verify.html
    take_while(|c| matches!(c, '.' | '_' | '$' | 'a'..='z' | 'A'..='Z' | '0'..='9'))(input)
}

// user_name =  /[_a-zA-Z$][._a-zA-Z0-9]*/; # Cannot start with a dot
fn parse_string(input: Span) -> IResult<Span, Span> {
    take_while(|c| !matches!(c, '*' | '%'))(input)
}

fn parse_field(input: Span) -> IResult<Span, Span> {
    take_while(|c| !matches!(c, '*' | '%' | ','))(input)
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

fn space(input: Span) -> IResult<Span, ()> {
    value((), take_while(is_space))(input)
}
