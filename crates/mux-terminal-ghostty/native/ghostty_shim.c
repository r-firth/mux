#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include <ghostty/vt.h>

typedef void *mux_ghostty_terminal_t;
typedef void *mux_ghostty_renderer_t;
typedef void *mux_ghostty_response_collector_t;
typedef void *mux_ghostty_key_encoder_t;
typedef void *mux_ghostty_mouse_encoder_t;

typedef struct {
  GhosttyKeyEncoder encoder;
  GhosttyKeyEvent event;
} mux_ghostty_key_encoder_impl_t;

typedef struct {
  GhosttyMouseEncoder encoder;
  GhosttyMouseEvent event;
} mux_ghostty_mouse_encoder_impl_t;

typedef struct {
  uint8_t *bytes;
  size_t len;
  size_t capacity;
  uint16_t cols;
  uint16_t rows;
  uint32_t cell_width_px;
  uint32_t cell_height_px;
  bool failed;
} mux_ghostty_response_collector_impl_t;

typedef struct {
  uint8_t r;
  uint8_t g;
  uint8_t b;
} mux_ghostty_rgb_t;

typedef struct {
  uint32_t text_offset;
  uint32_t text_len;
  mux_ghostty_rgb_t foreground;
  mux_ghostty_rgb_t background;
  mux_ghostty_rgb_t underline_color;
  uint16_t flags;
  uint8_t underline;
  uint8_t width;
  uint8_t semantic;
  uint8_t selected;
  uint8_t hyperlink;
} mux_ghostty_render_cell_t;

typedef struct {
  uint8_t wrapped;
  uint8_t continuation;
  uint8_t dirty;
} mux_ghostty_render_row_t;

typedef struct {
  uint16_t cols;
  uint16_t rows;
  uint8_t dirty;
  mux_ghostty_rgb_t background;
  mux_ghostty_rgb_t foreground;
  uint8_t cursor_has_value;
  uint8_t cursor_visible;
  uint8_t cursor_blinking;
  uint8_t cursor_style;
  uint16_t cursor_x;
  uint16_t cursor_y;
  mux_ghostty_rgb_t cursor_color;
  uint64_t scroll_total;
  uint64_t scroll_offset;
  uint64_t scroll_len;
  mux_ghostty_render_row_t *row_metadata;
  mux_ghostty_render_cell_t *cells;
  uint8_t *text;
  size_t text_len;
} mux_ghostty_render_frame_t;

typedef struct {
  GhosttyRenderState state;
  GhosttyRenderStateRowIterator rows;
  GhosttyRenderStateRowCells cells;
  mux_ghostty_render_row_t *row_metadata;
  size_t row_capacity;
  mux_ghostty_render_cell_t *cell_storage;
  size_t cell_capacity;
  uint8_t *text;
  size_t text_capacity;
} mux_ghostty_renderer_impl_t;

enum {
  MUX_GHOSTTY_CELL_BOLD = 1u << 0,
  MUX_GHOSTTY_CELL_ITALIC = 1u << 1,
  MUX_GHOSTTY_CELL_FAINT = 1u << 2,
  MUX_GHOSTTY_CELL_BLINK = 1u << 3,
  MUX_GHOSTTY_CELL_INVERSE = 1u << 4,
  MUX_GHOSTTY_CELL_INVISIBLE = 1u << 5,
  MUX_GHOSTTY_CELL_STRIKETHROUGH = 1u << 6,
  MUX_GHOSTTY_CELL_OVERLINE = 1u << 7,
};

static mux_ghostty_rgb_t to_mux_rgb(GhosttyColorRgb color) {
  mux_ghostty_rgb_t result = {color.r, color.g, color.b};
  return result;
}

static GhosttyColorRgb resolve_style_color(
    GhosttyStyleColor color,
    const GhosttyRenderStateColors *colors,
    GhosttyColorRgb fallback) {
  switch (color.tag) {
    case GHOSTTY_STYLE_COLOR_RGB:
      return color.value.rgb;
    case GHOSTTY_STYLE_COLOR_PALETTE:
      return colors->palette[color.value.palette];
    default:
      return fallback;
  }
}

static int ensure_text_capacity(
    uint8_t **text,
    size_t *capacity,
    size_t required) {
  if (required <= *capacity) {
    return 1;
  }
  size_t next = *capacity == 0 ? 256 : *capacity;
  while (next < required) {
    if (next > SIZE_MAX / 2) {
      next = required;
      break;
    }
    next *= 2;
  }
  uint8_t *grown = realloc(*text, next);
  if (grown == NULL) {
    return 0;
  }
  *text = grown;
  *capacity = next;
  return 1;
}

static int ensure_array_capacity(
    void **storage,
    size_t *capacity,
    size_t required,
    size_t element_size) {
  if (required <= *capacity) {
    return 1;
  }
  if (element_size == 0 || required > SIZE_MAX / element_size) {
    return 0;
  }
  size_t next = *capacity == 0 ? 64 : *capacity;
  while (next < required) {
    if (next > SIZE_MAX / 2) {
      next = required;
      break;
    }
    next *= 2;
  }
  if (next > SIZE_MAX / element_size) {
    return 0;
  }
  void *grown = realloc(*storage, next * element_size);
  if (grown == NULL) {
    return 0;
  }
  *storage = grown;
  *capacity = next;
  return 1;
}

static void on_write_pty(
    GhosttyTerminal terminal,
    void *userdata,
    const uint8_t *data,
    size_t len) {
  (void)terminal;
  mux_ghostty_response_collector_impl_t *collector = userdata;
  if (collector == NULL || len == 0 || collector->failed) {
    return;
  }
  if (len > SIZE_MAX - collector->len ||
      !ensure_text_capacity(
          &collector->bytes,
          &collector->capacity,
          collector->len + len)) {
    collector->failed = true;
    return;
  }
  memcpy(collector->bytes + collector->len, data, len);
  collector->len += len;
}

