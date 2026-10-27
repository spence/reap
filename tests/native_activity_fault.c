// native activity faults apply only to the explicitly selected owned test process
#include <errno.h>
#include <libproc.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

static int list_fault(uint32_t type, uint32_t info, void *buffer, int size) {
  const char *mode = getenv("REAP_NATIVE_ACTIVITY_FAULT");
  if (mode && !strcmp(mode, "capacity")) return size;
  if (mode && !strcmp(mode, "missing-caller")) {
    if (size < (int)sizeof(int)) { errno = EINVAL; return -1; }
    ((int *)buffer)[0] = 1;
    return sizeof(int);
  }
  return proc_listpids(type, info, buffer, size);
}

static int path_fault(int pid, void *buffer, uint32_t size) {
  const char *mode = getenv("REAP_NATIVE_ACTIVITY_FAULT");
  if (mode && !strcmp(mode, "denied")) { errno = EPERM; return 0; }
  const char *selected = getenv("REAP_NATIVE_ACTIVITY_PID");
  if (mode && !strcmp(mode, "missing-image") && selected && pid == atoi(selected)) {
    errno = ENOENT;
    return 0;
  }
  return proc_pidpath(pid, buffer, size);
}

#define INTERPOSE(replacement, original) \
  __attribute__((used, section("__DATA,__interpose"))) \
  static const struct { const void *new_function; const void *old_function; } \
  pair_##original = { (const void *)(uintptr_t)&replacement, (const void *)(uintptr_t)&original };

INTERPOSE(list_fault, proc_listpids)
INTERPOSE(path_fault, proc_pidpath)
