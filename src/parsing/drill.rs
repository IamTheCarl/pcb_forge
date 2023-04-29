//! The XNC drill file specification can be found [here](https://www.ucamco.com/en/guest/downloads/gerber-format). The copy
//! that was used to create this parser is located [here](../../specifications/xnc-format-specification-revision-2021-11_en.pdf)
//!
//! Several structures and functions in this file will state section numbers referencing that document.
//!

use nalgebra::Vector2;
use nom::{
    branch::alt,
    bytes::complete::{tag, take_while, take_while1},
    character::complete::char as nom_char,
    combinator::{map, map_res, opt, value},
    multi::many0,
    sequence::{delimited, pair, preceded, terminated, tuple},
    IResult,
};
use nom_locate::LocatedSpan;

use super::{LocationInfo, UnitMode};

pub type Span<'a> = LocatedSpan<&'a str>;

#[derive(Debug)]
pub struct HeaderCommandContext<'a> {
    pub span: Span<'a>,
    pub command: HeaderCommand<'a>,
}

impl<'a> HeaderCommandContext<'a> {
    pub fn location_info(&self) -> LocationInfo {
        LocationInfo {
            line: self.span.location_line(),
            column: self.span.get_utf8_column(),
        }
    }
}

#[derive(Debug)]
pub enum HeaderCommand<'a> {
    Comment(Span<'a>),  // 3.1
    UnitMode(UnitMode), // 3.3
    Format(Span<'a>),
    ToolDeclaration {
        // 3.4
        index: usize,
        diameter: f64,
    },
}

#[derive(Debug)]
pub struct DrillCommandContext<'a> {
    pub span: Span<'a>,
    pub command: DrillCommand<'a>,
}

