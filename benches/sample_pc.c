/** Sample executing native PCs using the Linux process CPU timer.
 * Provenance: original instrumentation using POSIX sigaction and setitimer.
 */
#define _GNU_SOURCE
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/time.h>
#include <ucontext.h>
#include <unistd.h>
#include <errno.h>
static int sample_fd;

static void sample(int unused, siginfo_t *info, void *raw) {
    int saved = errno;
    (void) unused;
    (void) info;
    ucontext_t *ctx = raw;
#if defined(__x86_64__)
    uintptr_t pc = ctx->uc_mcontext.gregs[REG_RIP];
#elif defined(__aarch64__)
    uintptr_t pc = ctx->uc_mcontext.pc;
#else
#error "Unsupported Linux signal frame"
#endif
    ssize_t count = write(sample_fd, &pc, sizeof(pc));
    (void) count;
    errno = saved;
}

__attribute__((constructor)) static void start(void) {
    const char *fd = getenv("SANSHO_SAMPLE_FD");
    if (!fd) return;
    sample_fd = atoi(fd);
    struct sigaction sa = { .sa_sigaction = sample, .sa_flags = SA_SIGINFO | SA_RESTART };
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGPROF,&sa,0)) return;
    struct itimerval timer = { {0,250}, {0,250} };
    setitimer(ITIMER_PROF,&timer,0);
}
