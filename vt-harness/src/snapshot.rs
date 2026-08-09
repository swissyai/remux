//! Complete deterministic terminal-state serialization.

use std::fmt::Write as _;
use std::io::Write as _;

use libghostty_vt::error::Error;
use libghostty_vt::fmt::{Format, Formatter, FormatterOptions};
use libghostty_vt::screen::CellContentTag;
use libghostty_vt::style::{RgbColor, Style, StyleColor};
use libghostty_vt::terminal::{Mode, Point, PointCoordinate};
use libghostty_vt::{RenderState, Terminal};

use crate::{hex, HarnessError, Result};

/// Stable names and every terminal mode exposed by the locked C-ABI wrapper.
pub const MODES: &[(&str, Mode)] = &[
    ("ansi-kam", Mode::KAM),
    ("ansi-insert", Mode::INSERT),
    ("ansi-srm", Mode::SRM),
    ("ansi-linefeed", Mode::LINEFEED),
    ("dec-decckm", Mode::DECCKM),
    ("dec-132-column", Mode::_132_COLUMN),
    ("dec-slow-scroll", Mode::SLOW_SCROLL),
    ("dec-reverse-colors", Mode::REVERSE_COLORS),
    ("dec-origin", Mode::ORIGIN),
    ("dec-wraparound", Mode::WRAPAROUND),
    ("dec-autorepeat", Mode::AUTOREPEAT),
    ("dec-x10-mouse", Mode::X10_MOUSE),
    ("dec-cursor-blinking", Mode::CURSOR_BLINKING),
    ("dec-cursor-visible", Mode::CURSOR_VISIBLE),
    ("dec-enable-mode3", Mode::ENABLE_MODE3),
    ("dec-reverse-wrap", Mode::REVERSE_WRAP),
    ("dec-alt-screen-legacy", Mode::ALT_SCREEN_LEGACY),
    ("dec-keypad-keys", Mode::KEYPAD_KEYS),
    ("dec-left-right-margin", Mode::LEFT_RIGHT_MARGIN),
    ("dec-normal-mouse", Mode::NORMAL_MOUSE),
    ("dec-button-mouse", Mode::BUTTON_MOUSE),
    ("dec-any-mouse", Mode::ANY_MOUSE),
    ("dec-focus-event", Mode::FOCUS_EVENT),
    ("dec-utf8-mouse", Mode::UTF8_MOUSE),
    ("dec-sgr-mouse", Mode::SGR_MOUSE),
    ("dec-alt-scroll", Mode::ALT_SCROLL),
    ("dec-urxvt-mouse", Mode::URXVT_MOUSE),
    ("dec-sgr-pixels-mouse", Mode::SGR_PIXELS_MOUSE),
    ("dec-numlock-keypad", Mode::NUMLOCK_KEYPAD),
    ("dec-alt-esc-prefix", Mode::ALT_ESC_PREFIX),
    ("dec-alt-sends-esc", Mode::ALT_SENDS_ESC),
    ("dec-reverse-wrap-ext", Mode::REVERSE_WRAP_EXT),
    ("dec-alt-screen", Mode::ALT_SCREEN),
    ("dec-save-cursor", Mode::SAVE_CURSOR),
    ("dec-alt-screen-save", Mode::ALT_SCREEN_SAVE),
    ("dec-bracketed-paste", Mode::BRACKETED_PASTE),
    ("dec-sync-output", Mode::SYNC_OUTPUT),
    ("dec-grapheme-cluster", Mode::GRAPHEME_CLUSTER),
    ("dec-color-scheme-report", Mode::COLOR_SCHEME_REPORT),
    ("dec-in-band-resize", Mode::IN_BAND_RESIZE),
];

