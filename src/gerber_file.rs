use anyhow::{bail, Context, Result};
use std::{fs, path::Path};

use crate::parsing::gerber::{parse_gerber_file, Span};

pub fn load(path: &Path) -> Result<()> {
    let file_content = fs::read_to_string(path).context("Failed to read file into memory.")?;
    let parsing_result = parse_gerber_file(Span::new(&file_content));

    match parsing_result {
        Ok((_unused_content, commands)) => {
            dbg!(commands);
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

    Ok(())
}
