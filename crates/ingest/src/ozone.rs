// SPDX-License-Identifier: MIT OR Apache-2.0

//! Coverage Summary table in SEGGER UM08025, "Code Profile Report".
//! Ozone's totals cover the selected report scope. Import them as supplied:
//! per-function rows do not establish instruction addresses or branch outcomes.

use anyhow::{anyhow, bail, ensure, Context, Result};
use butterfly_model::{limits, InputArtifact, OzoneCount, OzoneCoverage, OzoneFunction, OzoneModule, OzoneReport};
use std::path::Path;

pub fn read(path: &Path, limits: &limits::Limits) -> Result<(OzoneReport, InputArtifact)> {
    // None identifies an operator-named input: read_input permits links for explicitly supplied paths.
    // If reports ever participate in discovery, pass their discovery origin so open_discovered checks those links.
    let text = super::read_input(path, limits::Kind::Auxiliary, limits, None)?;
    let report = parse(&text).with_context(|| format!("reading Ozone Code Profile Report {}", path.display()))?;
    Ok((report, super::artifact("ozone-code-profile", path, text.as_bytes())))
}

fn count(cell: &str) -> Result<OzoneCount> {
    let (covered, rest) = cell.split_once('/').context("expected covered / total and percentage")?;
    let mut fields = rest.split_whitespace();
    let covered = covered.trim().parse::<u64>()?;
    let total = fields.next().context("missing total")?.parse::<u64>()?;
    let displayed = fields.next().and_then(|p| p.strip_suffix('%')).context("missing percentage")?;
    let (whole, fraction) = displayed.split_once('.').unwrap_or((displayed, ""));
    ensure!(!whole.is_empty() && whole.bytes().chain(fraction.bytes()).all(|b| b.is_ascii_digit()), "invalid percentage in {cell:?}");
    let whole: u8 = whole.parse()?;
    ensure!(covered <= total && fields.next().is_none() && whole <= 100 && (whole != 100 || fraction.bytes().all(|b| b == b'0')), "invalid coverage cell {cell:?}");
    if total == 0 {
        ensure!(whole == 0 && fraction.bytes().all(|b| b == b'0'), "nonzero percentage for an empty total in {cell:?}");
    } else {
        // Compare the exact ratio to the printed decimal, one digit at a time.
        // A difference greater than one unit cannot be recovered by later digits.
        // This bounds the remainder even for u64::MAX and arbitrarily long fractions.
        let denominator = i128::from(total);
        let mut remainder = 100 * i128::from(covered) - i128::from(whole) * denominator;
        for digit in fraction.bytes() {
            ensure!(remainder.abs() <= denominator, "percentage disagrees with counts in {cell:?}");
            remainder = 10 * remainder - i128::from(digit - b'0') * denominator;
        }
        ensure!(2 * remainder.abs() <= denominator, "percentage disagrees with counts in {cell:?}");
    }
    Ok(OzoneCount { covered, total })
}

fn parse(text: &str) -> Result<OzoneReport> {
    let mut lines = text.trim_start_matches('\u{feff}').lines().map(str::trim).enumerate().filter(|(_, line)| !line.is_empty());
    ensure!(lines.next().is_some_and(|(_, s)| s == "Ozone Code Profile Report"), "expected Ozone Code Profile Report text export");
    lines.find(|(_, s)| *s == "Code Coverage Summary").context("missing Code Coverage Summary")?;
    let (_, header) = lines.next().context("missing coverage table header")?;
    ensure!(header.split('|').map(str::trim).eq(["Module/Function", "Source Lines", "Instructions"]), "unsupported coverage table header");
    let mut modules = Vec::new();
    let mut total = None;
    let mut after_separator = false;
    for (index, line) in lines {
        if line == "Code Profile Summary" {
            break;
        }
        if line.starts_with('-') && line.bytes().all(|b| matches!(b, b'-' | b'+' | b'|' | b' ')) {
            after_separator = true;
            continue;
        }
        let row = index + 1;
        ensure!(total.is_none(), "unexpected row after Total on line {row}");
        // Split from the right because a C++ operator name can contain '|'.
        let mut cells = line.rsplitn(3, '|').map(str::trim);
        let instructions = cells.next().unwrap_or_default();
        let source_lines = cells.next().ok_or_else(|| anyhow!("missing columns on line {row}"))?;
        let name = cells.next().filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("missing module/function on line {row}"))?;
        if instructions.is_empty() && source_lines.is_empty() {
            modules.push(OzoneModule { name: name.to_owned(), functions: Vec::new() });
        } else {
            let coverage = OzoneCoverage {
                source_lines: count(source_lines).with_context(|| format!("source lines on line {row}"))?,
                instructions: count(instructions).with_context(|| format!("instructions on line {row}"))?,
            };
            if after_separator && name == "Total" {
                total = Some(coverage);
            } else {
                let module = modules.last_mut().ok_or_else(|| anyhow!("function without module on line {row}"))?;
                module.functions.push(OzoneFunction { name: name.to_owned(), coverage });
            }
        }
        after_separator = false;
    }
    let Some(total) = total else { bail!("missing coverage Total row") };
    Ok(OzoneReport { modules, total })
}