/// Serializes every observable grid/history cell and global terminal field.
///
/// Failure means the ABI could not produce a coherent state and is fatal to
/// the characterization run.
pub fn full(terminal: &Terminal<'_, '_>) -> Result<String> {
    let mut output = String::with_capacity(32 * 1024);
    let cols = terminal.cols()?;
    let rows = terminal.rows()?;
    let total_rows = terminal.total_rows()?;
    let scrollback_rows = terminal.scrollback_rows()?;
    let scrollbar = terminal.scrollbar()?;
    writeln!(output, "VT-SNAPSHOT-v1").expect("write string");
    writeln!(
        output,
        "geometry cols={cols} rows={rows} total={total_rows} scrollback={scrollback_rows}"
    )
    .expect("write string");
    writeln!(
        output,
        "scrollbar total={} offset={} len={}",
        scrollbar.total, scrollbar.offset, scrollbar.len
    )
    .expect("write string");
    writeln!(
        output,
        "cursor x={} y={} pending={} visible={} active={:?} kitty={} mouse={}",
        terminal.cursor_x()?,
        terminal.cursor_y()?,
        terminal.is_cursor_pending_wrap()?,
        terminal.is_cursor_visible()?,
        terminal.active_screen()?,
        terminal.kitty_keyboard_flags()?.bits(),
        terminal.is_mouse_tracking()?,
    )
    .expect("write string");
    write_style(&mut output, "cursor-sgr", terminal.cursor_style()?);
    writeln!(output, "title={}", hex(terminal.title()?.as_bytes())).expect("write string");
    writeln!(output, "pwd={}", hex(terminal.pwd()?.as_bytes())).expect("write string");
    write_color(&mut output, "fg", terminal.fg_color()?);
    write_color(&mut output, "bg", terminal.bg_color()?);
    write_color(&mut output, "cursor-color", terminal.cursor_color()?);
    write_color(&mut output, "default-fg", terminal.default_fg_color()?);
    write_color(&mut output, "default-bg", terminal.default_bg_color()?);
    write_color(
        &mut output,
        "default-cursor-color",
        terminal.default_cursor_color()?,
    );

    let palette = terminal.color_palette()?;
    output.push_str("palette=");
    for (index, color) in palette.0.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        write!(output, "{:02x}{:02x}{:02x}", color.r, color.g, color.b).expect("write string");
    }
    output.push('\n');

    output.push_str("modes");
    for (name, mode) in MODES {
        write!(output, " {name}={}", terminal.mode(*mode)?).expect("write string");
    }
    output.push('\n');

    let mut render = RenderState::new()?;
    {
        let rendered = render.update(terminal)?;
        let viewport = rendered.cursor_viewport()?;
        writeln!(
            output,
            "render-cursor visible={} blinking={} password={} visual={:?} viewport={viewport:?}",
            rendered.cursor_visible()?,
            rendered.cursor_blinking()?,
            rendered.cursor_password_input()?,
            rendered.cursor_visual_style()?,
        )
        .expect("write string");
        let colors = rendered.colors()?;
        writeln!(
            output,
            "render-colors fg={} bg={} cursor={}",
            rgb(colors.foreground),
            rgb(colors.background),
            colors.cursor.map_or_else(|| "none".to_owned(), rgb),
        )
        .expect("write string");
    }

    let total_rows_u32 = u32::try_from(total_rows)
        .map_err(|_| HarnessError::new("screen row count exceeds C-ABI coordinate range"))?;
    for y in 0..total_rows_u32 {
        let row_ref = terminal.grid_ref(Point::Screen(PointCoordinate { x: 0, y }))?;
        let row = row_ref.row()?;
        writeln!(
            output,
            "row y={y} wrap={} continuation={} grapheme={} styled={} hyperlink={} semantic={:?} placeholder={}",
            row.is_wrapped()?,
            row.is_wrap_continuation()?,
            row.has_grapheme_cluster()?,
            row.is_styled()?,
            row.has_hyperlink()?,
            row.semantic_prompt()?,
            row.has_kitty_virtual_placeholder()?,
        )
        .expect("write string");
        for x in 0..cols {
            write_cell(terminal, &mut output, x, y)?;
        }
    }
    Ok(output)
}

/// Produces a bounded complete-state representation for high-volume A/B runs.
///
/// libghostty's C-ABI formatter emits screen content, modes, cursor, style,
/// keyboard state, scrolling region, tab stops, palette, and metadata. Direct
/// scalar queries are appended so formatter omissions cannot hide divergence.
pub fn fast(terminal: &Terminal<'_, '_>) -> Result<Vec<u8>> {
    let options = FormatterOptions::new()
        .with_format(Format::Vt)
        .with_unwrap(false)
        .with_trim(false)
        .with_palette(true)
        .with_modes(true)
        .with_scrolling_region(true)
        .with_tabstops(true)
        .with_pwd(true)
        .with_keyboard(true)
        .with_cursor(true)
        .with_style(true)
        .with_hyperlink(true)
        .with_protection(true)
        .with_kitty_keyboard(true)
        .with_charsets(true);
    let mut formatter = Formatter::new(terminal, options)?;
    let formatted = formatter.format_alloc(None)?;
    let mut output = Vec::with_capacity(formatted.len() + 1024);
    output.extend_from_slice(&formatted);
    drop(formatted);
    drop(formatter);
    write!(
        output,
        "\nSCALAR {} {} {} {} {} {} {:?} {} {} {} {}\n",
        terminal.cols()?,
        terminal.rows()?,
        terminal.cursor_x()?,
        terminal.cursor_y()?,
        terminal.is_cursor_pending_wrap()?,
        terminal.is_cursor_visible()?,
        terminal.active_screen()?,
        terminal.kitty_keyboard_flags()?.bits(),
        terminal.total_rows()?,
        terminal.scrollback_rows()?,
        terminal.is_mouse_tracking()?,
    )
    .map_err(|error| HarnessError::new(format!("write fast state: {error}")))?;
    for (name, mode) in MODES {
        writeln!(output, "{name}={}", terminal.mode(*mode)?)
            .map_err(|error| HarnessError::new(format!("write fast mode: {error}")))?;
    }
    Ok(output)
}

