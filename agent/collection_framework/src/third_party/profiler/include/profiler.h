#ifndef PROFILER_H
#define PROFILER_H

#include <stdarg.h>
#include <stdint.h>

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif
#if 0
}
#endif

#define PROFILER_SUCCESS                0
#define PROFILER_ERR_OTHER              -1
#define PROFILER_ERR_CHECK_STACK        -2

#define THREAD_START        0x1
#define THREAD_DLOPEN       0x2
#define THREAD_DLSYM        0x3
#define THREAD_ENTRY        0x4
#define THREAD_DLCLOSE      0x5
#define THREAD_END          0x6

#define THREAD_KICK         0x10

#define THREAD_CONTROL_MASK 0xf0
#define THREAD_STATE_MASK   0x0f

#define ACTION_DLOPEN       0x1
#define ACTION_ENTRY        0x2
#define ACTION_DLCLOSE      0x4

typedef struct profiler profiler_t;

int profiler_init(profiler_t ** profiler, pid_t pid, char *entry);

int profiler_attach(profiler_t * profiler);

int profiler_detach(profiler_t * profiler);

int profiler_inject(profiler_t * profiler, const char *path,
                    void **handle);

int profiler_uninject(profiler_t * profiler, void *handle);

int profiler_call(profiler_t * profiler, void *handle, const char *name);

int profiler_remote_func_addr(profiler_t * profiler, void *handle,
                              const char *name, size_t *func_addr_out);

int profiler_remote_call(profiler_t * profiler, intptr_t * retval,
                         size_t func_addr, ...);

int profiler_remote_vcall(profiler_t * profiler, intptr_t * retval,
                          size_t func_addr, va_list ap);

int profiler_inject_in_cloned_thread(profiler_t * profiler,
                                     const char *path, void **handle);

int profiler_continue(profiler_t * profiler);
int profiler_stop(profiler_t * profiler);
int profiler_wait(profiler_t * profiler, int state);
int profiler_loose(profiler_t * profiler);
int profiler_retach(profiler_t * profiler);
int profiler_notify(profiler_t *profiler, int state);
int profiler_set_timeout(profiler_t *profiler, int timeout);

extern const char* libprofiler_version;

#if 0
{
#endif
#ifdef __cplusplus
};                              /* extern "C" */
#endif

#endif
