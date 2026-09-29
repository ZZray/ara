#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

/* Linux GNU test instrumentation; the inherited fd is the only log sink. */
#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#define HANDLE_LIMIT 64
#define LOG_CAPACITY (2 * PATH_MAX * 6 + 4096)

typedef DIR *(*opendir_fn)(const char *);
typedef struct dirent64 *(*readdir64_fn)(DIR *);
typedef int (*closedir_fn)(DIR *);
typedef int (*statx_fn)(int, const char *, int, unsigned int, struct statx *);
typedef int (*fstatat64_fn)(int, const char *, struct stat64 *, int);

enum fault_mode { MODE_OBSERVE, MODE_ENUMERATION, MODE_TYPE };

struct directory_handle {
    DIR *directory;
    int descriptor;
    uint64_t generation;
    uint64_t non_dot_count;
    uint64_t enumeration_injection_count;
    uint64_t metadata_injection_count;
    uint64_t dtype_unknown_count;
};

struct json_line {
    char data[LOG_CAPACITY];
    size_t used;
};

static opendir_fn g_opendir;
static readdir64_fn g_readdir64;
static closedir_fn g_closedir;
static statx_fn g_statx;
static fstatat64_fn g_fstatat64;
static enum fault_mode g_mode;
static char g_mode_name[16];
static char g_target_path[PATH_MAX];
static char g_arm_path[PATH_MAX];
static char g_entry_name[NAME_MAX + 1];
static int g_log_fd = -1;
static bool g_ready;
static uint64_t g_generation;
static struct directory_handle g_handles[HANDLE_LIMIT];
static pthread_mutex_t g_handle_mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_mutex_t g_log_mutex = PTHREAD_MUTEX_INITIALIZER;
static __thread unsigned int g_recursion_depth;

static void raw_diagnostic_write(int fd, const char *data, size_t length)
{
    while (length != 0) {
        long result = syscall(SYS_write, fd, data, length);
        if (result < 0 && errno == EINTR)
            continue;
        if (result <= 0)
            return;
        data += (size_t)result;
        length -= (size_t)result;
    }
}

/* This path must also work while resolving symbols or holding either mutex. */
static void fatal(const char *reason) __attribute__((noreturn));
static void fatal(const char *reason)
{
    const char prefix[] = "{\"event\":\"fatal\",\"reason\":\"";
    const char suffix[] = "\"}\n";
    if (g_log_fd >= 0) {
        raw_diagnostic_write(g_log_fd, prefix, sizeof(prefix) - 1);
        raw_diagnostic_write(g_log_fd, reason, strlen(reason));
        raw_diagnostic_write(g_log_fd, suffix, sizeof(suffix) - 1);
    }
    const char diagnostic[] = "ctx skill directory fault fatal: ";
    raw_diagnostic_write(STDERR_FILENO, diagnostic, sizeof(diagnostic) - 1);
    raw_diagnostic_write(STDERR_FILENO, reason, strlen(reason));
    raw_diagnostic_write(STDERR_FILENO, "\n", 1);
    syscall(SYS_exit_group, 120);
    _exit(120);
}

static void lock_mutex(pthread_mutex_t *mutex)
{
    if (pthread_mutex_lock(mutex) != 0)
        fatal("instrumentation mutex lock failed");
}

static void unlock_mutex(pthread_mutex_t *mutex)
{
    if (pthread_mutex_unlock(mutex) != 0)
        fatal("instrumentation mutex unlock failed");
}

static void append_bytes(struct json_line *line, const char *data, size_t size)
{
    if (size > sizeof(line->data) - line->used)
        fatal("instrumentation log buffer exhausted");
    memcpy(line->data + line->used, data, size);
    line->used += size;
}

static void append_literal(struct json_line *line, const char *text)
{
    append_bytes(line, text, strlen(text));
}

static void append_json_string(struct json_line *line, const char *text)
{
    static const char hex[] = "0123456789abcdef";
    append_literal(line, "\"");
    for (const unsigned char *cursor = (const unsigned char *)text;
         *cursor != '\0'; ++cursor) {
        if (*cursor == '"' || *cursor == '\\') {
            char escaped[2] = { '\\', (char)*cursor };
            append_bytes(line, escaped, sizeof(escaped));
        } else if (*cursor < 0x20 || *cursor >= 0x80) {
            /* Fixture names are ASCII; escaping bytes keeps every log valid JSON. */
            char escaped[6] = { '\\', 'u', '0', '0', hex[*cursor >> 4],
                                hex[*cursor & 0x0f] };
            append_bytes(line, escaped, sizeof(escaped));
        } else {
            append_bytes(line, (const char *)cursor, 1);
        }
    }
    append_literal(line, "\"");
}

