//! Domain-agnostic terminal prompt: editable magnitude + cyclable unit name.

use anyhow::{bail, Context, Result};
use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::queue;
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType};
use rust_decimal::Decimal;
use std::io::{stdout, Write};

const KEY_HINT: &str = "tab focus · ←→ unit · enter submit · esc cancel";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Magnitude,
    Unit,
}

#[derive(Clone, Debug)]
pub struct MagnitudeConstraints {
    pub minimum: Option<Decimal>,
    pub maximum: Option<Decimal>,
    pub decimals: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitValueAnswer {
    pub magnitude: String,
    pub unit_index: usize,
}

#[derive(Clone, Debug)]
pub struct UnitValueState {
    pub magnitude: String,
    pub cursor: usize,
    pub unit_index: usize,
    pub unit_count: usize,
    pub focus: Focus,
    pub error: Option<String>,
}

impl UnitValueState {
    pub fn new(
        default_magnitude: Option<&str>,
        default_unit_index: usize,
        unit_count: usize,
    ) -> Self {
        assert!(
            unit_count > 0,
            "BUG: unit value prompt needs at least one unit"
        );
        let unit_index = default_unit_index.min(unit_count - 1);
        let magnitude = default_magnitude.unwrap_or("").to_string();
        let cursor = magnitude.len();
        Self {
            magnitude,
            cursor,
            unit_index,
            unit_count,
            focus: Focus::Magnitude,
            error: None,
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Magnitude => Focus::Unit,
            Focus::Unit => Focus::Magnitude,
        };
        self.error = None;
    }

    pub fn cycle_unit(&mut self, delta: isize) {
        if self.unit_count == 0 {
            return;
        }
        let count = self.unit_count as isize;
        let next = (self.unit_index as isize + delta).rem_euclid(count) as usize;
        self.unit_index = next;
        self.error = None;
    }

    pub fn insert_char(&mut self, ch: char) {
        if self.focus != Focus::Magnitude {
            return;
        }
        if !ch.is_ascii_digit() && ch != '.' && ch != '_' && ch != '-' {
            return;
        }
        self.magnitude.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.error = None;
    }

    pub fn backspace(&mut self) {
        if self.focus != Focus::Magnitude || self.cursor == 0 {
            return;
        }
        let prev = self.magnitude[..self.cursor]
            .chars()
            .next_back()
            .map(|c| c.len_utf8())
            .unwrap_or(0);
        self.cursor -= prev;
        self.magnitude.remove(self.cursor);
        self.error = None;
    }

