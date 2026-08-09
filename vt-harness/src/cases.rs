//! Authored and adversarial table construction.

use crate::abi::Operation;

/// One named characterization with deterministic terminal options and actions.
#[derive(Clone, Debug)]
pub struct Case {
    /// Stable filename-safe identifier.
    pub id: String,
    /// Initial width.
    pub cols: u16,
    /// Initial height.
    pub rows: u16,
    /// Maximum retained history rows.
    pub max_scrollback: usize,
    /// Exact action sequence.
    pub operations: Vec<Operation>,
}

impl Case {
    fn writes(id: impl Into<String>, writes: Vec<Vec<u8>>) -> Self {
        Self {
            id: id.into(),
            cols: 16,
            rows: 8,
            max_scrollback: 32,
            operations: writes.into_iter().map(Operation::Write).collect(),
        }
    }

    fn write(id: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self::writes(id, vec![bytes.into()])
    }
}

fn dense_boundary_grid() -> Vec<u8> {
    let mut bytes = Vec::new();
    for row in 1..=6_u8 {
        let first = char::from(b'A' + row - 1);
        let last = char::from(b'Z' - row + 1);
        bytes.extend_from_slice(format!("\x1b[{row};1H{first}{row}界abc{last}").as_bytes());
    }
    bytes
}

fn margin_straddle_grid() -> Vec<u8> {
    let mut bytes = Vec::new();
    for row in 1..=6_u8 {
        let first = char::from(b'A' + row - 1);
        let last = char::from(b'Z' - row + 1);
        bytes.extend_from_slice(format!("\x1b[{row};1H{first}界{row}x界{last}").as_bytes());
    }
    bytes
}

fn boundary_content_case(id: &str, command: &[u8]) -> Case {
    let mut bytes = dense_boundary_grid();
    bytes.extend_from_slice(command);
    Case {
        id: id.to_owned(),
        cols: 8,
        rows: 6,
        max_scrollback: 16,
        operations: vec![Operation::write(bytes)],
    }
}

fn margin_straddle_case(id: &str, command: &[u8]) -> Case {
    let mut bytes = margin_straddle_grid();
    bytes.extend_from_slice(b"\x1b[?69h\x1b[3;6s\x1b[2;5r");
    bytes.extend_from_slice(command);
    Case {
        id: id.to_owned(),
        cols: 8,
        rows: 6,
        max_scrollback: 16,
        operations: vec![Operation::write(bytes)],
    }
}

fn boundary_content_cases() -> Vec<Case> {
    let mut cases = vec![
        boundary_content_case("erase-line-right-boundary-content", b"\x1b[3;4H\x1b[K"),
        boundary_content_case("erase-line-left-boundary-content", b"\x1b[3;3H\x1b[1K"),
        boundary_content_case("erase-line-complete-boundary-content", b"\x1b[3;5H\x1b[2K"),
        boundary_content_case("erase-display-below-boundary-content", b"\x1b[3;4H\x1b[J"),
        boundary_content_case("erase-display-above-boundary-content", b"\x1b[3;3H\x1b[1J"),
        boundary_content_case(
            "erase-display-complete-boundary-content",
            b"\x1b[3;5H\x1b[2J",
        ),
        boundary_content_case("insert-chars-first-boundary-content", b"\x1b[3;1H\x1b[@"),
        boundary_content_case("insert-chars-last-boundary-content", b"\x1b[3;8H\x1b[@"),
        boundary_content_case("insert-chars-wide-straddle", b"\x1b[3;4H\x1b[@"),
        boundary_content_case("delete-chars-first-boundary-content", b"\x1b[3;1H\x1b[P"),
        boundary_content_case("delete-chars-last-boundary-content", b"\x1b[3;8H\x1b[P"),
        boundary_content_case("delete-chars-wide-straddle", b"\x1b[3;4H\x1b[P"),
        boundary_content_case(
            "insert-lines-region-boundary-content",
            b"\x1b[2;5r\x1b[3;1H\x1b[L",
        ),
        boundary_content_case(
            "delete-lines-region-boundary-content",
            b"\x1b[2;5r\x1b[3;1H\x1b[M",
        ),
        boundary_content_case(
            "scroll-up-region-boundary-content",
            b"\x1b[2;5r\x1b[3;1H\x1b[S",
        ),
        boundary_content_case(
            "scroll-down-region-boundary-content",
            b"\x1b[2;5r\x1b[3;1H\x1b[T",
        ),
        boundary_content_case(
            "scroll-index-region-boundary-content",
            b"\x1b[2;5r\x1b[5;4H\x1bD",
        ),
        boundary_content_case(
            "scroll-reverse-index-region-boundary-content",
            b"\x1b[2;5r\x1b[2;3H\x1bM",
        ),
        margin_straddle_case("insert-lines-margin-wide-straddle", b"\x1b[3;3H\x1b[L"),
        margin_straddle_case("delete-lines-margin-wide-straddle", b"\x1b[3;3H\x1b[M"),
        margin_straddle_case("scroll-up-margin-wide-straddle", b"\x1b[3;3H\x1b[S"),
        margin_straddle_case("scroll-down-margin-wide-straddle", b"\x1b[3;3H\x1b[T"),
    ];

    let mut scrollback = Vec::new();
    for line in 0..10_u8 {
        let first = char::from(b'A' + line);
        let last = char::from(b'Z' - line);
        scrollback.extend_from_slice(format!("{first}{line}界abc{last}\r\n").as_bytes());
    }
    scrollback.extend_from_slice(b"\x1b[3J");
    cases.push(Case {
        id: "erase-display-scrollback-boundary-content".to_owned(),
        cols: 8,
        rows: 6,
        max_scrollback: 16,
        operations: vec![Operation::write(scrollback)],
    });
    cases
}