static void string_field(struct json_line *line, const char *key, const char *value)
{
    append_literal(line, ",\"");
    append_literal(line, key);
    append_literal(line, "\":");
    append_json_string(line, value);
}

static void unsigned_field(struct json_line *line, const char *key, uint64_t value)
{
    char number[32];
    int length = snprintf(number, sizeof(number), "%llu", (unsigned long long)value);
    if (length < 0 || (size_t)length >= sizeof(number))
        fatal("instrumentation numeric formatting failed");
    append_literal(line, ",\"");
    append_literal(line, key);
    append_literal(line, "\":");
    append_bytes(line, number, (size_t)length);
}

static void signed_field(struct json_line *line, const char *key, int value)
{
    char number[32];
    int length = snprintf(number, sizeof(number), "%d", value);
    if (length < 0 || (size_t)length >= sizeof(number))
        fatal("instrumentation numeric formatting failed");
    append_literal(line, ",\"");
    append_literal(line, key);
    append_literal(line, "\":");
    append_bytes(line, number, (size_t)length);
}

static void begin_event(struct json_line *line, const char *event)
{
    line->used = 0;
    append_literal(line, "{\"event\":");
    append_json_string(line, event);
    string_field(line, "mode", g_mode_name);
}

static void finish_event(struct json_line *line)
{
    append_literal(line, "}\n");
    /* Lock order is handle -> log. No logger acquires the handle mutex. */
    lock_mutex(&g_log_mutex);
    size_t offset = 0;
    while (offset < line->used) {
        long result = syscall(SYS_write, g_log_fd, line->data + offset,
                              line->used - offset);
        if (result < 0 && errno == EINTR)
            continue;
        if (result <= 0)
            fatal("instrumentation log write failed");
        offset += (size_t)result;
    }
    unlock_mutex(&g_log_mutex);
}

static void log_handle(const char *event, const struct directory_handle *handle,
                       const char *symbol, const char *name, int actual_type,
                       int returned_type, int error_number)
{
    struct json_line line;
    begin_event(&line, event);
    string_field(&line, "path", g_target_path);
    append_literal(&line, ",\"armed\":true,\"target\":true");
    unsigned_field(&line, "generation", handle->generation);
    signed_field(&line, "dirfd", handle->descriptor);
    unsigned_field(&line, "non_dot_count", handle->non_dot_count);
    unsigned_field(&line, "injection_count", handle->enumeration_injection_count);
    unsigned_field(&line, "enumeration_injection_count", handle->enumeration_injection_count);
    unsigned_field(&line, "metadata_injection_count", handle->metadata_injection_count);
    unsigned_field(&line, "dtype_unknown_count", handle->dtype_unknown_count);
    if (symbol != NULL)
        string_field(&line, "symbol", symbol);
    if (name != NULL)
        string_field(&line, "name", name);
    if (actual_type >= 0)
        signed_field(&line, "actual_entry_type", actual_type);
    if (returned_type >= 0)
        signed_field(&line, "entry_type", returned_type);
    signed_field(&line, "errno", error_number);
    finish_event(&line);
}

static void log_scope_mismatch(const char *symbol, const char *path, int descriptor,
                               const char *name, const char *reason,
                               int error_number)
{
    struct json_line line;
    begin_event(&line, "scope_mismatch");
    string_field(&line, "symbol", symbol);
    string_field(&line, "reason", reason);
    append_literal(&line, ",\"forwarded\":true");
    if (path != NULL)
        string_field(&line, "path", path);
    if (name != NULL)
        string_field(&line, "name", name);
    signed_field(&line, "dirfd", descriptor);
    signed_field(&line, "errno", error_number);
    finish_event(&line);
}

static bool is_armed(void)
{
    int saved_errno = errno;
    long descriptor;
    do {
        descriptor = syscall(SYS_openat, AT_FDCWD, g_arm_path,
                             O_RDONLY | O_CLOEXEC | O_NONBLOCK, 0);
    } while (descriptor < 0 && errno == EINTR);
    if (descriptor < 0) {
        if (errno != ENOENT)
            fatal("cannot inspect arm file");
        errno = saved_errno;
        return false;
    }
    if (syscall(SYS_close, descriptor) != 0)
        fatal("cannot close arm-file probe");
    errno = saved_errno;
    return true;
}