fn write_cell(terminal: &Terminal<'_, '_>, output: &mut String, x: u16, y: u32) -> Result<()> {
    let reference = terminal.grid_ref(Point::Screen(PointCoordinate { x, y }))?;
    let cell = reference.cell()?;
    let style = reference.style()?;
    let graphemes = read_graphemes(&reference)?;
    let hyperlink = read_hyperlink(&reference)?;
    write!(
        output,
        "cell x={x} glyph={} tag={:?} wide={:?} text={} styling={} hyperlink={} protected={} semantic={:?}",
        codepoints_hex(&graphemes),
        cell.content_tag()?,
        cell.wide()?,
        cell.has_text()?,
        cell.has_styling()?,
        cell.has_hyperlink()?,
        cell.is_protected()?,
        cell.semantic_content()?,
    )
    .expect("write string");
    match cell.content_tag()? {
        CellContentTag::BgColorPalette => {
            write!(output, " raw-bg=p{}", cell.bg_color_palette()?.0).expect("write string");
        }
        CellContentTag::BgColorRgb => {
            write!(output, " raw-bg={}", rgb(cell.bg_color_rgb()?)).expect("write string");
        }
        CellContentTag::Codepoint | CellContentTag::CodepointGrapheme => {}
    }
    write!(output, " uri={} ", hex(&hyperlink)).expect("write string");
    write_style_inline(output, style);
    output.push('\n');
    Ok(())
}

fn read_graphemes(reference: &libghostty_vt::screen::GridRef<'_>) -> Result<Vec<char>> {
    let mut stack = ['\0'; 8];
    match reference.graphemes(&mut stack) {
        Ok(length) => Ok(stack[..length].to_vec()),
        Err(Error::OutOfSpace { required }) => {
            let mut dynamic = vec!['\0'; required];
            let length = reference.graphemes(&mut dynamic)?;
            dynamic.truncate(length);
            Ok(dynamic)
        }
        Err(error) => Err(error.into()),
    }
}

fn read_hyperlink(reference: &libghostty_vt::screen::GridRef<'_>) -> Result<Vec<u8>> {
    let mut stack = [0_u8; 128];
    match reference.hyperlink_uri(&mut stack) {
        Ok(length) => Ok(stack[..length].to_vec()),
        Err(Error::OutOfSpace { required }) => {
            let mut dynamic = vec![0_u8; required];
            let length = reference.hyperlink_uri(&mut dynamic)?;
            dynamic.truncate(length);
            Ok(dynamic)
        }
        Err(error) => Err(error.into()),
    }
}

fn codepoints_hex(graphemes: &[char]) -> String {
    let mut output = String::new();
    for (index, grapheme) in graphemes.iter().enumerate() {
        if index > 0 {
            output.push('+');
        }
        write!(output, "{:x}", u32::from(*grapheme)).expect("write string");
    }
    if output.is_empty() {
        output.push('-');
    }
    output
}

fn write_color(output: &mut String, name: &str, color: Option<RgbColor>) {
    writeln!(
        output,
        "color {name}={}",
        color.map_or_else(|| "none".to_owned(), rgb)
    )
    .expect("write string");
}

fn rgb(color: RgbColor) -> String {
    format!("{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

fn write_style(output: &mut String, name: &str, style: Style) {
    write!(output, "style {name} ").expect("write string");
    write_style_inline(output, style);
    output.push('\n');
}

fn write_style_inline(output: &mut String, style: Style) {
    write!(
        output,
        "fg={} bg={} ulc={} bold={} italic={} faint={} blink={} inverse={} invisible={} strike={} overline={} underline={:?}",
        style_color(style.fg_color),
        style_color(style.bg_color),
        style_color(style.underline_color),
        style.bold,
        style.italic,
        style.faint,
        style.blink,
        style.inverse,
        style.invisible,
        style.strikethrough,
        style.overline,
        style.underline,
    )
    .expect("write string");
}

fn style_color(color: StyleColor) -> String {
    match color {
        StyleColor::None => "none".to_owned(),
        StyleColor::Palette(index) => format!("p{}", index.0),
        StyleColor::Rgb(color) => rgb(color),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libghostty_vt::TerminalOptions;

    #[test]
    fn full_snapshot_contains_scrollback_glyph_style_cursor_and_modes() {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 4,
            rows: 2,
            max_scrollback: 8,
        })
        .expect("terminal");
        terminal.vt_write(b"\x1b[1;31mA\x1b[0m\r\nB\r\nC");
        let state = full(&terminal).expect("snapshot");
        assert!(state.contains("scrollback="));
        assert!(state.contains("glyph=41"));
        assert!(state.contains("bold=true"));
        assert!(state.contains("render-cursor"));
        assert!(state.contains("dec-wraparound=true"));
    }

    #[test]
    fn fast_state_changes_for_a_mode_only_change() {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 4,
            rows: 2,
            max_scrollback: 0,
        })
        .expect("terminal");
        let before = fast(&terminal).expect("before");
        terminal.vt_write(b"\x1b[?2004h");
        let after = fast(&terminal).expect("after");
        assert_ne!(before, after);
    }

    #[test]
    fn enum_shapes_remain_explicit() {
        assert_eq!(
            format!("{:?}", libghostty_vt::screen::CellWide::SpacerTail),
            "SpacerTail"
        );
    }
}