/// Returns the fixed table of manually selected VT behavior classes.
///
/// Parameter loops expand reviewed tables; no seeded/random generator
/// contributes a case to this authored count.
#[must_use]
pub fn authored() -> Vec<Case> {
    let mut cases = Vec::with_capacity(400);

    const TEXT_SAMPLES: &[&[u8]] = &[
        b"plain ascii",
        b"tabs\tbetween\twords",
        b"carriage\rreturn",
        b"line\nfeed",
        b"crlf\r\nnext",
        "snowman [0m☃".as_bytes(),
        "wide 界 glyph".as_bytes(),
        "combining e\u{301}".as_bytes(),
    ];
    for repeat in 1..=8 {
        for (sample_index, sample) in TEXT_SAMPLES.iter().enumerate() {
            let mut bytes = Vec::new();
            for index in 0..repeat {
                bytes.extend_from_slice(sample);
                bytes.extend_from_slice(format!("-{index:02}").as_bytes());
            }
            cases.push(Case::write(
                format!("text-{sample_index:02}-{repeat:02}"),
                bytes,
            ));
        }
    }

    const CONTROLS: &[u8] = &[
        0x00, 0x05, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x18, 0x1a, 0x1b, 0x7f,
        0x84,
    ];
    for (index, control) in CONTROLS.iter().enumerate() {
        cases.push(Case::write(
            format!("control-single-{index:02}"),
            vec![b'A', *control, b'B'],
        ));
        cases.push(Case::writes(
            format!("control-split-{index:02}"),
            vec![vec![b'A'], vec![*control], vec![b'B']],
        ));
    }

    const CURSOR_FINALS: &[u8] = b"ABCDEFGHfG`adEeF";
    for (index, final_byte) in CURSOR_FINALS.iter().enumerate() {
        for parameter in [0_u16, 1, 2] {
            let bytes = format!("origin\x1b[{parameter}{}X", char::from(*final_byte)).into_bytes();
            cases.push(Case::write(
                format!("cursor-{index:02}-{parameter:02}"),
                bytes,
            ));
        }
    }

    const SGR_PARAMS: &[&str] = &[
        "0",
        "1",
        "2",
        "3",
        "4",
        "4:2",
        "4:3",
        "4:4",
        "4:5",
        "5",
        "7",
        "8",
        "9",
        "21",
        "22",
        "23",
        "24",
        "25",
        "27",
        "28",
        "29",
        "30",
        "31",
        "32",
        "33",
        "34",
        "35",
        "36",
        "37",
        "38;5;0",
        "38;5;7",
        "38;5;15",
        "38;5;196",
        "38;2;1;2;3",
        "39",
        "40",
        "41",
        "42",
        "43",
        "44",
        "45",
        "46",
        "47",
        "48;5;17",
        "48;5;231",
        "48;2;250;128;4",
        "49",
        "51",
        "52",
        "53",
        "54",
        "55",
        "58;5;99",
        "58;2;4;5;6",
        "59",
        "73",
        "74",
        "90",
        "91",
        "92",
        "93",
        "94",
        "95",
        "96",
        "97",
        "100",
        "101",
        "102",
        "103",
        "104",
        "105",
        "106",
        "107",
        "1;3;4;9",
        "1;31;44",
        "2;38;2;8;9;10",
        "7;38;5;2;48;5;4",
    ];
    for (index, params) in SGR_PARAMS.iter().enumerate() {
        cases.push(Case::write(
            format!("sgr-{index:03}"),
            format!("\x1b[{params}mX\x1b[0mY").into_bytes(),
        ));
        cases.push(Case::writes(
            format!("sgr-persist-{index:03}"),
            vec![format!("\x1b[{params}m").into_bytes(), b"AB".to_vec()],
        ));
    }

    const ERASE_INSERT: &[&str] = &[
        "J", "0J", "1J", "2J", "3J", "K", "0K", "1K", "2K", "@", "2@", "P", "2P", "X", "2X", "L",
        "2L", "M", "2M", "S", "2S", "T", "2T", "g",
    ];
    for (index, sequence) in ERASE_INSERT.iter().enumerate() {
        cases.push(Case::write(
            format!("edit-{index:02}"),
            format!("row-one\r\nrow-two\x1b[2;4H\x1b[{sequence}Z").into_bytes(),
        ));
    }
    cases.extend(boundary_content_cases());

    const DEC_MODES: &[u16] = &[
        1, 3, 5, 6, 7, 8, 9, 12, 25, 40, 45, 47, 66, 69, 1000, 1002, 1003, 1004, 1005, 1006, 1007,
        1015, 1016, 1035, 1036, 1039, 1045, 1047, 1048, 1049, 2004, 2026, 2027, 2031, 2048,
    ];
    for mode in DEC_MODES {
        cases.push(Case::write(
            format!("mode-dec-{mode}-set"),
            format!("\x1b[?{mode}hM").into_bytes(),
        ));
    }
    for mode in [2_u16, 4, 12, 20] {
        cases.push(Case::write(
            format!("mode-ansi-{mode}-set"),
            format!("\x1b[{mode}hM").into_bytes(),
        ));
    }

    const OSC_BODIES: &[&str] = &[
        "0;title-zero",
        "1;icon-title",
        "2;window-title",
        "7;file:///tmp/vt1",
        "8;id=x;https://example.invalid",
        "8;;",
        "10;rgb:11/22/33",
        "11;#445566",
        "12;rgb:77/88/99",
        "4;1;rgb:aa/bb/cc",
        "104;1",
        "133;A",
    ];
    for (index, body) in OSC_BODIES.iter().enumerate() {
        cases.push(Case::write(
            format!("osc-bel-{index:02}"),
            format!("\x1b]{body}\x07X").into_bytes(),
        ));
        cases.push(Case::write(
            format!("osc-st-{index:02}"),
            format!("\x1b]{body}\x1b\\X").into_bytes(),
        ));
    }

    cases.push(Case {
        id: "bug-scrollback-max-five-retains-six".to_owned(),
        cols: 8,
        rows: 3,
        max_scrollback: 5,
        operations: vec![Operation::write(
            b"0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n".to_vec(),
        )],
    });

    for index in 0..16_u8 {
        let mut operations = vec![Operation::write(
            (0..20)
                .map(|row| format!("line-{index:02}-{row:02}\r\n"))
                .collect::<String>()
                .into_bytes(),
        )];
        operations.push(Operation::ScrollTop);
        if index % 2 == 0 {
            operations.push(Operation::ScrollBottom);
        }
        operations.push(Operation::Resize {
            cols: 8 + u16::from(index % 5),
            rows: 4 + u16::from(index % 3),
            cell_width_px: 7,
            cell_height_px: 14,
        });
        cases.push(Case {
            id: format!("scroll-resize-{index:02}"),
            cols: 16,
            rows: 8,
            max_scrollback: 64,
            operations,
        });
    }

    cases.sort_by(|left, right| left.id.cmp(&right.id));
    cases
}