static GhosttyString on_xtversion(
    GhosttyTerminal terminal,
    void *userdata) {
  (void)terminal;
  (void)userdata;
  static const uint8_t version[] = "mux 0.1.0";
  GhosttyString result = {version, sizeof(version) - 1};
  return result;
}

static bool on_color_scheme(
    GhosttyTerminal terminal,
    void *userdata,
    GhosttyColorScheme *out_scheme) {
  (void)terminal;
  (void)userdata;
  *out_scheme = GHOSTTY_COLOR_SCHEME_DARK;
  return true;
}

static bool on_device_attributes(
    GhosttyTerminal terminal,
    void *userdata,
    GhosttyDeviceAttributes *out_attributes) {
  (void)terminal;
  (void)userdata;
  memset(out_attributes, 0, sizeof(*out_attributes));
  out_attributes->primary.conformance_level = GHOSTTY_DA_CONFORMANCE_VT220;
  out_attributes->primary.features[0] = GHOSTTY_DA_FEATURE_SELECTIVE_ERASE;
  out_attributes->primary.features[1] = GHOSTTY_DA_FEATURE_WINDOWING;
  out_attributes->primary.features[2] = GHOSTTY_DA_FEATURE_ANSI_COLOR;
  out_attributes->primary.features[3] = GHOSTTY_DA_FEATURE_CLIPBOARD;
  out_attributes->primary.num_features = 4;
  out_attributes->secondary.device_type = GHOSTTY_DA_DEVICE_TYPE_VT220;
  out_attributes->secondary.firmware_version = 100;
  out_attributes->secondary.rom_cartridge = 0;
  out_attributes->tertiary.unit_id = 0;
  return true;
}

static bool on_size(
    GhosttyTerminal terminal,
    void *userdata,
    GhosttySizeReportSize *out_size) {
  (void)terminal;
  mux_ghostty_response_collector_impl_t *collector = userdata;
  if (collector == NULL) {
    return false;
  }
  out_size->columns = collector->cols;
  out_size->rows = collector->rows;
  out_size->cell_width = collector->cell_width_px;
  out_size->cell_height = collector->cell_height_px;
  return true;
}

int32_t mux_ghostty_response_collector_new(
    uint16_t cols,
    uint16_t rows,
    mux_ghostty_response_collector_t *out_collector) {
  mux_ghostty_response_collector_impl_t *collector = calloc(1, sizeof(*collector));
  if (collector == NULL) {
    return (int32_t)GHOSTTY_OUT_OF_MEMORY;
  }
  collector->cols = cols;
  collector->rows = rows;
  *out_collector = collector;
  return (int32_t)GHOSTTY_SUCCESS;
}

void mux_ghostty_response_collector_free(
    mux_ghostty_response_collector_t raw_collector) {
  mux_ghostty_response_collector_impl_t *collector = raw_collector;
  if (collector == NULL) {
    return;
  }
  free(collector->bytes);
  free(collector);
}

void mux_ghostty_response_collector_set_size(
    mux_ghostty_response_collector_t raw_collector,
    uint16_t cols,
    uint16_t rows,
    uint32_t cell_width_px,
    uint32_t cell_height_px) {
  mux_ghostty_response_collector_impl_t *collector = raw_collector;
  if (collector == NULL) {
    return;
  }
  collector->cols = cols;
  collector->rows = rows;
  collector->cell_width_px = cell_width_px;
  collector->cell_height_px = cell_height_px;
}

int32_t mux_ghostty_response_collector_peek(
    mux_ghostty_response_collector_t raw_collector,
    const uint8_t **out_bytes,
    size_t *out_len) {
  mux_ghostty_response_collector_impl_t *collector = raw_collector;
  if (collector == NULL || out_bytes == NULL || out_len == NULL) {
    return (int32_t)GHOSTTY_INVALID_VALUE;
  }
  if (collector->failed) {
    collector->failed = false;
    collector->len = 0;
    return (int32_t)GHOSTTY_OUT_OF_MEMORY;
  }
  *out_bytes = collector->bytes;
  *out_len = collector->len;
  return (int32_t)GHOSTTY_SUCCESS;
}

void mux_ghostty_response_collector_clear(
    mux_ghostty_response_collector_t raw_collector) {
  mux_ghostty_response_collector_impl_t *collector = raw_collector;
  if (collector != NULL) {
    collector->len = 0;
  }
}

int32_t mux_ghostty_terminal_enable_responses(
    mux_ghostty_terminal_t raw_terminal,
    mux_ghostty_response_collector_t raw_collector) {
  GhosttyTerminal terminal = (GhosttyTerminal)raw_terminal;
  mux_ghostty_response_collector_impl_t *collector = raw_collector;
  if (terminal == NULL || collector == NULL) {
    return (int32_t)GHOSTTY_INVALID_VALUE;
  }
  GhosttyResult result = ghostty_terminal_get(
      terminal, GHOSTTY_TERMINAL_DATA_COLS, &collector->cols);
  if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  result = ghostty_terminal_get(
      terminal, GHOSTTY_TERMINAL_DATA_ROWS, &collector->rows);
  if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  result = ghostty_terminal_set(
      terminal, GHOSTTY_TERMINAL_OPT_USERDATA, collector);
  if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  result = ghostty_terminal_set(
      terminal, GHOSTTY_TERMINAL_OPT_WRITE_PTY, (const void *)on_write_pty);
  if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  result = ghostty_terminal_set(
      terminal, GHOSTTY_TERMINAL_OPT_XTVERSION, (const void *)on_xtversion);
  if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  result = ghostty_terminal_set(
      terminal, GHOSTTY_TERMINAL_OPT_COLOR_SCHEME, (const void *)on_color_scheme);
  if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  result = ghostty_terminal_set(
      terminal,
      GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES,
      (const void *)on_device_attributes);
  if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  return (int32_t)ghostty_terminal_set(
      terminal, GHOSTTY_TERMINAL_OPT_SIZE, (const void *)on_size);
}

