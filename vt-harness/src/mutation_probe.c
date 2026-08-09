/* Public-C-ABI-only planted-mutation probe. Expected output is obtained by
 * executing the unmodified library; no constants below are oracle values. */
#include <ghostty/vt.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void must(GhosttyResult result, const char *operation) {
  if (result != GHOSTTY_SUCCESS) {
    fprintf(stderr, "mutation probe %s failed: %d\n", operation, (int)result);
    exit(2);
  }
}

static void feed(GhosttyTerminal terminal, const char *bytes) {
  ghostty_terminal_vt_write(terminal, (const uint8_t *)bytes, strlen(bytes));
}

static void fill_row(GhosttyTerminal terminal, uint16_t row, char value,
                     uint16_t cols) {
  char cursor[32];
  const int written = snprintf(cursor, sizeof(cursor), "\x1b[%u;1H",
                               (unsigned int)row);
  if (written < 0 || (size_t)written >= sizeof(cursor)) {
    fprintf(stderr, "mutation probe cursor formatting failed\n");
    exit(2);
  }
  feed(terminal, cursor);

  char content[81];
  if (cols >= sizeof(content)) {
    fprintf(stderr, "mutation probe content row is too wide\n");
    exit(2);
  }
  memset(content, value, cols);
  content[cols] = '\0';
  feed(terminal, content);
}

static uint64_t fold_u32(uint64_t digest, uint32_t value) {
  for (unsigned int shift = 0; shift < 32; shift += 8) {
    digest ^= (value >> shift) & UINT32_C(0xff);
    digest *= UINT64_C(0x00000100000001b3);
  }
  return digest;
}

static uint64_t content_digest(GhosttyTerminal terminal, uint16_t cols,
                               uint16_t rows) {
  uint64_t digest = UINT64_C(0xcbf29ce484222325);
  for (uint32_t y = 0; y < rows; y++) {
    for (uint16_t x = 0; x < cols; x++) {
      const GhosttyPoint point = {
          .tag = GHOSTTY_POINT_TAG_ACTIVE,
          .value = {.coordinate = {.x = x, .y = y}},
      };
      GhosttyGridRef reference = GHOSTTY_INIT_SIZED(GhosttyGridRef);
      must(ghostty_terminal_grid_ref(terminal, point, &reference), "grid-ref");

      GhosttyCell cell = 0;
      uint32_t codepoint = 0;
      GhosttyCellContentTag tag = GHOSTTY_CELL_CONTENT_CODEPOINT;
      GhosttyCellWide wide = GHOSTTY_CELL_WIDE_NARROW;
      bool has_text = false;
      must(ghostty_grid_ref_cell(&reference, &cell), "grid-ref-cell");
      must(ghostty_cell_get(cell, GHOSTTY_CELL_DATA_CODEPOINT, &codepoint),
           "cell-codepoint");
      must(ghostty_cell_get(cell, GHOSTTY_CELL_DATA_CONTENT_TAG, &tag),
           "cell-content-tag");
      must(ghostty_cell_get(cell, GHOSTTY_CELL_DATA_WIDE, &wide), "cell-wide");
      must(ghostty_cell_get(cell, GHOSTTY_CELL_DATA_HAS_TEXT, &has_text),
           "cell-has-text");

      digest = fold_u32(digest, codepoint);
      digest = fold_u32(digest, (uint32_t)tag);
      digest = fold_u32(digest, (uint32_t)wide);
      digest = fold_u32(digest, has_text ? UINT32_C(1) : UINT32_C(0));
    }
  }
  return digest;
}

int main(void) {
  GhosttyTerminal terminal = NULL;
  const GhosttyTerminalOptions options = {
      .cols = 80,
      .rows = 24,
      .max_scrollback = 64,
  };
  must(ghostty_terminal_new(NULL, &terminal, options), "new");

  feed(terminal, "\x1b[4;6H");
  uint16_t cursor_x = 0;
  uint16_t cursor_y = 0;
  must(ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_X,
                           &cursor_x),
       "cursor-x");
  must(ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_Y,
                           &cursor_y),
       "cursor-y");

  ghostty_terminal_reset(terminal);
  char full_row[81];
  memset(full_row, 'A', 80);
  full_row[80] = '\0';
  feed(terminal, full_row);
  bool pending_wrap = false;
  must(ghostty_terminal_get(terminal,
                           GHOSTTY_TERMINAL_DATA_CURSOR_PENDING_WRAP,
                           &pending_wrap),
       "pending-wrap");

  feed(terminal, "\x1b[?1049h");
  GhosttyTerminalScreen active_screen = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
  must(ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                           &active_screen),
       "active-screen");
  feed(terminal, "\x1b[?1049l\x1b[?25l\x1b[?1000h");

  bool cursor_visible = true;
  bool mouse_tracking = false;
  must(ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE,
                           &cursor_visible),
       "cursor-visible");
  must(ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING,
                           &mouse_tracking),
       "mouse-tracking");

  for (unsigned int index = 0; index < 80; index++) {
    feed(terminal, "line\r\n");
  }
  size_t total_rows = 0;
  size_t scrollback_rows = 0;
  must(ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_TOTAL_ROWS,
                           &total_rows),
       "total-rows");
  must(ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS,
                           &scrollback_rows),
       "scrollback-rows");

  must(ghostty_terminal_resize(terminal, 20, 10, 7, 14), "resize");
  uint16_t cols = 0;
  uint16_t rows = 0;
  uint32_t width_px = 0;
  must(ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_COLS, &cols),
       "cols");
  must(ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_ROWS, &rows),
       "rows");
  must(ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_WIDTH_PX,
                           &width_px),
       "width-px");

  bool wraparound = false;
  must(ghostty_terminal_mode_get(terminal, GHOSTTY_MODE_WRAPAROUND,
                                &wraparound),
       "mode-wraparound");

  ghostty_terminal_reset(terminal);
  fill_row(terminal, 2, 'C', cols);
  feed(terminal, "\x1b[2;10H\x1b[2K");
  const uint64_t erase_complete = content_digest(terminal, cols, rows);

  ghostty_terminal_reset(terminal);
  fill_row(terminal, 3, 'R', cols);
  feed(terminal, "\x1b[3;10H\x1b[K");
  const uint64_t erase_right = content_digest(terminal, cols, rows);

  ghostty_terminal_reset(terminal);
  fill_row(terminal, 1, 'D', cols);
  fill_row(terminal, rows, 'E', cols);
  feed(terminal, "\x1b[2J");
  const uint64_t erase_display = content_digest(terminal, cols, rows);

  printf("cols=%u rows=%u cursor_x=%u cursor_y=%u pending=%u active=%d "
         "visible=%u mouse=%u total=%zu scrollback=%zu width_px=%u mode=%u "
         "content-complete=%016llx content-right=%016llx "
         "content-display=%016llx\n",
         (unsigned int)cols, (unsigned int)rows, (unsigned int)cursor_x,
         (unsigned int)cursor_y, (unsigned int)pending_wrap,
         (int)active_screen, (unsigned int)cursor_visible,
         (unsigned int)mouse_tracking, total_rows, scrollback_rows,
         (unsigned int)width_px, (unsigned int)wraparound,
         (unsigned long long)erase_complete,
         (unsigned long long)erase_right,
         (unsigned long long)erase_display);

  ghostty_terminal_free(terminal);
  return 0;
}
