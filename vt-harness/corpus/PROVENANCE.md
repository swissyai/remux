# VT1 locally captured PTY corpus provenance

Capture date: 2026-08-06. Host: founder's arm64 macOS 26.5 machine.
Capture geometry: 80 columns x 24 rows. `TERM=xterm-256color`; `LC_ALL=C.UTF-8`.
Method: Python standard-library `pty.openpty` launched each command with stdin,
stdout, and stderr on the slave PTY, set `TIOCSWINSZ`, and recorded exact bytes
read from the master. No GUI, window, CGEvent, screencapture, network, or
telemetry was used. Each committed `.bin` is the exact first 270,000 bytes of
that local PTY capture; bounding prefixes makes corpus size deterministic while
retaining application startup, interaction, and redraw traffic.

Fixtures were generated locally before capture:

- `long.txt`: 30,000 numbered 82-column text lines.
- `art-NN.bin`: repeated 80x24 cursor-addressed, true-color ANSI-art frames.
- `painter.sh`: 220 80x24 cursor-addressed text frames used as the tmux child.

The manifest is closed TSV. It records ID, basename, program family, hex-encoded
actual command/action description, exact byte count, and FNV-1a integrity
value. FNV is only a deterministic corruption guard; differential equality is
performed on state bytes, not digest equality.

## Session inventory

| IDs | Family | Local command and interaction |
|---|---|---|
| session-00..03 | ansi-cat | `/bin/sh -c 'cat art-NN.bin'` |
| session-04..07 | vim | `/usr/bin/vim -Nu NONE -n -i NONE long.txt`; scripted Ctrl-F redraws then `:q!`; shell then cats the corresponding ANSI fixture |
| session-08..11 | less | `/usr/bin/less -R -X long.txt`; scripted Ctrl-F redraws then `q`; shell then cats the corresponding ANSI fixture |
| session-12..15 | top | `/usr/bin/top -l 12 -s 0 -stats pid,command,cpu,mem,threads,time`; shell then cats the corresponding ANSI fixture |
| session-16..19 | tmux | `/opt/homebrew/bin/tmux -L remux-vt1-NN -f /dev/null new-session 'painter.sh NN'`; shell then cats the corresponding ANSI fixture |

`htop` was not installed locally. macOS `/usr/bin/top` supplied the four live
process-table terminal sessions instead; no dependency or network install was
introduced to manufacture a preferred program name.