static int32_t enable_continuation_tracking(GhosttyTerminal terminal) {
  size_t continuation_limit = 16 * 1024 * 1024;
  return (int32_t)ghostty_terminal_set(
      terminal,
      GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES,
      &continuation_limit);
}

int32_t mux_ghostty_terminal_new(
    uint16_t cols,
    uint16_t rows,
    mux_ghostty_terminal_t *out_terminal) {
  GhosttyTerminal terminal = NULL;
  GhosttyResult result = ghostty_terminal_new(NULL, &terminal, cols, rows);
  if (result != GHOSTTY_SUCCESS) {
    return (int32_t)result;
  }
  result = (GhosttyResult)enable_continuation_tracking(terminal);
  if (result != GHOSTTY_SUCCESS) {
    ghostty_terminal_free(terminal);
    return (int32_t)result;
  }
  *out_terminal = terminal;
  return 0;
}

int32_t mux_ghostty_terminal_apply_theme(
    mux_ghostty_terminal_t raw_terminal,
    const mux_ghostty_rgb_t *background,
    const mux_ghostty_rgb_t *foreground,
    const mux_ghostty_rgb_t *cursor,
    const uint8_t *palette_indices,
    const mux_ghostty_rgb_t *palette_colors,
    size_t palette_len) {
  GhosttyTerminal terminal = (GhosttyTerminal)raw_terminal;
  if (terminal == NULL ||
      (palette_len > 0 &&
       (palette_indices == NULL || palette_colors == NULL))) {
    return (int32_t)GHOSTTY_INVALID_VALUE;
  }
  GhosttyResult result;
  if (background != NULL) {
    GhosttyColorRgb color = {background->r, background->g, background->b};
    result = ghostty_terminal_set(
        terminal, GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, &color);
    if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  }
  if (foreground != NULL) {
    GhosttyColorRgb color = {foreground->r, foreground->g, foreground->b};
    result = ghostty_terminal_set(
        terminal, GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, &color);
    if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  }
  if (cursor != NULL) {
    GhosttyColorRgb color = {cursor->r, cursor->g, cursor->b};
    result = ghostty_terminal_set(
        terminal, GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, &color);
    if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  }
  if (palette_len > 0) {
    GhosttyColorRgb palette[256];
    result = ghostty_terminal_get(
        terminal, GHOSTTY_TERMINAL_DATA_COLOR_PALETTE, palette);
    if (result != GHOSTTY_SUCCESS) return (int32_t)result;
    for (size_t i = 0; i < palette_len; i++) {
      const uint8_t index = palette_indices[i];
      palette[index] = (GhosttyColorRgb){
          palette_colors[i].r,
          palette_colors[i].g,
          palette_colors[i].b,
      };
    }
    result = ghostty_terminal_set(
        terminal, GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, palette);
    if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  }
  return (int32_t)GHOSTTY_SUCCESS;
}

int32_t mux_ghostty_terminal_restore(
    const uint8_t *snapshot,
    size_t snapshot_len,
    mux_ghostty_terminal_t *out_terminal) {
  GhosttySnapshotDecoder decoder = NULL;
  GhosttyResult result = ghostty_snapshot_decoder_new_buf(
      NULL, &decoder, snapshot, snapshot_len);
  if (result != GHOSTTY_SUCCESS) {
    return (int32_t)result;
  }

  GhosttyTerminal terminal = NULL;
  result = ghostty_snapshot_decoder_decode(decoder, &terminal);
  ghostty_snapshot_decoder_free(decoder);
  if (result != GHOSTTY_SUCCESS) {
    return (int32_t)result;
  }

  result = (GhosttyResult)enable_continuation_tracking(terminal);
  if (result != GHOSTTY_SUCCESS) {
    ghostty_terminal_free(terminal);
    return (int32_t)result;
  }
  *out_terminal = terminal;
  return 0;
}

void mux_ghostty_terminal_free(mux_ghostty_terminal_t terminal) {
  ghostty_terminal_free((GhosttyTerminal)terminal);
}

void mux_ghostty_terminal_write(
    mux_ghostty_terminal_t terminal,
    const uint8_t *bytes,
    size_t len) {
  ghostty_terminal_vt_write((GhosttyTerminal)terminal, bytes, len);
}

int32_t mux_ghostty_terminal_resize(
    mux_ghostty_terminal_t terminal,
    uint16_t cols,
    uint16_t rows,
    uint32_t cell_width_px,
    uint32_t cell_height_px) {
  return (int32_t)ghostty_terminal_resize(
      (GhosttyTerminal)terminal,
      cols,
      rows,
      cell_width_px,
      cell_height_px);
}

int32_t mux_ghostty_terminal_set_selection(
    mux_ghostty_terminal_t raw_terminal,
    uint16_t anchor_x,
    uint16_t anchor_y,
    uint16_t focus_x,
    uint16_t focus_y,
    bool rectangular) {
  GhosttyTerminal terminal = (GhosttyTerminal)raw_terminal;
  GhosttyPoint anchor_point = {
      .tag = GHOSTTY_POINT_TAG_VIEWPORT,
      .value = {.coordinate = {.x = anchor_x, .y = anchor_y}},
  };
  GhosttyPoint focus_point = {
      .tag = GHOSTTY_POINT_TAG_VIEWPORT,
      .value = {.coordinate = {.x = focus_x, .y = focus_y}},
  };
  GhosttyGridRef anchor = GHOSTTY_INIT_SIZED(GhosttyGridRef);
  GhosttyGridRef focus = GHOSTTY_INIT_SIZED(GhosttyGridRef);
  GhosttyResult result = ghostty_terminal_grid_ref(terminal, anchor_point, &anchor);
  if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  result = ghostty_terminal_grid_ref(terminal, focus_point, &focus);
  if (result != GHOSTTY_SUCCESS) return (int32_t)result;
  GhosttySelection selection = GHOSTTY_INIT_SIZED(GhosttySelection);
  selection.start = anchor;
  selection.end = focus;
  selection.rectangle = rectangular;
  return (int32_t)ghostty_terminal_set(
      terminal, GHOSTTY_TERMINAL_OPT_SELECTION, &selection);
}