static void copy_config(char *destination, size_t capacity, const char *key)
{
    const char *value = getenv(key);
    if (value == NULL || *value == '\0' || strlen(value) >= capacity)
        fatal("missing or oversized instrumentation configuration");
    memcpy(destination, value, strlen(value) + 1);
}

static void *resolve_symbol(const char *symbol)
{
    dlerror();
    void *address = dlsym(RTLD_NEXT, symbol);
    const char *error = dlerror();
    if (address == NULL || error != NULL)
        fatal("required GNU symbol could not be resolved");
    struct json_line line;
    char address_text[2 + sizeof(void *) * 2 + 1];
    int length = snprintf(address_text, sizeof(address_text), "%p", address);
    if (length < 0 || (size_t)length >= sizeof(address_text))
        fatal("instrumentation address formatting failed");
    begin_event(&line, "resolved");
    string_field(&line, "symbol", symbol);
    string_field(&line, "address", address_text);
    finish_event(&line);
    return address;
}

static void initialize(void) __attribute__((constructor));
static void initialize(void)
{
    int saved_errno = errno;
    ++g_recursion_depth;
    const char *fd_text = getenv("ARA_CTX_FAULT_FD");
    if (fd_text == NULL || *fd_text == '\0')
        fatal("missing inherited instrumentation log fd");
    unsigned int descriptor = 0;
    for (const unsigned char *cursor = (const unsigned char *)fd_text;
         *cursor != '\0'; ++cursor) {
        if (*cursor < '0' || *cursor > '9' ||
            descriptor > ((unsigned int)INT_MAX - (*cursor - '0')) / 10)
            fatal("invalid inherited instrumentation log fd");
        descriptor = descriptor * 10 + (*cursor - '0');
    }
    g_log_fd = (int)descriptor;
    long fd_flags = syscall(SYS_fcntl, g_log_fd, F_GETFL, 0);
    if (fd_flags < 0 || (fd_flags & O_ACCMODE) == O_RDONLY)
        fatal("inherited instrumentation log fd is not writable");
    copy_config(g_target_path, sizeof(g_target_path), "ARA_CTX_FAULT_PATH");
    copy_config(g_arm_path, sizeof(g_arm_path), "ARA_CTX_FAULT_ARM");
    copy_config(g_mode_name, sizeof(g_mode_name), "ARA_CTX_FAULT_MODE");
    copy_config(g_entry_name, sizeof(g_entry_name), "ARA_CTX_FAULT_NAME");
    if (g_target_path[0] != '/' || g_arm_path[0] != '/' ||
        strcmp(g_target_path, g_arm_path) == 0 || strcmp(g_target_path, "/") == 0)
        fatal("target and arm must be distinct absolute dedicated paths");
    if (strcmp(g_entry_name, "probe-type.txt") != 0)
        fatal("fault entry name must be probe-type.txt");
    if (strcmp(g_mode_name, "observe") == 0)
        g_mode = MODE_OBSERVE;
    else if (strcmp(g_mode_name, "enumeration") == 0)
        g_mode = MODE_ENUMERATION;
    else if (strcmp(g_mode_name, "type") == 0)
        g_mode = MODE_TYPE;
    else
        fatal("unknown instrumentation mode");

    struct json_line line;
    begin_event(&line, "init");
    string_field(&line, "path", g_target_path);
    string_field(&line, "arm", g_arm_path);
    string_field(&line, "name", g_entry_name);
    signed_field(&line, "log_fd", g_log_fd);
    append_literal(&line, is_armed() ? ",\"initial_armed\":true" :
                                      ",\"initial_armed\":false");
    finish_event(&line);
    g_opendir = (opendir_fn)resolve_symbol("opendir");
    g_readdir64 = (readdir64_fn)resolve_symbol("readdir64");
    g_closedir = (closedir_fn)resolve_symbol("closedir");
    g_statx = (statx_fn)resolve_symbol("statx");
    g_fstatat64 = (fstatat64_fn)resolve_symbol("fstatat64");
    g_ready = true;
    --g_recursion_depth;
    errno = saved_errno;
}

static void require_ready(void)
{
    if (!g_ready)
        fatal("hook called before instrumentation initialization completed");
}