    pub fn delete(&mut self) {
        if self.focus != Focus::Magnitude || self.cursor >= self.magnitude.len() {
            return;
        }
        let next = self.magnitude[self.cursor..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(0);
        self.magnitude
            .replace_range(self.cursor..self.cursor + next, "");
        self.error = None;
    }

    pub fn move_left(&mut self) {
        match self.focus {
            Focus::Unit => self.cycle_unit(-1),
            Focus::Magnitude => {
                if self.cursor == 0 {
                    return;
                }
                let prev = self.magnitude[..self.cursor]
                    .chars()
                    .next_back()
                    .map(|c| c.len_utf8())
                    .unwrap_or(0);
                self.cursor -= prev;
            }
        }
    }

    pub fn move_right(&mut self) {
        match self.focus {
            Focus::Unit => self.cycle_unit(1),
            Focus::Magnitude => {
                if self.cursor >= self.magnitude.len() {
                    return;
                }
                let next = self.magnitude[self.cursor..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(0);
                self.cursor += next;
            }
        }
    }

    pub fn move_home(&mut self) {
        if self.focus == Focus::Magnitude {
            self.cursor = 0;
        }
    }

    pub fn move_end(&mut self) {
        if self.focus == Focus::Magnitude {
            self.cursor = self.magnitude.len();
        }
    }

    /// Validate current magnitude against `constraints` for the selected unit.
    pub fn try_submit(
        &self,
        constraints: &MagnitudeConstraints,
    ) -> Result<UnitValueAnswer, String> {
        let magnitude = normalize_magnitude(&self.magnitude)?;
        validate_magnitude(&magnitude, constraints)?;
        Ok(UnitValueAnswer {
            magnitude,
            unit_index: self.unit_index,
        })
    }
}

pub fn normalize_magnitude(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Value is required".to_string());
    }
    Ok(trimmed.replace('_', ""))
}

pub fn validate_magnitude(clean: &str, constraints: &MagnitudeConstraints) -> Result<(), String> {
    let provided_decimals = clean
        .split_once('.')
        .map(|(_, frac)| frac.len())
        .unwrap_or(0);
    if let Some(d) = constraints.decimals {
        if provided_decimals > d as usize {
            return Err(format!("Too many decimals (max {d})"));
        }
    }
    let value = Decimal::from_str_exact(clean).map_err(|_| format!("Invalid number: '{clean}'"))?;
    if let Some(min) = constraints.minimum {
        if value < min {
            return Err(format!("Must be >= {min}"));
        }
    }
    if let Some(max) = constraints.maximum {
        if value > max {
            return Err(format!("Must be <= {max}"));
        }
    }
    Ok(())
}

/// Bounds for the unit currently selected in `state`.
pub fn constraints_for_unit(
    unit_index: usize,
    per_unit: &[MagnitudeConstraints],
) -> &MagnitudeConstraints {
    per_unit
        .get(unit_index)
        .expect("BUG: unit_index out of range for magnitude constraints")
}

pub struct UnitValuePrompt<'a> {
    pub message: &'a str,
    pub help: &'a str,
    pub units: &'a [String],
    pub default_unit_index: usize,
    pub default_magnitude: Option<&'a str>,
    pub per_unit_constraints: &'a [MagnitudeConstraints],
}

pub fn prompt_unit_value(prompt: UnitValuePrompt<'_>) -> Result<UnitValueAnswer> {
    if prompt.units.is_empty() {
        bail!("BUG: unit value prompt requires at least one unit");
    }
    if prompt.per_unit_constraints.len() != prompt.units.len() {
        bail!("BUG: unit value prompt constraints must match units");
    }

    let mut state = UnitValueState::new(
        prompt.default_magnitude,
        prompt.default_unit_index,
        prompt.units.len(),
    );
    let mut stdout = stdout();
    enable_raw_mode().context("enable terminal raw mode")?;
    let result = (|| -> Result<UnitValueAnswer> {
        let mut drawn_rows = 0usize;
        loop {
            drawn_rows = draw_frame(&mut stdout, &prompt, &state, drawn_rows)?;
            let Event::Key(key) = event::read().context("read terminal key")? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match handle_key(&mut state, key) {
                KeyOutcome::Continue => {}
                KeyOutcome::Cancel => bail!("Cancelled"),
                KeyOutcome::Submit => {
                    let constraints =
                        constraints_for_unit(state.unit_index, prompt.per_unit_constraints);
                    match state.try_submit(constraints) {
                        Ok(answer) => {
                            clear_drawn(&mut stdout, drawn_rows)?;
                            write_finished(&mut stdout, prompt.message, &answer, prompt.units)?;
                            return Ok(answer);
                        }
                        Err(message) => {
                            state.error = Some(message);
                        }
                    }
                }
            }
        }
    })();
    let _ = disable_raw_mode();
    result
}

enum KeyOutcome {
    Continue,
    Submit,
    Cancel,
}

fn handle_key(state: &mut UnitValueState, key: KeyEvent) -> KeyOutcome {
    match key.code {
        KeyCode::Esc => KeyOutcome::Cancel,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => KeyOutcome::Cancel,
        KeyCode::Enter => KeyOutcome::Submit,
        KeyCode::Tab => {
            state.toggle_focus();
            KeyOutcome::Continue
        }
        KeyCode::BackTab => {
            state.toggle_focus();
            KeyOutcome::Continue
        }
        KeyCode::Left => {
            state.move_left();
            KeyOutcome::Continue
        }
        KeyCode::Right => {
            state.move_right();
            KeyOutcome::Continue
        }
        KeyCode::Up if state.focus == Focus::Unit => {
            state.cycle_unit(-1);
            KeyOutcome::Continue
        }
        KeyCode::Down if state.focus == Focus::Unit => {
            state.cycle_unit(1);
            KeyOutcome::Continue
        }
        KeyCode::Home => {
            state.move_home();
            KeyOutcome::Continue
        }
        KeyCode::End => {
            state.move_end();
            KeyOutcome::Continue
        }
        KeyCode::Backspace => {
            state.backspace();
            KeyOutcome::Continue
        }
        KeyCode::Delete => {
            state.delete();
            KeyOutcome::Continue
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.insert_char(ch);
            KeyOutcome::Continue
        }
        _ => KeyOutcome::Continue,
    }
}

fn draw_frame(
    out: &mut impl Write,
    prompt: &UnitValuePrompt<'_>,
    state: &UnitValueState,
    previous_rows: usize,
) -> Result<usize> {
    clear_drawn(out, previous_rows)?;

    let unit = prompt
        .units
        .get(state.unit_index)
        .expect("BUG: unit_index out of range");
    let mut rows = 0usize;

    queue!(
        out,
        SetForegroundColor(Color::Green),
        Print("? "),
        ResetColor,
        Print(prompt.message),
        Print("\r\n")
    )?;
    rows += 1;

    if !prompt.help.is_empty() {
        queue!(
            out,
            SetForegroundColor(Color::DarkGrey),
            Print("  "),
            Print(prompt.help),
            ResetColor,
            Print("\r\n")
        )?;
        rows += 1;
    }

    queue!(out, Print("> "))?;
    match state.focus {
        Focus::Magnitude => {
            queue!(
                out,
                SetForegroundColor(Color::Cyan),
                Print(&state.magnitude),
                ResetColor,
                Print(" | "),
                Print(unit),
            )?;
        }
        Focus::Unit => {
            queue!(
                out,
                Print(&state.magnitude),
                Print(" | "),
                SetForegroundColor(Color::Cyan),
                Print(unit),
                ResetColor,
            )?;
        }
    }
    queue!(out, Print("\r\n"))?;
    rows += 1;

    if let Some(error) = &state.error {
        queue!(
            out,
            SetForegroundColor(Color::Red),
            Print("  "),
            Print(error),
            ResetColor,
            Print("\r\n")
        )?;
        rows += 1;
    }

    queue!(
        out,
        SetForegroundColor(Color::Blue),
        Print("  "),
        Print(KEY_HINT),
        ResetColor,
        Print("\r\n")
    )?;
    rows += 1;

    out.flush()?;
    Ok(rows)
}

fn clear_drawn(out: &mut impl Write, rows: usize) -> Result<()> {
    if rows == 0 {
        return Ok(());
    }
    // Cursor sits on the blank line after the frame; move to the first frame line
    // and erase everything downward before redrawing.
    queue!(
        out,
        MoveUp(rows as u16),
        MoveToColumn(0),
        Clear(ClearType::FromCursorDown)
    )?;
    out.flush()?;
    Ok(())
}

fn write_finished(
    out: &mut impl Write,
    message: &str,
    answer: &UnitValueAnswer,
    units: &[String],
) -> Result<()> {
    let unit = &units[answer.unit_index];
    queue!(
        out,
        SetForegroundColor(Color::Green),
        Print("√ "),
        ResetColor,
        Print(message),
        Print(": "),
        SetForegroundColor(Color::Cyan),
        Print(format!("{} {}", answer.magnitude, unit)),
        ResetColor,
        Print("\r\n")
    )?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn decimal(s: &str) -> Decimal {
        Decimal::from_str_exact(s).expect("decimal")
    }

    fn bounds(min: Option<&str>, max: Option<&str>) -> MagnitudeConstraints {
        MagnitudeConstraints {
            minimum: min.map(decimal),
            maximum: max.map(decimal),
            decimals: None,
        }
    }

    #[test]
    fn cycle_unit_wraps() {
        let mut state = UnitValueState::new(Some("1"), 0, 3);
        state.focus = Focus::Unit;
        state.cycle_unit(1);
        assert_eq!(state.unit_index, 1);
        state.cycle_unit(1);
        assert_eq!(state.unit_index, 2);
        state.cycle_unit(1);
        assert_eq!(state.unit_index, 0);
        state.cycle_unit(-1);
        assert_eq!(state.unit_index, 2);
    }

    #[test]
    fn toggle_focus() {
        let mut state = UnitValueState::new(None, 0, 2);
        assert_eq!(state.focus, Focus::Magnitude);
        state.toggle_focus();
        assert_eq!(state.focus, Focus::Unit);
        state.toggle_focus();
        assert_eq!(state.focus, Focus::Magnitude);
    }

    #[test]
    fn magnitude_editing() {
        let mut state = UnitValueState::new(None, 0, 1);
        state.insert_char('1');
        state.insert_char('0');
        state.insert_char('0');
        assert_eq!(state.magnitude, "100");
        state.backspace();
        assert_eq!(state.magnitude, "10");
        state.move_home();
        state.insert_char('9');
        assert_eq!(state.magnitude, "910");
    }

    #[test]
    fn submit_uses_selected_unit_bounds() {
        let state = UnitValueState::new(Some("100"), 0, 2);
        let percent = bounds(Some("0"), Some("100"));
        let permille = bounds(Some("0"), Some("1000"));
        assert!(state.try_submit(&percent).is_ok());
        assert!(state.try_submit(&permille).is_ok());

        let over = UnitValueState::new(Some("101"), 0, 2);
        let err = over.try_submit(&percent).expect_err("over percent max");
        assert!(err.contains("Must be <="), "{err}");
        assert!(over.try_submit(&permille).is_ok());
    }

    #[test]
    fn submit_rejects_empty_and_invalid() {
        let empty = UnitValueState::new(None, 0, 1);
        assert_eq!(
            empty.try_submit(&bounds(None, None)).unwrap_err(),
            "Value is required"
        );
        let bad = UnitValueState::new(Some("nope"), 0, 1);
        assert!(bad
            .try_submit(&bounds(None, None))
            .unwrap_err()
            .contains("Invalid number"));
    }

    #[test]
    fn constraints_for_unit_picks_selected() {
        let per = [
            bounds(Some("0"), Some("100")),
            bounds(Some("0"), Some("1000")),
        ];
        assert_eq!(constraints_for_unit(1, &per).maximum, Some(decimal("1000")));
    }
}