int32_t mux_ghostty_terminal_clear_selection(
    mux_ghostty_terminal_t terminal) {
  return (int32_t)ghostty_terminal_set(
      (GhosttyTerminal)terminal, GHOSTTY_TERMINAL_OPT_SELECTION, NULL);
}

int32_t mux_ghostty_terminal_selected_text(
    mux_ghostty_terminal_t raw_terminal,
    uint8_t **out_bytes,
    size_t *out_len,
    bool *out_has_selection) {
  if (out_bytes == NULL || out_len == NULL || out_has_selection == NULL) {
    return (int32_t)GHOSTTY_INVALID_VALUE;
  }
  *out_bytes = NULL;
  *out_len = 0;
  *out_has_selection = false;
  GhosttyTerminal terminal = (GhosttyTerminal)raw_terminal;
  GhosttySelection selection = GHOSTTY_INIT_SIZED(GhosttySelection);
  GhosttyResult result = ghostty_terminal_get(
      terminal, GHOSTTY_TERMINAL_DATA_SELECTION, &selection);
  if (result == GHOSTTY_NO_VALUE) return (int32_t)GHOSTTY_SUCCESS;
  if (result != GHOSTTY_SUCCESS) return (int32_t)result;

  GhosttyTerminalSelectionFormatOptions options =
      GHOSTTY_INIT_SIZED(GhosttyTerminalSelectionFormatOptions);
  options.emit = GHOSTTY_FORMATTER_FORMAT_PLAIN;
  options.unwrap = true;
  options.trim = true;
  options.selection = &selection;
  uint8_t *ghostty_bytes = NULL;
  size_t ghostty_len = 0;
  result = ghostty_terminal_selection_format_alloc(
      terminal, NULL, options, &ghostty_bytes, &ghostty_len);
  if (result != GHOSTTY_SUCCESS) return (int32_t)result;

  uint8_t *copy = NULL;
  if (ghostty_len > 0) {
    copy = malloc(ghostty_len);
    if (copy == NULL) {
      ghostty_free(NULL, ghostty_bytes, ghostty_len);
      return (int32_t)GHOSTTY_OUT_OF_MEMORY;
    }
    memcpy(copy, ghostty_bytes, ghostty_len);
  }
  ghostty_free(NULL, ghostty_bytes, ghostty_len);
  *out_bytes = copy;
  *out_len = ghostty_len;
  *out_has_selection = true;
  return (int32_t)GHOSTTY_SUCCESS;
}

int32_t mux_ghostty_terminal_encode_paste(
    mux_ghostty_terminal_t raw_terminal,
    const uint8_t *bytes,
    size_t len,
    uint8_t **out_bytes,
    size_t *out_len) {
  if ((len > 0 && bytes == NULL) || out_bytes == NULL || out_len == NULL ||
      len > SIZE_MAX - 12) {
    return (int32_t)GHOSTTY_INVALID_VALUE;
  }
  *out_bytes = NULL;
  *out_len = 0;
  GhosttyTerminal terminal = (GhosttyTerminal)raw_terminal;
  GhosttyTerminalModeConfig mode = {
      .mode = GHOSTTY_MODE_BRACKETED_PASTE,
      .value = false,
  };
  GhosttyResult result = ghostty_terminal_get(
      terminal, GHOSTTY_TERMINAL_DATA_MODE, &mode);
  if (result != GHOSTTY_SUCCESS) return (int32_t)result;

  char *scratch = malloc(len == 0 ? 1 : len);
  if (scratch == NULL) return (int32_t)GHOSTTY_OUT_OF_MEMORY;
  if (len > 0) memcpy(scratch, bytes, len);
  size_t capacity = len + 12;
  uint8_t *encoded = malloc(capacity == 0 ? 1 : capacity);
  if (encoded == NULL) {
    free(scratch);
    return (int32_t)GHOSTTY_OUT_OF_MEMORY;
  }
  size_t written = 0;
  result = ghostty_paste_encode(
      scratch, len, mode.value, (char *)encoded, capacity, &written);
  free(scratch);
  if (result != GHOSTTY_SUCCESS) {
    free(encoded);
    return (int32_t)result;
  }
  *out_bytes = encoded;
  *out_len = written;
  return (int32_t)GHOSTTY_SUCCESS;
}

void mux_ghostty_buffer_free(uint8_t *bytes) {
  free(bytes);
}

void mux_ghostty_terminal_scroll_viewport(
    mux_ghostty_terminal_t raw_terminal,
    uint8_t tag,
    int64_t value) {
  GhosttyTerminalScrollViewport scroll;
  memset(&scroll, 0, sizeof(scroll));
  switch (tag) {
    case 0:
      scroll.tag = GHOSTTY_SCROLL_VIEWPORT_TOP;
      break;
    case 1:
      scroll.tag = GHOSTTY_SCROLL_VIEWPORT_BOTTOM;
      break;
    case 2:
      scroll.tag = GHOSTTY_SCROLL_VIEWPORT_DELTA;
      scroll.value.delta = (intptr_t)value;
      break;
    default:
      return;
  }
  ghostty_terminal_scroll_viewport((GhosttyTerminal)raw_terminal, scroll);
}

int32_t mux_ghostty_key_encoder_new(
    mux_ghostty_key_encoder_t *out_encoder) {
  if (out_encoder == NULL) return (int32_t)GHOSTTY_INVALID_VALUE;
  *out_encoder = NULL;
  mux_ghostty_key_encoder_impl_t *input =
      calloc(1, sizeof(mux_ghostty_key_encoder_impl_t));
  if (input == NULL) return (int32_t)GHOSTTY_OUT_OF_MEMORY;
  GhosttyResult result = ghostty_key_encoder_new(NULL, &input->encoder);
  if (result != GHOSTTY_SUCCESS) {
    free(input);
    return (int32_t)result;
  }
  result = ghostty_key_event_new(NULL, &input->event);
  if (result != GHOSTTY_SUCCESS) {
    ghostty_key_encoder_free(input->encoder);
    free(input);
    return (int32_t)result;
  }
  *out_encoder = input;
  return (int32_t)GHOSTTY_SUCCESS;
}