static struct directory_handle *find_directory(DIR *directory)
{
    for (size_t index = 0; index < HANDLE_LIMIT; ++index) {
        if (g_handles[index].directory == directory)
            return &g_handles[index];
    }
    return NULL;
}

static struct directory_handle *find_descriptor(int descriptor)
{
    for (size_t index = 0; index < HANDLE_LIMIT; ++index) {
        if (g_handles[index].directory != NULL &&
            g_handles[index].descriptor == descriptor)
            return &g_handles[index];
    }
    return NULL;
}

DIR *opendir(const char *path)
{
    if (g_recursion_depth != 0) {
        if (g_opendir == NULL)
            fatal("recursive opendir before symbol resolution");
        return g_opendir(path);
    }
    require_ready();
    ++g_recursion_depth;
    int incoming_errno = errno;
    bool target = strcmp(path, g_target_path) == 0;
    bool armed = target && is_armed();
    errno = incoming_errno;
    DIR *directory = g_opendir(path);
    int saved_errno = errno;
    if (target && armed && directory != NULL) {
        int descriptor = dirfd(directory);
        if (descriptor < 0)
            fatal("successful target opendir has no dirfd");
        lock_mutex(&g_handle_mutex);
        if (find_directory(directory) != NULL || find_descriptor(descriptor) != NULL)
            fatal("target directory registration collides with a live handle");
        struct directory_handle *slot = NULL;
        for (size_t index = 0; index < HANDLE_LIMIT; ++index) {
            if (g_handles[index].directory == NULL) {
                slot = &g_handles[index];
                break;
            }
        }
        if (slot == NULL || g_generation == UINT64_MAX)
            fatal("target directory registration table exhausted");
        *slot = (struct directory_handle){ .directory = directory,
            .descriptor = descriptor, .generation = ++g_generation };
        log_handle("opendir", slot, "opendir", NULL, -1, -1, saved_errno);
        unlock_mutex(&g_handle_mutex);
    } else if (target) {
        log_scope_mismatch("opendir", path, -1, NULL,
                           armed ? "target_open_failed" : "pre_arm",
                           saved_errno);
    }
    --g_recursion_depth;
    errno = saved_errno;
    return directory;
}

struct dirent64 *readdir64(DIR *directory)
{
    if (g_recursion_depth != 0) {
        if (g_readdir64 == NULL)
            fatal("recursive readdir64 before symbol resolution");
        return g_readdir64(directory);
    }
    require_ready();
    ++g_recursion_depth;
    int incoming_errno = errno;
    lock_mutex(&g_handle_mutex);
    struct directory_handle *handle = find_directory(directory);
    if (handle == NULL) {
        unlock_mutex(&g_handle_mutex);
        errno = incoming_errno;
        struct dirent64 *entry = g_readdir64(directory);
        int saved_errno = errno;
        --g_recursion_depth;
        errno = saved_errno;
        return entry;
    }
    if (g_mode == MODE_ENUMERATION && handle->non_dot_count != 0) {
        if (handle->enumeration_injection_count == UINT64_MAX)
            fatal("enumeration injection counter exhausted");
        ++handle->enumeration_injection_count;
        log_handle("enumeration_eio", handle, "readdir64", NULL, -1, -1, EIO);
        unlock_mutex(&g_handle_mutex);
        --g_recursion_depth;
        errno = EIO;
        return NULL;
    }
    errno = incoming_errno;
    struct dirent64 *entry = g_readdir64(directory);
    int saved_errno = errno;
    if (entry == NULL) {
        log_handle(saved_errno == 0 ? "eof" : "readdir_error", handle,
                   "readdir64", NULL, -1, -1, saved_errno);
    } else {
        int actual_type = entry->d_type;
        bool dot = strcmp(entry->d_name, ".") == 0 ||
                   strcmp(entry->d_name, "..") == 0;
        if (!dot) {
            if (handle->non_dot_count == UINT64_MAX)
                fatal("real entry counter exhausted");
            ++handle->non_dot_count;
            if (g_mode == MODE_TYPE && strcmp(entry->d_name, g_entry_name) == 0) {
                if (handle->dtype_unknown_count == UINT64_MAX)
                    fatal("unknown dtype counter exhausted");
                ++handle->dtype_unknown_count;
                /* d_reclen is variable: never copy the whole dirent64 object. */
                entry->d_type = DT_UNKNOWN;
                log_handle("dtype_unknown", handle, "readdir64", entry->d_name,
                           actual_type, entry->d_type, saved_errno);
            }
        }
        log_handle(dot ? "dot_entry" : "entry", handle, "readdir64", entry->d_name,
                   actual_type, entry->d_type, saved_errno);
    }
    unlock_mutex(&g_handle_mutex);
    --g_recursion_depth;
    errno = saved_errno;
    return entry;
}

