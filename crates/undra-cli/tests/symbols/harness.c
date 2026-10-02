/*
 * A C host for a built Undra core, for the symbol and debugger tests (tests/symbols.rs,
 * tests/debugging.rs; ADR-046). It loads the core through its C ABI table, registers a
 * `Diagnostics` port callback, makes the playground core panic, and prints what the core reports:
 *
 *     BASE 0x<load address of the image that holds the core>
 *     REPORT operation=<op> thread=<thread> namespace=<ns> version=<v> schema=0x<hash> image=<id>
 *     LOCATION <file:line:column>
 *     MESSAGE <message>
 *     FRAME 0x<address>              one per frame, innermost first (release builds: addresses only)
 *     FRAMESYM <symbol> <file> <line> a debug build's frame that names itself
 *     REPLY status=<n>                the call that panicked (2 = panicked)
 *     DONE reports=<n>
 *
 * usage: [UNDRA_HARNESS_WAIT=1] harness explode | detached | add
 *   UNDRA_HARNESS_WAIT stops the process with SIGSTOP at the start, for a debugger to attach to
 *   explode   the free function `explode("kaboom")`, a synchronous call (a panic in a call)
 *   detached  `explode_detached("task")`, which panics in a task of its own (operation "task")
 *   add       `add(40, 2)`, which does not panic: a method to put a breakpoint in
 *
 * Built with -DUNDRA_NS=<namespace> and `-I` the directory of undra.h; the core's library (or its
 * prelinked static archive) is linked in. The wire layouts are SPEC 3.1 and contract-tests/README:
 * PanicReport = message, location, operation, thread, frames, namespace, core_version,
 * schema_hash, image_id.
 */
#include "undra.h"

#include <dlfcn.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define CAT_(a, b) a##b
#define CAT(a, b) CAT_(a, b)
/* The core's one export (C ABI v2, ADR-044). */
extern const void *CAT(UNDRA_NS, _undra_api)(void);

#define PORT_DIAGNOSTICS 0xab68cd7cu /* fnv1a32("port.Diagnostics") */
#define DIAGNOSTICS_PANICKED 0xbd147e2eu /* fnv1a32("Diagnostics.panicked") */

static pthread_mutex_t out_lock = PTHREAD_MUTEX_INITIALIZER;
static volatile int reports = 0;
static volatile int replies = 0;
static volatile int last_status = -1;

/* A cursor over the wire bytes of the report: every read is checked, a short buffer sets `bad`. */
typedef struct {
    const uint8_t *p;
    uint32_t n, at;
    int bad;
} Cur;

static uint32_t rd_u32(Cur *c) {
    uint32_t v = 0;
    if (c->at + 4 > c->n) { c->bad = 1; return 0; }
    memcpy(&v, c->p + c->at, 4); /* little-endian hosts only: every platform Undra runs on */
    c->at += 4;
    return v;
}
static uint64_t rd_u64(Cur *c) {
    uint64_t v = 0;
    if (c->at + 8 > c->n) { c->bad = 1; return 0; }
    memcpy(&v, c->p + c->at, 8);
    c->at += 8;
    return v;
}
static uint8_t rd_u8(Cur *c) {
    if (c->at + 1 > c->n) { c->bad = 1; return 0; }
    return c->p[c->at++];
}
/* A String (u32 length, UTF-8) copied into `out` with newlines escaped, so a line stays a line. */
static void rd_str(Cur *c, char *out, size_t cap) {
    uint32_t len = rd_u32(c);
    size_t k = 0;
    if (c->bad || c->at + len > c->n) { c->bad = 1; out[0] = 0; return; }
    for (uint32_t i = 0; i < len && k + 3 < cap; i++) {
        uint8_t b = c->p[c->at + i];
        if (b == '\n') { out[k++] = '\\'; out[k++] = 'n'; }
        else out[k++] = (char)b;
    }
    out[k] = 0;
    c->at += len;
}

static void print_report(const uint8_t *args, uint32_t n) {
    Cur c = {args, n, 0, 0};
    char message[1024], location[512], operation[128], thread[128], ns[128], version[128], image[256];
    rd_str(&c, message, sizeof message);
    rd_str(&c, location, sizeof location);
    rd_str(&c, operation, sizeof operation);
    rd_str(&c, thread, sizeof thread);
    uint32_t frames = rd_u32(&c);
    /* Frames are printed after the fixed fields, which come later on the wire: collect them first. */
    uint64_t addresses[512];
    char symbols[64][160];
    uint32_t lines[512];
    int named[512];
    char files[64][256];
    if (frames > 512) frames = 512;
    for (uint32_t i = 0; i < frames && !c.bad; i++) {
        addresses[i] = rd_u64(&c);
        named[i] = 0;
        lines[i] = 0;
        char symbol[160] = "", file[256] = "";
        if (rd_u8(&c)) rd_str(&c, symbol, sizeof symbol);
        if (rd_u8(&c)) rd_str(&c, file, sizeof file);
        if (rd_u8(&c)) lines[i] = rd_u32(&c);
        if (i < 64) {
            strncpy(symbols[i], symbol, 159);
            symbols[i][159] = 0;
            strncpy(files[i], file, 255);
            files[i][255] = 0;
        }
        named[i] = symbol[0] != 0;
    }
    rd_str(&c, ns, sizeof ns);
    rd_str(&c, version, sizeof version);
    uint64_t schema = rd_u64(&c);
    rd_str(&c, image, sizeof image);
    pthread_mutex_lock(&out_lock);
    if (c.bad || c.at != n) {
        printf("BADREPORT bytes=%u read=%u\n", n, c.at);
    } else {
        printf("REPORT operation=%s thread=%s namespace=%s version=%s schema=0x%016llx image=%s\n", operation,
               thread, ns, version, (unsigned long long)schema, image);
        printf("LOCATION %s\n", location);
        printf("MESSAGE %s\n", message);
        for (uint32_t i = 0; i < frames; i++) {
            printf("FRAME 0x%llx\n", (unsigned long long)addresses[i]);
            if (named[i] && i < 64) printf("FRAMESYM %s %s %u\n", symbols[i], files[i], lines[i]);
        }
    }
    fflush(stdout);
    pthread_mutex_unlock(&out_lock);
    reports++;
}