void mux_ghostty_key_encoder_free(mux_ghostty_key_encoder_t raw_encoder) {
  mux_ghostty_key_encoder_impl_t *input = raw_encoder;
  if (input == NULL) return;
  ghostty_key_event_free(input->event);
  ghostty_key_encoder_free(input->encoder);
  free(input);
}

static GhosttyKey mux_ghostty_key(uint8_t tag, uint8_t function_number) {
  switch (tag) {
    case 1: return GHOSTTY_KEY_BACKSPACE;
    case 2: return GHOSTTY_KEY_ENTER;
    case 3: return GHOSTTY_KEY_TAB;
    case 4: return GHOSTTY_KEY_SPACE;
    case 5: return GHOSTTY_KEY_DELETE;
    case 6: return GHOSTTY_KEY_INSERT;
    case 7: return GHOSTTY_KEY_HOME;
    case 8: return GHOSTTY_KEY_END;
    case 9: return GHOSTTY_KEY_PAGE_UP;
    case 10: return GHOSTTY_KEY_PAGE_DOWN;
    case 11: return GHOSTTY_KEY_ARROW_UP;
    case 12: return GHOSTTY_KEY_ARROW_DOWN;
    case 13: return GHOSTTY_KEY_ARROW_LEFT;
    case 14: return GHOSTTY_KEY_ARROW_RIGHT;
    case 15: return GHOSTTY_KEY_ESCAPE;
    case 16:
      if (function_number >= 1 && function_number <= 25) {
        return (GhosttyKey)(GHOSTTY_KEY_F1 + function_number - 1);
      }
      return GHOSTTY_KEY_UNIDENTIFIED;
    case 17: return GHOSTTY_KEY_NUMPAD_ENTER;
    case 18: return GHOSTTY_KEY_BACKQUOTE;
    case 19: return GHOSTTY_KEY_BACKSLASH;
    case 20: return GHOSTTY_KEY_BRACKET_LEFT;
    case 21: return GHOSTTY_KEY_BRACKET_RIGHT;
    case 22: return GHOSTTY_KEY_COMMA;
    case 23:
      if (function_number <= 9) {
        return (GhosttyKey)(GHOSTTY_KEY_DIGIT_0 + function_number);
      }
      return GHOSTTY_KEY_UNIDENTIFIED;
    case 24: return GHOSTTY_KEY_EQUAL;
    case 25:
      if (function_number < 26) {
        return (GhosttyKey)(GHOSTTY_KEY_A + function_number);
      }
      return GHOSTTY_KEY_UNIDENTIFIED;
    case 26: return GHOSTTY_KEY_MINUS;
    case 27: return GHOSTTY_KEY_PERIOD;
    case 28: return GHOSTTY_KEY_QUOTE;
    case 29: return GHOSTTY_KEY_SEMICOLON;
    case 30: return GHOSTTY_KEY_SLASH;
    case 31: return GHOSTTY_KEY_INTL_BACKSLASH;
    case 32: return GHOSTTY_KEY_INTL_RO;
    case 33: return GHOSTTY_KEY_INTL_YEN;
    default: return GHOSTTY_KEY_UNIDENTIFIED;
  }
}

int32_t mux_ghostty_key_encoder_encode(
    mux_ghostty_key_encoder_t raw_encoder,
    mux_ghostty_terminal_t raw_terminal,
    uint8_t action,
    uint8_t key_tag,
    uint8_t function_number,
    uint16_t modifiers,
    uint16_t consumed_modifiers,
    const uint8_t *utf8,
    size_t utf8_len,
    uint32_t unshifted_codepoint,
    bool composing,
    uint8_t *out_bytes,
    size_t out_capacity,
    size_t *out_len) {
  mux_ghostty_key_encoder_impl_t *input = raw_encoder;
  if (input == NULL || raw_terminal == NULL || out_len == NULL ||
      (utf8_len > 0 && utf8 == NULL) ||
      (out_capacity > 0 && out_bytes == NULL) || action > 2) {
    return (int32_t)GHOSTTY_INVALID_VALUE;
  }
  ghostty_key_encoder_setopt_from_terminal(
      input->encoder, (GhosttyTerminal)raw_terminal);
  GhosttyOptionAsAlt option_as_alt = GHOSTTY_OPTION_AS_ALT_TRUE;
  ghostty_key_encoder_setopt(
      input->encoder,
      GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
      &option_as_alt);
  ghostty_key_event_set_action(input->event, (GhosttyKeyAction)action);
  ghostty_key_event_set_key(
      input->event, mux_ghostty_key(key_tag, function_number));
  ghostty_key_event_set_mods(input->event, modifiers);
  ghostty_key_event_set_consumed_mods(input->event, consumed_modifiers);
  ghostty_key_event_set_composing(input->event, composing);
  ghostty_key_event_set_utf8(
      input->event,
      utf8_len == 0 ? NULL : (const char *)utf8,
      utf8_len);
  ghostty_key_event_set_unshifted_codepoint(
      input->event, unshifted_codepoint);
  return (int32_t)ghostty_key_encoder_encode(
      input->encoder,
      input->event,
      (char *)out_bytes,
      out_capacity,
      out_len);
}

