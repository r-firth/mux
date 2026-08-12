#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <ghostty/vt.h>

int main(void) {
  GhosttyTerminal terminal = NULL;
  GhosttyResult result = ghostty_terminal_new(NULL, &terminal, 80, 24);
  assert(result == GHOSTTY_SUCCESS);

  const char *text = "\033[1;32mlibghostty-vt\033[0m\r\n";
  ghostty_terminal_vt_write(terminal, (const uint8_t *)text, strlen(text));
  result = ghostty_terminal_resize(terminal, 120, 40, 8, 16);
  assert(result == GHOSTTY_SUCCESS);

  size_t snapshot_len = 0;
  result = ghostty_snapshot_encode_buf(terminal, NULL, 0, &snapshot_len);
  assert(result == GHOSTTY_OUT_OF_SPACE);
  assert(snapshot_len > 0);

  uint8_t *snapshot = malloc(snapshot_len);
  assert(snapshot != NULL);
  size_t written = 0;
  result = ghostty_snapshot_encode_buf(
      terminal, snapshot, snapshot_len, &written);
  assert(result == GHOSTTY_SUCCESS);
  assert(written == snapshot_len);

  printf("libghostty-vt snapshot: %zu bytes\n", snapshot_len);
  free(snapshot);
  ghostty_terminal_free(terminal);
  return 0;
}

