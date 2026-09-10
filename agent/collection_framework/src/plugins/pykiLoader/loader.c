#define _GNU_SOURCE
#define PY_SSIZE_T_CLEAN
#include <Python.h>

#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

// libprofiler's clone-thread injection lands us here with no Python
// thread state. PyGILState_Ensure() blocks unconditionally until the
// interpreter hands over the GIL — and when the target's main thread
// is a tight CUDA / bf16 tensorop loop that only yields the GIL every
// few milliseconds AND the target does long uninterruptible ioctls,
// the wait can outlive CF's 60 s profiler_wait budget. Rather than
// hanging silently until CF times out, we run under a watchdog: if
// we don't reach the "GIL acquired" checkpoint within N seconds we
// tgkill(SIGUSR2) ourselves and the signal handler exits just this
// thread, so profiler_wait(THREAD_END) returns immediately with a
// distinct exit code instead of the target's 60 s black hole.
//
// Every state transition is timestamped to /tmp/aiprof_loader_<tgid>_<tid>.log
// so post-mortem diagnosis can tell "stuck waiting for GIL" apart from
// "stuck in dlopen" or "PyRun_SimpleString threw".

#define LOADER_EXIT_OK              0
#define LOADER_EXIT_READ_FAILED     41
#define LOADER_EXIT_GIL_TIMEOUT     42
#define LOADER_EXIT_PYRUN_FAILED    43

static const char *LOADER_LOG_PREFIX = "/tmp/aiprof_loader";

static pid_t g_loader_tid = 0;
static atomic_int g_gil_acquired = 0;
static atomic_int g_watchdog_fired = 0;

static void loader_log(const char *stage, const char *note) {
    char path[128];
    snprintf(path, sizeof(path), "%s_%d_%d.log",
             LOADER_LOG_PREFIX, (int)getpid(), (int)g_loader_tid);
    FILE *f = fopen(path, "a");
    if (!f) return;
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    struct tm tm;
    localtime_r(&ts.tv_sec, &tm);
    char stamp[32];
    strftime(stamp, sizeof(stamp), "%Y-%m-%d %H:%M:%S", &tm);
    fprintf(f, "[%s.%03ld pid=%d tid=%d] %s%s%s\n",
            stamp, ts.tv_nsec / 1000000, (int)getpid(), (int)g_loader_tid,
            stage, note ? " " : "", note ? note : "");
    fclose(f);
}

static void sigusr2_handler(int sig) {
    (void)sig;
    // Called on the loader thread by the watchdog. The loader thread has
    // never been registered with Python (PyGILState_Ensure never returned),
    // so there is nothing for the interpreter to clean up. Raw SYS_exit
    // terminates just this thread and leaves the target process untouched.
    atomic_store(&g_watchdog_fired, 1);
    // Best-effort last log entry; the fopen might miss the fsync but the
    // watchdog thread will also have logged its trigger reason.
    loader_log("SIGUSR2 handler -> SYS_exit", NULL);
    syscall(SYS_exit, LOADER_EXIT_GIL_TIMEOUT);
    _exit(LOADER_EXIT_GIL_TIMEOUT); // unreachable, silence any noreturn warning
}

static void *watchdog_fn(void *arg) {
    long timeout_sec = (long)(intptr_t)arg;
    pid_t tgid = getpid();
    pid_t target_tid = g_loader_tid;

    struct timespec ts;
    ts.tv_sec = timeout_sec;
    ts.tv_nsec = 0;
    nanosleep(&ts, NULL);

    if (atomic_load(&g_gil_acquired)) {
        return NULL;
    }

    char buf[96];
    snprintf(buf, sizeof(buf),
             "GIL not acquired after %lds; sending SIGUSR2 to loader tid=%d",
             timeout_sec, (int)target_tid);
    loader_log("watchdog fire", buf);

    // tgkill delivers to a specific thread; a plain kill(tgid, SIGUSR2)
    // could be dispatched to any thread in the target and fatally exit
    // the wrong one.
    syscall(SYS_tgkill, tgid, target_tid, SIGUSR2);
    return NULL;
}

static long parse_watchdog_sec(void) {
    const char *env = getenv("AIPROF_PYKI_GIL_WATCHDOG_SEC");
    if (!env || !*env) return 30;
    char *end = NULL;
    long v = strtol(env, &end, 10);
    if (end == env || v <= 0 || v > 600) return 30;
    return v;
}

