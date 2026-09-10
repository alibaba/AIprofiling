/*
 * probe.c — direct libprofiler.a stress driver.
 *
 * Bypasses pyki, CollectionFramework, Rust, tokio. Talks to libprofiler.a
 * with the same call sequence that InjectHandle::attach/detach uses, so a
 * repro here is an injector-level bug. If probe passes but CF fails, the
 * problem is above the C library.
 *
 * Build: see Makefile in the same directory.
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "profiler.h"

static int opt_pid = 0;
static int opt_iterations = 100;
static int opt_sleep_ms = 50;
static int opt_timeout = 30;         /* 0 = skip profiler_set_timeout */
static int opt_detach_on_fail = 0;   /* reproduce double-free */
static int opt_verbose = 0;

static volatile sig_atomic_t g_stop = 0;
static void on_sig(int _) { (void)_; g_stop = 1; }

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000.0 + ts.tv_nsec / 1e6;
}

static int cmp_double(const void *a, const void *b) {
    double da = *(const double *)a, db = *(const double *)b;
    return (da > db) - (da < db);
}

static void usage(const char *argv0) {
    fprintf(stderr,
        "usage: %s --pid PID [options]\n"
        "  --iterations N      number of attach cycles (default 100)\n"
        "  --sleep-ms N        delay between cycles (default 50)\n"
        "  --timeout SEC       call profiler_set_timeout(SEC) before\n"
        "                      attach; 0 = do not call (reproduces the\n"
        "                      pre-fix wait4-hangs-forever behaviour)\n"
        "                      (default 30)\n"
        "  --detach-on-fail    on attach failure, also call\n"
        "                      profiler_detach — reproduces the\n"
        "                      tcache double-free crash\n"
        "  --verbose           per-iteration log\n",
        argv0);
}

static int parse_args(int argc, char **argv) {
    for (int i = 1; i < argc; ++i) {
        const char *a = argv[i];
        if (!strcmp(a, "--pid") && i + 1 < argc)         opt_pid = atoi(argv[++i]);
        else if (!strcmp(a, "--iterations") && i + 1 < argc) opt_iterations = atoi(argv[++i]);
        else if (!strcmp(a, "--sleep-ms") && i + 1 < argc)   opt_sleep_ms   = atoi(argv[++i]);
        else if (!strcmp(a, "--timeout") && i + 1 < argc)    opt_timeout    = atoi(argv[++i]);
        else if (!strcmp(a, "--detach-on-fail"))             opt_detach_on_fail = 1;
        else if (!strcmp(a, "--verbose"))                    opt_verbose = 1;
        else if (!strcmp(a, "-h") || !strcmp(a, "--help"))   { usage(argv[0]); exit(0); }
        else { fprintf(stderr, "unknown arg: %s\n", a); usage(argv[0]); return -1; }
    }
    if (opt_pid <= 0) { usage(argv[0]); return -1; }
    return 0;
}

int main(int argc, char **argv) {
    if (parse_args(argc, argv) != 0) return 2;
    signal(SIGINT, on_sig);
    signal(SIGTERM, on_sig);

    printf("probe: pid=%d iters=%d timeout=%d detach_on_fail=%d\n",
           opt_pid, opt_iterations, opt_timeout, opt_detach_on_fail);
    fflush(stdout);

    int n_ok = 0, n_m2 = 0, n_m1 = 0, n_hung = 0;
    double *samples = calloc(opt_iterations, sizeof(double));
    int n_samples = 0;

    /* A hung attach with timeout=0 pins the probe; the caller is expected
     * to Ctrl-C. Keep going otherwise. */
    double watchdog_ms = opt_timeout > 0 ? (opt_timeout + 5) * 1000.0
                                         : 120000.0;

    for (int it = 0; it < opt_iterations && !g_stop; ++it) {
        profiler_t *p = NULL;
        char entry[] = "entry";
        int r = profiler_init(&p, opt_pid, entry);
        if (r != 0 || !p) {
            fprintf(stderr, "iter %d: profiler_init failed r=%d\n", it, r);
            n_m1++;
            usleep(opt_sleep_ms * 1000);
            continue;
        }

        if (opt_timeout > 0) {
            profiler_set_timeout(p, opt_timeout);
        }

        double t0 = now_ms();
        int ar = profiler_attach(p);
        double t1 = now_ms();
        double dt = t1 - t0;
        samples[n_samples++] = dt;

        if (opt_verbose) {
            printf("iter %d: attach=%d dt=%.1fms\n", it, ar, dt);
            fflush(stdout);
        }

        if (dt >= watchdog_ms) n_hung++;

        if (ar == 0) {
            n_ok++;
            profiler_detach(p);
        } else if (ar == -2) {
            n_m2++;
            if (opt_detach_on_fail) {
                /* This is the bug: libprofiler.a already freed its ptrace
                 * state when it returned -2. Any extra detach may crash. */
                profiler_detach(p);
            }
        } else {
            n_m1++;
            if (opt_detach_on_fail) {
                profiler_detach(p);
            }
        }

        /* profiler_t is opaque; libprofiler.a owns freeing it on both
         * success paths (detach after ok) and error paths (returns -2/-1).
         * Nothing to free here from the caller side.
         */

        if (opt_sleep_ms > 0) usleep(opt_sleep_ms * 1000);
    }

    qsort(samples, n_samples, sizeof(double), cmp_double);
    double avg = 0.0;
    for (int i = 0; i < n_samples; ++i) avg += samples[i];
    if (n_samples) avg /= n_samples;
    double p95 = n_samples ? samples[(int)(n_samples * 0.95)] : 0.0;

    printf("probe: pid=%d iters=%d histogram:\n"
           "  ok                : %d\n"
           "  err_check_stack -2: %d\n"
           "  err_other       -1: %d\n"
           "  hung (>watchdog)  : %d\n"
           "  avg_attach_ms     : %.2f\n"
           "  p95_attach_ms     : %.2f\n",
           opt_pid, opt_iterations, n_ok, n_m2, n_m1, n_hung, avg, p95);

    free(samples);
    return 0;
}