int closedir(DIR *directory)
{
    if (g_recursion_depth != 0) {
        if (g_closedir == NULL)
            fatal("recursive closedir before symbol resolution");
        return g_closedir(directory);
    }
    require_ready();
    ++g_recursion_depth;
    int incoming_errno = errno;
    lock_mutex(&g_handle_mutex);
    struct directory_handle *handle = find_directory(directory);
    if (handle == NULL) {
        unlock_mutex(&g_handle_mutex);
        errno = incoming_errno;
        int result = g_closedir(directory);
        int saved_errno = errno;
        --g_recursion_depth;
        errno = saved_errno;
        return result;
    }
    struct directory_handle snapshot = *handle;
    errno = incoming_errno;
    int result = g_closedir(directory);
    int saved_errno = errno;
    /* Clear before another successful open can register a reused pointer/fd. */
    memset(handle, 0, sizeof(*handle));
    log_handle("closedir", &snapshot, "closedir", NULL, -1, -1, saved_errno);
    unlock_mutex(&g_handle_mutex);
    --g_recursion_depth;
    errno = saved_errno;
    return result;
}

static bool inject_metadata(const char *symbol, int descriptor, const char *name)
{
    /* Rust's statx availability probe uses a null path; it must pass through. */
    if (name == NULL || strcmp(name, g_entry_name) != 0)
        return false;
    int incoming_errno = errno;
    lock_mutex(&g_handle_mutex);
    struct directory_handle *handle = find_descriptor(descriptor);
    if (handle != NULL && g_mode == MODE_TYPE && handle->dtype_unknown_count != 0) {
        if (handle->metadata_injection_count == UINT64_MAX)
            fatal("metadata injection counter exhausted");
        ++handle->metadata_injection_count;
        log_handle("metadata_eio", handle, symbol, name, -1, -1, EIO);
        unlock_mutex(&g_handle_mutex);
        return true;
    }
    bool registered = handle != NULL;
    unlock_mutex(&g_handle_mutex);
    if (!registered)
        log_scope_mismatch(symbol, NULL, descriptor, name, "unregistered_dirfd",
                           incoming_errno);
    errno = incoming_errno;
    return false;
}

/*
 * Keep the exported GNU symbols without inheriting glibc's nonnull attributes.
 * Rust intentionally calls statx with a null path to probe its availability;
 * otherwise -O2 could discard inject_metadata's null-path guard after inlining.
 */
int ctx_hook_statx(int descriptor, const char *name, int flags, unsigned int mask,
                  struct statx *status) __asm__("statx");
int ctx_hook_statx(int descriptor, const char *name, int flags, unsigned int mask,
                  struct statx *status)
{
    if (g_recursion_depth != 0) {
        if (g_statx == NULL)
            fatal("recursive statx before symbol resolution");
        return g_statx(descriptor, name, flags, mask, status);
    }
    require_ready();
    ++g_recursion_depth;
    if (inject_metadata("statx", descriptor, name)) {
        --g_recursion_depth;
        errno = EIO;
        return -1;
    }
    int result = g_statx(descriptor, name, flags, mask, status);
    int saved_errno = errno;
    --g_recursion_depth;
    errno = saved_errno;
    return result;
}

int ctx_hook_fstatat64(int descriptor, const char *name, struct stat64 *status,
                      int flags) __asm__("fstatat64");
int ctx_hook_fstatat64(int descriptor, const char *name, struct stat64 *status,
                      int flags)
{
    if (g_recursion_depth != 0) {
        if (g_fstatat64 == NULL)
            fatal("recursive fstatat64 before symbol resolution");
        return g_fstatat64(descriptor, name, status, flags);
    }
    require_ready();
    ++g_recursion_depth;
    if (inject_metadata("fstatat64", descriptor, name)) {
        --g_recursion_depth;
        errno = EIO;
        return -1;
    }
    int result = g_fstatat64(descriptor, name, status, flags);
    int saved_errno = errno;
    --g_recursion_depth;
    errno = saved_errno;
    return result;
}
