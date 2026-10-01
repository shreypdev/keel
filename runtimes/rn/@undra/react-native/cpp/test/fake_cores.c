// Tables a real core never exports, for the refusals of host_test.cpp (testsOfTheTable). Built into
// one library that run.sh links under each name (`lib<namespace>`) for the dlopen shim, and linked
// into the test with `fake_classes.m` for the linked shim. Each exports `<namespace>_undra_api`.
#include <stddef.h>
#include <stdint.h>

#include "../undra.h"

/* Version 1 had no table: the "table" is 4 bytes holding 1, so a host that read past `abi_version`
 * would overflow it (AddressSanitizer reports it). */
static const uint32_t bad_abi_table = 1;

/* C ABI 2, but a `size` from before the last field. */
static const UndraApi short_table = {
    .abi_version = UNDRA_ABI_VERSION,
    .size = (uint32_t)offsetof(UndraApi, buf_free),
    .schema_hash = 1,
    .name_space = "short_table",
};

/* A whole table that says it is another core. */
static const UndraApi wrong_ns = {
    .abi_version = UNDRA_ABI_VERSION,
    .size = (uint32_t)sizeof(UndraApi),
    .schema_hash = 1,
    .name_space = "other_core",
};

/* A whole table of the right core, without its entries. */
static const UndraApi null_entry = {
    .abi_version = UNDRA_ABI_VERSION,
    .size = (uint32_t)sizeof(UndraApi),
    .schema_hash = 1,
    .name_space = "null_entry",
};

const void *bad_abi_undra_api(void) { return &bad_abi_table; }
const void *short_table_undra_api(void) { return &short_table; }
const void *wrong_ns_undra_api(void) { return &wrong_ns; }
const void *null_entry_undra_api(void) { return &null_entry; }
/* `libnot_a_core` exports no `not_a_core_undra_api`. */