int32_t mux_ghostty_mouse_encoder_new(
    mux_ghostty_mouse_encoder_t *out_encoder) {
  if (out_encoder == NULL) return (int32_t)GHOSTTY_INVALID_VALUE;
  *out_encoder = NULL;
  mux_ghostty_mouse_encoder_impl_t *input = calloc(1, sizeof(*input));
  if (input == NULL) return (int32_t)GHOSTTY_OUT_OF_MEMORY;
  GhosttyResult result = ghostty_mouse_encoder_new(NULL, &input->encoder);
  if (result != GHOSTTY_SUCCESS) {
    free(input);
    return (int32_t)result;
  }
  result = ghostty_mouse_event_new(NULL, &input->event);
  if (result != GHOSTTY_SUCCESS) {
    ghostty_mouse_encoder_free(input->encoder);
    free(input);
    return (int32_t)result;
  }
  bool track_last_cell = true;
  ghostty_mouse_encoder_setopt(
      input->encoder,
      GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL,
      &track_last_cell);
  *out_encoder = input;
  return (int32_t)GHOSTTY_SUCCESS;
}

void mux_ghostty_mouse_encoder_free(mux_ghostty_mouse_encoder_t raw_encoder) {
  mux_ghostty_mouse_encoder_impl_t *input = raw_encoder;
  if (input == NULL) return;
  ghostty_mouse_event_free(input->event);
  ghostty_mouse_encoder_free(input->encoder);
  free(input);
}

int32_t mux_ghostty_mouse_encoder_encode(
    mux_ghostty_mouse_encoder_t raw_encoder,
    mux_ghostty_terminal_t raw_terminal,
    uint8_t action,
    uint8_t button,
    uint16_t modifiers,
    float x,
    float y,
    uint32_t screen_width,
    uint32_t screen_height,
    uint32_t cell_width,
    uint32_t cell_height,
    uint32_t padding_top,
    uint32_t padding_bottom,
    uint32_t padding_right,
    uint32_t padding_left,
    bool any_button_pressed,
    uint8_t *out_bytes,
    size_t out_capacity,
    size_t *out_len) {
  mux_ghostty_mouse_encoder_impl_t *input = raw_encoder;
  if (input == NULL || raw_terminal == NULL || action > 2 || button > 11 ||
      cell_width == 0 || cell_height == 0 || out_len == NULL ||
      (out_capacity > 0 && out_bytes == NULL)) {
    return (int32_t)GHOSTTY_INVALID_VALUE;
  }
  ghostty_mouse_encoder_setopt_from_terminal(
      input->encoder, (GhosttyTerminal)raw_terminal);
  GhosttyMouseEncoderSize size = {
      .size = sizeof(GhosttyMouseEncoderSize),
      .screen_width = screen_width,
      .screen_height = screen_height,
      .cell_width = cell_width,
      .cell_height = cell_height,
      .padding_top = padding_top,
      .padding_bottom = padding_bottom,
      .padding_right = padding_right,
      .padding_left = padding_left,
  };
  ghostty_mouse_encoder_setopt(
      input->encoder, GHOSTTY_MOUSE_ENCODER_OPT_SIZE, &size);
  ghostty_mouse_encoder_setopt(
      input->encoder,
      GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
      &any_button_pressed);
  ghostty_mouse_event_set_action(input->event, (GhosttyMouseAction)action);
  if (button == 0) {
    ghostty_mouse_event_clear_button(input->event);
  } else {
    ghostty_mouse_event_set_button(input->event, (GhosttyMouseButton)button);
  }
  ghostty_mouse_event_set_mods(input->event, modifiers);
  ghostty_mouse_event_set_position(
      input->event, (GhosttyMousePosition){.x = x, .y = y});
  return (int32_t)ghostty_mouse_encoder_encode(
      input->encoder,
      input->event,
      (char *)out_bytes,
      out_capacity,
      out_len);
}

int32_t mux_ghostty_terminal_snapshot(
    mux_ghostty_terminal_t terminal,
    uint8_t **out_bytes,
    size_t *out_len) {
  return (int32_t)ghostty_snapshot_encode_alloc(
      (GhosttyTerminal)terminal,
      NULL,
      out_bytes,
      out_len);
}

void mux_ghostty_snapshot_free(uint8_t *bytes, size_t len) {
  ghostty_free(NULL, bytes, len);
}

int32_t mux_ghostty_renderer_new(mux_ghostty_renderer_t *out_renderer) {
  mux_ghostty_renderer_impl_t *renderer =
      calloc(1, sizeof(mux_ghostty_renderer_impl_t));
  if (renderer == NULL) {
    return (int32_t)GHOSTTY_OUT_OF_MEMORY;
  }

  GhosttyResult result = ghostty_render_state_new(NULL, &renderer->state);
  if (result != GHOSTTY_SUCCESS) {
    free(renderer);
    return (int32_t)result;
  }
  result = ghostty_render_state_row_iterator_new(NULL, &renderer->rows);
  if (result != GHOSTTY_SUCCESS) {
    ghostty_render_state_free(renderer->state);
    free(renderer);
    return (int32_t)result;
  }
  result = ghostty_render_state_row_cells_new(NULL, &renderer->cells);
  if (result != GHOSTTY_SUCCESS) {
    ghostty_render_state_row_iterator_free(renderer->rows);
    ghostty_render_state_free(renderer->state);
    free(renderer);
    return (int32_t)result;
  }

  *out_renderer = renderer;
  return (int32_t)GHOSTTY_SUCCESS;
}

void mux_ghostty_renderer_free(mux_ghostty_renderer_t raw_renderer) {
  mux_ghostty_renderer_impl_t *renderer = raw_renderer;
  if (renderer == NULL) {
    return;
  }
  ghostty_render_state_row_cells_free(renderer->cells);
  ghostty_render_state_row_iterator_free(renderer->rows);
  ghostty_render_state_free(renderer->state);
  free(renderer->row_metadata);
  free(renderer->cell_storage);
  free(renderer->text);
  free(renderer);
}

void mux_ghostty_render_frame_free(mux_ghostty_render_frame_t *frame) {
  if (frame == NULL) {
    return;
  }
  // Frame storage is borrowed from the renderer and remains valid until the
  // next frame call. Releasing a frame only invalidates the caller's view.
  memset(frame, 0, sizeof(*frame));
}

