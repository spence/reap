// native test interposition; only owned I/O fixtures receive this library.
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void pause_gate(const char *gate) {
  char armed[PATH_MAX], entered[PATH_MAX], fifo[PATH_MAX];
  if (snprintf(armed, sizeof(armed), "%s/armed", gate) >= (int)sizeof(armed) ||
      snprintf(entered, sizeof(entered), "%s/entered", gate) >= (int)sizeof(entered) ||
      snprintf(fifo, sizeof(fifo), "%s/fifo", gate) >= (int)sizeof(fifo)) _exit(97);
  if (unlink(armed) != 0) return;
  int marker = open(entered, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
  if (marker < 0 || write(marker, "1", 1) != 1) _exit(97);
  close(marker);
  int reader = open(fifo, O_RDONLY | O_CLOEXEC);
  if (reader < 0) _exit(97);
  char byte;
  ssize_t count;
  do { count = read(reader, &byte, 1); } while (count < 0 && errno == EINTR);
  close(reader);
  if (count != 1) _exit(97);
}

static void pause_sync(int descriptor) {
  const char *target = getenv("REAP_SYNC_FAULT_TARGET");
  const char *gate = getenv("REAP_SYNC_FAULT_GATE");
  const char *mode = getenv("REAP_SYNC_FAULT_MODE");
  if (!target || !gate || !mode) return;
  char path[PATH_MAX];
  if (fcntl(descriptor, F_GETPATH, path) != 0) return;
  const size_t length = strlen(target);
  const int file = strcmp(mode, "file") == 0 && strncmp(path, target, length) == 0 &&
                   strcmp(path + length, "/transaction.pending") == 0;
  const int directory = strcmp(mode, "directory") == 0 && strcmp(path, target) == 0;
  if (!file && !directory) return;
  if (file) {
    // only job records activate this gate, not a writer's header claim during restart.
    int input = open(path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    if (input < 0) return;
    char document[128];
    ssize_t length = pread(input, document, sizeof(document) - 1, 40);
    close(input);
    if (length <= 0) return;
    document[length] = '\0';
    if (!strstr(document, "\"document\":\"Job\"")) return;
  }
  pause_gate(gate);
}

static int gated_openat(int descriptor, const char *name, int flags, ...) {
  if (flags & O_CREAT) {
    va_list arguments;
    va_start(arguments, flags);
    int mode = va_arg(arguments, int);
    va_end(arguments);
    return openat(descriptor, name, flags, mode);
  }
  const char *gate = getenv("REAP_CONTEXT_FAULT_GATE");
  const char *target = getenv("REAP_CONTEXT_FAULT_CONFIG_PARENT");
  char parent[PATH_MAX];
  if (gate && target && strcmp(name, "config.json") == 0 &&
      fcntl(descriptor, F_GETPATH, parent) == 0 && strcmp(parent, target) == 0)
    pause_gate(gate);
  return openat(descriptor, name, flags);
}

static int gated_fcntl(int descriptor, int command, ...) {
  if (command == F_FULLFSYNC) {
    pause_sync(descriptor);
    return fcntl(descriptor, command);
  }
  switch (command) {
    case F_GETFD: case F_GETFL: case F_GETOWN: case F_CHKCLEAN:
    case F_FREEZE_FS: case F_THAW_FS: case F_GETPROTECTIONCLASS:
    case F_GETNOSIGPIPE: case F_GETPROTECTIONLEVEL: case F_BARRIERFSYNC: case F_GETLEASE:
      return fcntl(descriptor, command);
    default: break;
  }
  va_list arguments;
  va_start(arguments, command);
  int result;
  switch (command) {
    case F_DUPFD: case F_DUPFD_CLOEXEC: case F_SETFD: case F_SETFL: case F_SETOWN:
    case F_RDAHEAD: case F_NOCACHE: case F_GLOBAL_NOCACHE: case F_NODIRECT:
    case F_SETPROTECTIONCLASS: case F_SETNOSIGPIPE: case F_SINGLE_WRITER:
    case F_SETBACKINGSTORE: case F_SETLEASE: case F_NOCACHE_EXT:
      result = fcntl(descriptor, command, va_arg(arguments, int));
      break;
    default:
      result = fcntl(descriptor, command, va_arg(arguments, void *));
      break;
  }
  va_end(arguments);
  return result;
}

__attribute__((used, section("__DATA,__interpose")))
static const struct { const void *replacement; const void *original; } interposition = {
  (const void *)gated_fcntl, (const void *)fcntl
};

__attribute__((used, section("__DATA,__interpose")))
static const struct { const void *replacement; const void *original; } context_interposition = {
  (const void *)gated_openat, (const void *)openat
};