static void on_reply(void *user, uint32_t call_id, const uint8_t *p, uint32_t n) {
    (void)user; (void)call_id;
    if (n >= 5) last_status = p[4];
    replies++;
}
static void on_changes(void *user, const uint8_t *p, uint32_t n) { (void)user; (void)p; (void)n; }
static void on_stream(void *user, uint32_t call_id, const uint8_t *p, uint32_t n) {
    (void)user; (void)call_id; (void)p; (void)n;
}

/* The Diagnostics port (and nothing else): fire and forget, answered with an empty PortReply. */
static uint8_t on_port(void *user, uint32_t port, uint32_t method, uint32_t call, const uint8_t *args,
                       uint32_t n, UndraBuf *out) {
    (void)user;
    if (port != PORT_DIAGNOSTICS || method != DIAGNOSTICS_PANICKED) return 2;
    print_report(args, n);
    uint8_t *reply = malloc(5);
    memset(reply, 0, 5);
    memcpy(reply, &call, 4);
    out->ptr = reply;
    out->len = 5;
    out->cap = 0;
    return 0;
}

static uint32_t fnv1a32(const char *s) {
    uint32_t h = 0x811c9dc5u;
    for (; *s; s++) {
        h ^= (uint8_t)*s;
        h *= 0x01000193u;
    }
    return h;
}

/* A `Call` payload for a free function (SPEC 3.3): target 0, handle 0, method id, call id, args. */
static uint32_t call_payload(uint8_t *out, const char *function, uint32_t call_id, const uint8_t *args,
                             uint32_t args_len) {
    char name[160];
    snprintf(name, sizeof name, "fn.%s", function);
    uint32_t method = fnv1a32(name);
    memset(out, 0, 9);
    memcpy(out + 9, &method, 4);
    memcpy(out + 13, &call_id, 4);
    memcpy(out + 17, args, args_len);
    return 17 + args_len;
}

/* RuntimeConfig: platform String, mode String, core_threads u8, blocking_threads u8, log_level u8. */
static const uint8_t CFG[] = {1, 0, 0, 0, 'c', 6, 0, 0, 0, 'i', 'n', 'p', 'r', 'o', 'c', 1, 1, 2};

int main(int argc, char **argv) {
    const char *scenario = argc > 1 ? argv[1] : "explode";
    /* Under a debugger test: stop here until it attaches (the process is then resumed by the debugger). */
    if (getenv("UNDRA_HARNESS_WAIT") != NULL) raise(SIGSTOP);
    const UndraApi *api = (const UndraApi *)CAT(UNDRA_NS, _undra_api)();
    if (api == NULL || api->abi_version != UNDRA_ABI_VERSION) {
        printf("NOAPI\n");
        return 1;
    }
    Dl_info info;
    if (dladdr((const void *)CAT(UNDRA_NS, _undra_api), &info) && info.dli_fbase) {
        printf("BASE 0x%llx\n", (unsigned long long)(uintptr_t)info.dli_fbase);
    }
    fflush(stdout);
    if (api->init(CFG, sizeof CFG, on_reply, on_changes, on_stream, NULL) != 0) {
        printf("INITFAILED\n");
        return 1;
    }
    api->port_register(PORT_DIAGNOSTICS, on_port, NULL);

    uint8_t call[512];
    uint8_t args[256];
    if (strcmp(scenario, "explode") == 0 || strcmp(scenario, "detached") == 0) {
        const char *reason = strcmp(scenario, "explode") == 0 ? "kaboom" : "task";
        uint32_t len = (uint32_t)strlen(reason);
        memcpy(args, &len, 4);
        memcpy(args + 4, reason, len);
        if (strcmp(scenario, "explode") == 0) {
            uint32_t n = call_payload(call, "explode", 7, args, 4 + len);
            UndraBuf reply = api->call_sync(call, n);
            printf("REPLY status=%d\n", reply.len >= 5 ? reply.ptr[4] : -1);
            api->buf_free(reply);
        } else {
            uint32_t n = call_payload(call, "explode_detached", 8, args, 4 + len);
            int status = api->call(call, n);
            printf("SUBMIT status=%d\n", status);
        }
    } else {
        /* add(40, 2): `a i32, b i32`. */
        int32_t a = 40, b = 2;
        memcpy(args, &a, 4);
        memcpy(args + 4, &b, 4);
        uint32_t n = call_payload(call, "add", 9, args, 8);
        UndraBuf reply = api->call_sync(call, n);
        int32_t sum = 0;
        if (reply.len >= 9) memcpy(&sum, reply.ptr + 5, 4);
        printf("REPLY status=%d sum=%d\n", reply.len >= 5 ? reply.ptr[4] : -1, sum);
        api->buf_free(reply);
    }
    fflush(stdout);
    /* A report from a task of its own arrives on the core's thread: wait for it (10 s at most). */
    for (int i = 0; i < 1000 && strcmp(scenario, "add") != 0 && reports == 0; i++) usleep(10000);
    printf("DONE reports=%d\n", reports);
    fflush(stdout);
    api->shutdown();
    return 0;
}
