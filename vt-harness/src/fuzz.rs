//! Seeded grammar-aware structured differential smoke fuzzer.

use libghostty_vt::TerminalOptions;

use crate::abi::{AbiImplementation, Operation};
use crate::differential::Pair;
use crate::{hex, HarnessError, Result};

/// Frozen seed printed in every divergence replay line.
pub const DEFAULT_SEED: u64 = 0x5654_312d_4655_5a5a;

/// Differential fuzzer result included in the receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FuzzSummary {
    /// Structured sequences executed.
    pub executions: u64,
    /// State divergences observed.
    pub divergences: u64,
    /// Frozen generator seed.
    pub seed: u64,
}

/// Runs a bounded or caller-selected deterministic fuzz campaign.
///
/// Each execution is one generated escape/control/text sequence. Selected
/// sequences are split across ABI writes to exercise parser boundary state.
pub fn run(
    left: &dyn AbiImplementation,
    right: &dyn AbiImplementation,
    executions: u64,
    seed: u64,
) -> Result<FuzzSummary> {
    if executions == 0 {
        return Err(HarnessError::new("fuzz executions must be non-zero"));
    }
    let mut random = XorShift64::new(seed);
    let mut pair = new_pair(left, right)?;
    for execution in 0..executions {
        if execution > 0 && execution % 256 == 0 {
            pair = new_pair(left, right)?;
        }
        let input = structured_input(&mut random);
        let replay = format!(
            "fuzz:seed={seed:016x}:execution={execution}:bytes={}",
            hex(&input)
        );
        if execution % 16 == 0 && input.len() > 1 {
            let split = 1 + random.bounded(input.len() - 1);
            pair.apply(&Operation::Write(input[..split].to_vec()), &replay)?;
            pair.apply(&Operation::Write(input[split..].to_vec()), &replay)?;
        } else {
            pair.apply(&Operation::Write(input), &replay)?;
        }
    }
    Ok(FuzzSummary {
        executions,
        divergences: 0,
        seed,
    })
}

fn new_pair(left: &dyn AbiImplementation, right: &dyn AbiImplementation) -> Result<Pair> {
    Pair::new(
        left,
        right,
        TerminalOptions {
            cols: 12,
            rows: 5,
            max_scrollback: 24,
        },
    )
}

fn structured_input(random: &mut XorShift64) -> Vec<u8> {
    const CSI_FINALS: &[u8] = b"ABCDEFGHJKLMPSTX@abcdefgmrhl";
    const UTF8: &[&[u8]] = &[
        "é".as_bytes(),
        "界".as_bytes(),
        "🙂".as_bytes(),
        "e\u{301}".as_bytes(),
        &[0xf0, 0x28, 0x8c, 0xbc],
        &[0xed, 0xa0, 0x80],
    ];
    match random.bounded(10) {
        0 => {
            let length = 1 + random.bounded(32);
            (0..length)
                .map(|_| b' ' + u8::try_from(random.bounded(95)).expect("ASCII range"))
                .collect()
        }
        1 => format!(
            "\x1b[{};{}{}",
            random.bounded(10_000),
            random.bounded(10_000),
            char::from(CSI_FINALS[random.bounded(CSI_FINALS.len())])
        )
        .into_bytes(),
        2 => {
            let params = [
                random.bounded(108),
                random.bounded(256),
                random.bounded(256),
                random.bounded(256),
            ];
            format!(
                "\x1b[{};{};{};{}m{}",
                params[0],
                params[1],
                params[2],
                params[3],
                char::from(b'A' + u8::try_from(random.bounded(26)).expect("letter"))
            )
            .into_bytes()
        }
        3 => {
            const MODES: &[u16] = &[
                1, 5, 6, 7, 12, 25, 45, 69, 1000, 1002, 1003, 1004, 1006, 1016, 1049, 2004, 2026,
                2027, 2031, 2048,
            ];
            format!(
                "\x1b[?{}{}",
                MODES[random.bounded(MODES.len())],
                if random.next() & 1 == 0 { 'h' } else { 'l' }
            )
            .into_bytes()
        }
        4 => {
            let body = format!("fuzz-{:016x}", random.next());
            match random.bounded(4) {
                0 => format!("\x1b]2;{body}\x07").into_bytes(),
                1 => format!("\x1b]7;file:///tmp/{body}\x1b\\").into_bytes(),
                2 => format!("\x1b]8;id=f;https://{body}.invalid\x07").into_bytes(),
                _ => format!(
                    "\x1b]4;{};rgb:{:02x}/{:02x}/{:02x}\x07",
                    random.bounded(256),
                    random.bounded(256),
                    random.bounded(256),
                    random.bounded(256)
                )
                .into_bytes(),
            }
        }
        5 => match random.bounded(4) {
            0 => b"\x1bP$qm\x1b\\".to_vec(),
            1 => b"\x1bP+q544e\x1b\\".to_vec(),
            2 => b"\x1bP1;2|fuzz\x1b\\".to_vec(),
            _ => b"\x1bPunterminated".to_vec(),
        },
        6 => UTF8[random.bounded(UTF8.len())].to_vec(),
        7 => {
            let mut bytes = match random.bounded(4) {
                0 => b"\x1b[".to_vec(),
                1 => b"\x1b]2;".to_vec(),
                2 => b"\x1bP".to_vec(),
                _ => vec![0x1b, 0x18, 0x9b],
            };
            bytes.extend(std::iter::repeat_n(
                b'0' + u8::try_from(random.bounded(10)).expect("digit"),
                random.bounded(48),
            ));
            bytes
        }
        8 => vec![
            b'A',
            [0x00, 0x07, 0x08, 0x09, 0x0a, 0x0d, 0x18, 0x1a, 0x7f][random.bounded(9)],
            b'Z',
        ],
        _ => b"\x1b[?1049hALT\x1b[2J\x1b[?1049l".to_vec(),
    }
}

struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }

    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn bounded(&mut self, bound: usize) -> usize {
        debug_assert!(bound > 0);
        usize::try_from(self.next() % u64::try_from(bound).expect("bound fits u64"))
            .expect("remainder fits usize")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::LinkedAbi;

    #[test]
    fn generator_is_seed_replayable() {
        let mut left = XorShift64::new(DEFAULT_SEED);
        let mut right = XorShift64::new(DEFAULT_SEED);
        for _ in 0..1000 {
            assert_eq!(structured_input(&mut left), structured_input(&mut right));
        }
    }

    #[test]
    fn smoke_self_check_has_no_divergence() {
        let result = run(&LinkedAbi, &LinkedAbi, 128, DEFAULT_SEED).expect("fuzz");
        assert_eq!(result.executions, 128);
        assert_eq!(result.divergences, 0);
    }
}