char* read_file(const char* filename) {
    FILE* fp = fopen(filename, "r");
    if (!fp) {
        fprintf(stderr, "Error: Cannot open config file %s\n", filename);
        return NULL;
    }

    fseek(fp, 0, SEEK_END);
    long filesize = ftell(fp);
    fseek(fp, 0, SEEK_SET);

    if (filesize <= 0) {
        fprintf(stderr, "Error: Config file %s is empty or invalid\n", filename);
        fclose(fp);
        return NULL;
    }

    char* buffer = (char*)malloc(filesize + 1);
    if (!buffer) {
        fprintf(stderr, "Error: Out of memory\n");
        fclose(fp);
        return NULL;
    }

    size_t read_size = fread(buffer, 1, filesize, fp);
    fclose(fp);

    if (read_size != (size_t)filesize) {
        fprintf(stderr, "Error: Failed to read entire config file\n");
        free(buffer);
        return NULL;
    }

    buffer[filesize] = '\0';

    char* start = buffer;
    while (*start == ' ' || *start == '\t' || *start == '\n' || *start == '\r') start++;
    char* end = start + strlen(start) - 1;
    while (end > start && (*end == ' ' || *end == '\t' || *end == '\n' || *end == '\r')) end--;
    *(end + 1) = '\0';

    if (*start == '\0') {
        fprintf(stderr, "Error: Config file %s contains only whitespace\n", filename);
        free(buffer);
        return NULL;
    }

    size_t len = strlen(start);
    char* result = (char*)malloc(len + 1);
    if (result) {
        strcpy(result, start);
    } else {
        result = start;
    }

    if (result != start) {
        free(buffer);
    }

    return result;
}

void load_pyki(void* ignored) {
    (void)ignored;

    g_loader_tid = (pid_t)syscall(SYS_gettid);
    atomic_store(&g_gil_acquired, 0);
    atomic_store(&g_watchdog_fired, 0);

    loader_log("enter load_pyki", NULL);

    pid_t pid = getpid();
    char filepath[64];
    snprintf(filepath, sizeof(filepath), "/tmp/cf_loader_%d.txt", pid);

    char* py_command = read_file(filepath);
    if (!py_command) {
        loader_log("read_file failed", filepath);
        printf("Failed to load pyki module: cannot read cf_loader_xxx.txt\n");
        syscall(SYS_exit, LOADER_EXIT_READ_FAILED);
        return;
    }

    long watchdog_sec = parse_watchdog_sec();
    char note[64];
    snprintf(note, sizeof(note), "budget=%lds", watchdog_sec);
    loader_log("start GIL wait", note);

    // SIGUSR2 handler must be installed before the watchdog can fire it.
    // sigaction is a well-defined thing to call outside the interpreter
    // — it does not require a Python thread state.
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = sigusr2_handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;
    if (sigaction(SIGUSR2, &sa, NULL) != 0) {
        loader_log("sigaction failed", strerror(errno));
        // Continue without watchdog rather than aborting: if we can't
        // install the handler, the worst case is the pre-existing 60 s
        // hang, which is no regression.
    }

    pthread_t wd;
    int wd_started = pthread_create(&wd, NULL, watchdog_fn,
                                    (void *)(intptr_t)watchdog_sec) == 0;
    if (wd_started) {
        pthread_detach(wd);
    } else {
        loader_log("pthread_create watchdog failed", strerror(errno));
    }

    PyGILState_STATE gstate = PyGILState_Ensure();
    atomic_store(&g_gil_acquired, 1);
    loader_log("GIL acquired", NULL);

    int result = PyRun_SimpleString(py_command);
    if (result != 0) {
        loader_log("PyRun_SimpleString failed", filepath);
        fprintf(stderr, "Failed to execute Python code from %s\n", filepath);
        PyErr_Print();
    } else {
        loader_log("PyRun_SimpleString ok", NULL);
        printf("Success to load pyki module from cf_loader\n");
    }

    PyGILState_Release(gstate);
    loader_log("GIL released, load_pyki return", NULL);

    free(py_command);
    // Fall through: the injected clone thread returns and libprofiler
    // exits it. If PyRun_SimpleString failed, we still return normally
    // so libprofiler's clone-thread wrapper can clean up its stack — the
    // failure is logged and shows up in cf_loader log.
    (void)result;
}