impl<'a> DrillCommandContext<'a> {
    pub fn location_info(&self) -> LocationInfo {
        LocationInfo {
            line: self.span.location_line(),
            column: self.span.get_utf8_column(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum DrillCommand<'a> {
    Comment(Span<'a>), // 3.1
    AbsoluteMode,
    IncrementalMode,
    DrillMode,         // 3.6
    RouteMode,         // 3.7
    SelectTool(usize), // 3.8
    DrillHit {
        // 3.9
        target: Vector2<f64>,
    },
    ToolDown, // 3.10
    ToolUp,   // 3.11
    LinearMove {
        // 3.12
        target: Vector2<f64>,
    },
    ClockwiseCurve {
        // 3.13
        target: Vector2<f64>,
        diameter: f64,
    },
    CounterClockwiseCurve {
        // 3.14
        target: Vector2<f64>,
        diameter: f64,
    },
}

pub fn parse_drill_file(
    input: Span,
) -> IResult<Span, (Vec<HeaderCommandContext>, Vec<DrillCommandContext>)> {
    pair(parse_header, parse_body)(input)
}

fn parse_header(input: Span) -> IResult<Span, Vec<HeaderCommandContext>> {
    delimited(
        terminated(tag("M48"), space),
        many0(terminated(parse_header_command, space)),
        terminated(nom_char('%'), space),
    )(input)
}

fn parse_header_command(input: Span) -> IResult<Span, HeaderCommandContext> {
    map(
        alt((
            map(parse_comment, HeaderCommand::Comment),
            parse_format_specification,
            parse_unit_mode,
            parse_tool_declaration,
        )),
        |command| HeaderCommandContext {
            span: input,
            command,
        },
    )(input)
}

fn parse_unit_mode(input: Span) -> IResult<Span, HeaderCommand> {
    map(
        alt((
            value(UnitMode::Metric, tag("METRIC")),
            value(UnitMode::Imperial, tag("INCH")),
        )),
        HeaderCommand::UnitMode,
    )(input)
}

/// KiCad seems to produce specification compliant drill files, but they also include a
/// format command at the start and it's not even in a comment, so I have to account for it.
fn parse_format_specification(input: Span) -> IResult<Span, HeaderCommand> {
    map(
        preceded(tag("FMAT,"), take_while(|c| c != '\n')),
        HeaderCommand::Format,
    )(input)
}

fn parse_tool_declaration(input: Span) -> IResult<Span, HeaderCommand> {
    map(
        pair(
            preceded(nom_char('T'), parse_unsigned_integer),
            preceded(nom_char('C'), parse_unsigned_decimal),
        ),
        |(index, diameter)| HeaderCommand::ToolDeclaration { index, diameter },
    )(input)
}

fn parse_body(input: Span) -> IResult<Span, Vec<DrillCommandContext>> {
    terminated(
        many0(terminated(parse_drill_command, space)),
        terminated(tag("M30"), space),
    )(input)
}

fn parse_drill_command(input: Span) -> IResult<Span, DrillCommandContext> {
    map(
        alt((
            map(parse_comment, DrillCommand::Comment),
            parse_absolute_mode,
            parse_incremental_mode,
            parse_dill_mode,
            parse_route_mode,
            parse_select_tool,
            parse_drill_hit,
            parse_tool_down,
            parse_tool_up,
            parse_linear_move,
            parse_clockwise_curve,
            parse_counter_clockwise_curve,
        )),
        |command| DrillCommandContext {
            span: input,
            command,
        },
    )(input)
}

fn parse_absolute_mode(input: Span) -> IResult<Span, DrillCommand> {
    // KiCad adds a custom "absolute mode". All coordinates are absolute.
    value(DrillCommand::AbsoluteMode, tag("G90"))(input)
}

fn parse_incremental_mode(input: Span) -> IResult<Span, DrillCommand> {
    // KiCad adds a custom "incremental mode". All coordinates are relative to the previous position.
    value(DrillCommand::IncrementalMode, tag("G90"))(input)
}

fn parse_dill_mode(input: Span) -> IResult<Span, DrillCommand> {
    value(DrillCommand::DrillMode, tag("G05"))(input)
}

fn parse_route_mode(input: Span) -> IResult<Span, DrillCommand> {
    value(DrillCommand::RouteMode, tag("G00"))(input)
}

fn parse_select_tool(input: Span) -> IResult<Span, DrillCommand> {
    map(
        preceded(nom_char('T'), parse_unsigned_integer),
        DrillCommand::SelectTool,
    )(input)
}

fn parse_drill_hit(input: Span) -> IResult<Span, DrillCommand> {
    map(
        pair(
            preceded(nom_char('X'), parse_decimal),
            preceded(nom_char('Y'), parse_decimal),
        ),
        |(x, y)| DrillCommand::DrillHit {
            target: Vector2::new(x, y),
        },
    )(input)
}

fn parse_tool_down(input: Span) -> IResult<Span, DrillCommand> {
    value(DrillCommand::ToolDown, tag("M15"))(input)
}

fn parse_tool_up(input: Span) -> IResult<Span, DrillCommand> {
    value(DrillCommand::ToolUp, tag("M16"))(input)
}

fn parse_linear_move(input: Span) -> IResult<Span, DrillCommand> {
    map(
        preceded(
            tag("G01"),
            pair(
                preceded(nom_char('X'), parse_decimal),
                preceded(nom_char('Y'), parse_decimal),
            ),
        ),
        |(x, y)| DrillCommand::LinearMove {
            target: Vector2::new(x, y),
        },
    )(input)
}

fn parse_clockwise_curve(input: Span) -> IResult<Span, DrillCommand> {
    map(
        preceded(
            tag("G02"),
            tuple((
                preceded(nom_char('X'), parse_decimal),
                preceded(nom_char('Y'), parse_decimal),
                preceded(nom_char('A'), parse_decimal),
            )),
        ),
        |(x, y, a)| DrillCommand::ClockwiseCurve {
            target: Vector2::new(x, y),
            diameter: a,
        },
    )(input)
}

fn parse_counter_clockwise_curve(input: Span) -> IResult<Span, DrillCommand> {
    map(
        preceded(
            tag("G02"),
            tuple((
                preceded(nom_char('X'), parse_decimal),
                preceded(nom_char('Y'), parse_decimal),
                preceded(nom_char('A'), parse_decimal),
            )),
        ),
        |(x, y, a)| DrillCommand::CounterClockwiseCurve {
            target: Vector2::new(x, y),
            diameter: a,
        },
    )(input)
}

fn parse_unsigned_integer(input: Span) -> IResult<Span, usize> {
    map_res(take_while1(|c: char| c.is_ascii_digit()), |digits: Span| {
        digits.fragment().parse::<usize>()
    })(input)
}

fn parse_decimal(input: Span) -> IResult<Span, f64> {
    // decimal          = /[+-]?((([0-9]+)(\.[0-9]*)?)|(\.[0-9]+))/;

    // Get the sign of the number..
    let (input, sign) = map(
        opt(alt((value(1.0, nom_char('+')), value(-1.0, nom_char('-'))))),
        |sign| sign.unwrap_or(1.0),
    )(input)?;

    // Now we can parse the digits.
    map_res(
        take_while(|c| matches!(c, '.' | '0'..='9')),
        move |number: Span| number.fragment().parse::<f64>().map(|value| value * sign),
    )(input)
}

fn parse_comment(input: Span) -> IResult<Span, Span> {
    delimited(nom_char(';'), take_while(|c| c != '\n'), space)(input)
}

fn parse_unsigned_decimal(input: Span) -> IResult<Span, f64> {
    // unsigned_decimal =      /((([0-9]+)(\.[0-9]*)?)|(\.[0-9]+))/;
    map_res(
        take_while(|c| matches!(c, '.' | '0'..='9')), // Intentionally no + or - sign in there.
        move |number: Span| number.fragment().parse::<f64>(),
    )(input)
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

fn space(input: Span) -> IResult<Span, ()> {
    value((), take_while(is_space))(input)
}