int32_t mux_ghostty_renderer_frame(
    mux_ghostty_renderer_t raw_renderer,
    mux_ghostty_terminal_t raw_terminal,
    mux_ghostty_render_frame_t *out_frame) {
  mux_ghostty_renderer_impl_t *renderer = raw_renderer;
  GhosttyTerminal terminal = (GhosttyTerminal)raw_terminal;
  if (renderer == NULL || terminal == NULL || out_frame == NULL) {
    return (int32_t)GHOSTTY_INVALID_VALUE;
  }

  mux_ghostty_render_frame_t frame;
  memset(&frame, 0, sizeof(frame));

  GhosttyResult result = ghostty_render_state_update(renderer->state, terminal);
  if (result != GHOSTTY_SUCCESS) {
    return (int32_t)result;
  }

  GhosttyRenderStateDirty dirty = GHOSTTY_RENDER_STATE_DIRTY_FALSE;
  result = ghostty_render_state_get(
      renderer->state, GHOSTTY_RENDER_STATE_DATA_DIRTY, &dirty);
  if (result != GHOSTTY_SUCCESS) goto error;
  frame.dirty = (uint8_t)dirty;

  result = ghostty_render_state_get(
      renderer->state, GHOSTTY_RENDER_STATE_DATA_COLS, &frame.cols);
  if (result != GHOSTTY_SUCCESS) goto error;
  result = ghostty_render_state_get(
      renderer->state, GHOSTTY_RENDER_STATE_DATA_ROWS, &frame.rows);
  if (result != GHOSTTY_SUCCESS) goto error;

  GhosttyTerminalScrollbar scrollbar = {0};
  result = ghostty_terminal_get(
      terminal, GHOSTTY_TERMINAL_DATA_SCROLLBAR, &scrollbar);
  if (result != GHOSTTY_SUCCESS) goto error;
  frame.scroll_total = scrollbar.total;
  frame.scroll_offset = scrollbar.offset;
  frame.scroll_len = scrollbar.len;

  GhosttyRenderStateColors colors = GHOSTTY_INIT_SIZED(GhosttyRenderStateColors);
  result = ghostty_render_state_colors_get(renderer->state, &colors);
  if (result != GHOSTTY_SUCCESS) goto error;
  frame.background = to_mux_rgb(colors.background);
  frame.foreground = to_mux_rgb(colors.foreground);
  frame.cursor_color = to_mux_rgb(
      colors.cursor_has_value ? colors.cursor : colors.foreground);

  bool cursor_in_viewport = false;
  bool cursor_visible = false;
  bool cursor_blinking = false;
  result = ghostty_render_state_get(
      renderer->state,
      GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE,
      &cursor_in_viewport);
  if (result != GHOSTTY_SUCCESS) goto error;
  result = ghostty_render_state_get(
      renderer->state, GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE,
      &cursor_visible);
  if (result != GHOSTTY_SUCCESS) goto error;
  result = ghostty_render_state_get(
      renderer->state, GHOSTTY_RENDER_STATE_DATA_CURSOR_BLINKING,
      &cursor_blinking);
  if (result != GHOSTTY_SUCCESS) goto error;
  frame.cursor_has_value = cursor_in_viewport ? 1 : 0;
  frame.cursor_visible = cursor_visible ? 1 : 0;
  frame.cursor_blinking = cursor_blinking ? 1 : 0;
  if (cursor_in_viewport) {
    GhosttyRenderStateCursorVisualStyle cursor_style;
    result = ghostty_render_state_get(
        renderer->state, GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X,
        &frame.cursor_x);
    if (result != GHOSTTY_SUCCESS) goto error;
    result = ghostty_render_state_get(
        renderer->state, GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y,
        &frame.cursor_y);
    if (result != GHOSTTY_SUCCESS) goto error;
    result = ghostty_render_state_get(
        renderer->state, GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE,
        &cursor_style);
    if (result != GHOSTTY_SUCCESS) goto error;
    frame.cursor_style = (uint8_t)cursor_style;
  }

  const size_t cell_count = (size_t)frame.cols * (size_t)frame.rows;
  if (!ensure_array_capacity(
          (void **)&renderer->row_metadata,
          &renderer->row_capacity,
          frame.rows,
          sizeof(*renderer->row_metadata)) ||
      !ensure_array_capacity(
          (void **)&renderer->cell_storage,
          &renderer->cell_capacity,
          cell_count,
          sizeof(*renderer->cell_storage))) {
    result = GHOSTTY_OUT_OF_MEMORY;
    goto error;
  }
  frame.row_metadata = renderer->row_metadata;
  frame.cells = renderer->cell_storage;
  frame.text = renderer->text;
  if (frame.rows > 0) {
    memset(frame.row_metadata, 0, frame.rows * sizeof(*frame.row_metadata));
  }
  if (cell_count > 0) {
    memset(frame.cells, 0, cell_count * sizeof(*frame.cells));
  }

  result = ghostty_render_state_get(
      renderer->state, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
      &renderer->rows);
  if (result != GHOSTTY_SUCCESS) goto error;

  uint16_t y = 0;
  while (y < frame.rows && ghostty_render_state_row_iterator_next(renderer->rows)) {
    mux_ghostty_render_row_t *out_row = &frame.row_metadata[y];
    bool row_dirty = false;
    GhosttyRow raw_row = 0;
    result = ghostty_render_state_row_get(
        renderer->rows, GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY, &row_dirty);
    if (result != GHOSTTY_SUCCESS) goto error;
    result = ghostty_render_state_row_get(
        renderer->rows, GHOSTTY_RENDER_STATE_ROW_DATA_RAW, &raw_row);
    if (result != GHOSTTY_SUCCESS) goto error;
    out_row->dirty = row_dirty ? 1 : 0;

    bool wrapped = false;
    bool continuation = false;
    result = ghostty_row_get(raw_row, GHOSTTY_ROW_DATA_WRAP, &wrapped);
    if (result != GHOSTTY_SUCCESS) goto error;
    result = ghostty_row_get(
        raw_row, GHOSTTY_ROW_DATA_WRAP_CONTINUATION, &continuation);
    if (result != GHOSTTY_SUCCESS) goto error;
    out_row->wrapped = wrapped ? 1 : 0;
    out_row->continuation = continuation ? 1 : 0;

    result = ghostty_render_state_row_get(
        renderer->rows, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
        &renderer->cells);
    if (result != GHOSTTY_SUCCESS) goto error;

    uint16_t x = 0;
    while (x < frame.cols && ghostty_render_state_row_cells_next(renderer->cells)) {
      mux_ghostty_render_cell_t *out_cell =
          &frame.cells[(size_t)y * frame.cols + x];
      GhosttyCell raw_cell = 0;
      GhosttyStyle style = GHOSTTY_INIT_SIZED(GhosttyStyle);
      result = ghostty_render_state_row_cells_get(
          renderer->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
          &raw_cell);
      if (result != GHOSTTY_SUCCESS) goto error;
      result = ghostty_render_state_row_cells_get(
          renderer->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
          &style);
      if (result != GHOSTTY_SUCCESS) goto error;

      GhosttyColorRgb foreground = colors.foreground;
      GhosttyColorRgb background = colors.background;
      GhosttyResult color_result = ghostty_render_state_row_cells_get(
          renderer->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
          &foreground);
      if (color_result != GHOSTTY_SUCCESS &&
          color_result != GHOSTTY_INVALID_VALUE) {
        result = color_result;
        goto error;
      }
      color_result = ghostty_render_state_row_cells_get(
          renderer->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
          &background);
      if (color_result != GHOSTTY_SUCCESS &&
          color_result != GHOSTTY_INVALID_VALUE) {
        result = color_result;
        goto error;
      }
      if (style.inverse) {
        GhosttyColorRgb swap = foreground;
        foreground = background;
        background = swap;
      }
      if (style.invisible) {
        foreground = background;
      }
      out_cell->foreground = to_mux_rgb(foreground);
      out_cell->background = to_mux_rgb(background);
      out_cell->underline_color = to_mux_rgb(resolve_style_color(
          style.underline_color, &colors, foreground));

      if (style.bold) out_cell->flags |= MUX_GHOSTTY_CELL_BOLD;
      if (style.italic) out_cell->flags |= MUX_GHOSTTY_CELL_ITALIC;
      if (style.faint) out_cell->flags |= MUX_GHOSTTY_CELL_FAINT;
      if (style.blink) out_cell->flags |= MUX_GHOSTTY_CELL_BLINK;
      if (style.inverse) out_cell->flags |= MUX_GHOSTTY_CELL_INVERSE;
      if (style.invisible) out_cell->flags |= MUX_GHOSTTY_CELL_INVISIBLE;
      if (style.strikethrough) out_cell->flags |= MUX_GHOSTTY_CELL_STRIKETHROUGH;
      if (style.overline) out_cell->flags |= MUX_GHOSTTY_CELL_OVERLINE;
      out_cell->underline = (uint8_t)style.underline;

      GhosttyCellWide width = GHOSTTY_CELL_WIDE_NARROW;
      GhosttyCellSemanticContent semantic = GHOSTTY_CELL_SEMANTIC_OUTPUT;
      bool hyperlink = false;
      result = ghostty_cell_get(raw_cell, GHOSTTY_CELL_DATA_WIDE, &width);
      if (result != GHOSTTY_SUCCESS) goto error;
      result = ghostty_cell_get(
          raw_cell, GHOSTTY_CELL_DATA_SEMANTIC_CONTENT, &semantic);
      if (result != GHOSTTY_SUCCESS) goto error;
      result = ghostty_cell_get(
          raw_cell, GHOSTTY_CELL_DATA_HAS_HYPERLINK, &hyperlink);
      if (result != GHOSTTY_SUCCESS) goto error;
      out_cell->width = (uint8_t)width;
      out_cell->semantic = (uint8_t)semantic;
      out_cell->hyperlink = hyperlink ? 1 : 0;

      bool selected = false;
      result = ghostty_render_state_row_cells_get(
          renderer->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_SELECTED,
          &selected);
      if (result != GHOSTTY_SUCCESS) goto error;
      out_cell->selected = selected ? 1 : 0;

      GhosttyBuffer text = {0};
      result = ghostty_render_state_row_cells_get(
          renderer->cells,
          GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
          &text);
      if (result == GHOSTTY_OUT_OF_SPACE) {
        const size_t required = frame.text_len + text.len;
        if (required > UINT32_MAX ||
            !ensure_text_capacity(
                &renderer->text, &renderer->text_capacity, required)) {
          result = GHOSTTY_OUT_OF_MEMORY;
          goto error;
        }
        frame.text = renderer->text;
        text.ptr = frame.text + frame.text_len;
        text.cap = renderer->text_capacity - frame.text_len;
        result = ghostty_render_state_row_cells_get(
            renderer->cells,
            GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
            &text);
      }
      if (result != GHOSTTY_SUCCESS) goto error;
      if (text.len > 0) {
        out_cell->text_offset = (uint32_t)frame.text_len;
        out_cell->text_len = (uint32_t)text.len;
        frame.text_len += text.len;
      }
      x++;
    }

    bool clean_row = false;
    result = ghostty_render_state_row_set(
        renderer->rows, GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY, &clean_row);
    if (result != GHOSTTY_SUCCESS) goto error;
    y++;
  }

  GhosttyRenderStateDirty clean = GHOSTTY_RENDER_STATE_DIRTY_FALSE;
  result = ghostty_render_state_set(
      renderer->state, GHOSTTY_RENDER_STATE_OPTION_DIRTY, &clean);
  if (result != GHOSTTY_SUCCESS) goto error;

  *out_frame = frame;
  return (int32_t)GHOSTTY_SUCCESS;

error:
  return (int32_t)result;
}