/// Returns hostile/malformed byte streams whose observed behavior is golden.
#[must_use]
pub fn adversarial() -> Vec<Case> {
    let mut cases = Vec::with_capacity(120);

    const TRUNCATED: &[&[u8]] = &[
        b"\x1b", b"\x1b[", b"\x1b[?", b"\x1b[1;", b"\x1b]", b"\x1b]0;", b"\x1bP", b"\x1bP$q",
    ];
    for index in 0..24 {
        let mut bytes = TRUNCATED[index % TRUNCATED.len()].to_vec();
        bytes.extend(std::iter::repeat_n(
            b'0' + u8::try_from(index % 10).expect("digit"),
            index,
        ));
        cases.push(Case::write(format!("hostile-truncated-{index:02}"), bytes));
    }

    const SPLIT_SEQUENCES: &[&[u8]] = &[
        b"\x1b[31mRED\x1b[0m",
        b"\x1b[4;7HXY",
        b"\x1b[?25lhidden",
        b"\x1b]2;split-title\x07",
        b"\x1bP1;2|payload\x1b\\",
        b"\x1b[38;2;1;2;3mRGB",
        b"\x1b[?1049halt\x1b[?1049l",
        b"\x1b[2Jclear",
    ];
    for index in 0..24 {
        let bytes = SPLIT_SEQUENCES[index % SPLIT_SEQUENCES.len()];
        let split = 1 + (index % bytes.len().saturating_sub(1).max(1));
        cases.push(Case::writes(
            format!("hostile-split-escape-{index:02}"),
            vec![bytes[..split].to_vec(), bytes[split..].to_vec()],
        ));
    }

    const UTF8: &[&[u8]] = &[
        "é".as_bytes(),
        "界".as_bytes(),
        "🙂".as_bytes(),
        "e\u{301}".as_bytes(),
        &[0xf0, 0x28, 0x8c, 0xbc],
        &[0xc0, 0xaf],
        &[0xed, 0xa0, 0x80],
        &[0xe2, 0x82],
    ];
    for index in 0..24 {
        let bytes = UTF8[index % UTF8.len()];
        let split = 1 + (index % bytes.len().saturating_sub(1).max(1));
        cases.push(Case::writes(
            format!("hostile-utf8-boundary-{index:02}"),
            vec![
                b"A".to_vec(),
                bytes[..split].to_vec(),
                bytes[split..].to_vec(),
                b"Z".to_vec(),
            ],
        ));
    }

    for index in 0..24 {
        let digits = "9".repeat(64 + index * 17);
        let bytes = format!("\x1b[{digits};{digits};{digits}mX").into_bytes();
        cases.push(Case::write(
            format!("hostile-oversized-param-{index:02}"),
            bytes,
        ));
    }

    for index in 0..24 {
        let bytes = match index % 6 {
            0 => b"\x1b]2;osc\x1bP1;2|dcs\x1b\\tail\x07".to_vec(),
            1 => b"\x1bP$qm\x1b]11;#123456\x07\x1b\\".to_vec(),
            2 => b"\x1b]8;;https://invalid\x1bPignored\x1b\\text\x1b]8;;\x07".to_vec(),
            3 => b"\x1bP+q544e\x1b]0;nested\x1b\\\x07".to_vec(),
            4 => b"\x1b]52;c;%%%%\x1bP!|bad\x07\x1b\\".to_vec(),
            _ => b"\x1bP\x1b\x1b]\x1b\\\x07\x18cancel".to_vec(),
        };
        cases.push(Case::write(
            format!("hostile-interleaved-osc-dcs-{index:02}"),
            bytes,
        ));
    }

    cases.sort_by(|left, right| left.id.cmp(&right.id));
    cases
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn authored_floor_and_ids_are_stable_and_unique() {
        let cases = authored();
        assert!(cases.len() >= 300, "got {}", cases.len());
        let ids = cases.iter().map(|case| &case.id).collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), cases.len());
        assert!(ids.iter().all(|id| id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')));
    }

    #[test]
    fn boundary_content_matrix_covers_erase_and_sibling_movers() {
        let cases = authored();
        let ids = cases
            .iter()
            .map(|case| case.id.as_str())
            .collect::<BTreeSet<_>>();
        for id in [
            "erase-line-right-boundary-content",
            "erase-line-left-boundary-content",
            "erase-line-complete-boundary-content",
            "erase-display-below-boundary-content",
            "erase-display-above-boundary-content",
            "erase-display-complete-boundary-content",
            "erase-display-scrollback-boundary-content",
            "insert-chars-first-boundary-content",
            "insert-chars-last-boundary-content",
            "insert-chars-wide-straddle",
            "delete-chars-first-boundary-content",
            "delete-chars-last-boundary-content",
            "delete-chars-wide-straddle",
            "insert-lines-region-boundary-content",
            "delete-lines-region-boundary-content",
            "scroll-up-region-boundary-content",
            "scroll-down-region-boundary-content",
            "scroll-index-region-boundary-content",
            "scroll-reverse-index-region-boundary-content",
            "insert-lines-margin-wide-straddle",
            "delete-lines-margin-wide-straddle",
            "scroll-up-margin-wide-straddle",
            "scroll-down-margin-wide-straddle",
        ] {
            assert!(ids.contains(id), "missing boundary characterization {id}");
        }
    }

    #[test]
    fn adversarial_floor_covers_all_five_families() {
        let cases = adversarial();
        assert_eq!(cases.len(), 120);
        for family in [
            "truncated",
            "split-escape",
            "utf8-boundary",
            "oversized-param",
            "interleaved-osc-dcs",
        ] {
            assert_eq!(
                cases.iter().filter(|case| case.id.contains(family)).count(),
                24
            );
        }
    }
}
