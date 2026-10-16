/* SPDX-License-Identifier: GPL-3.0-only */
/* Test fixture: an azmod.so whose constructor aborts inside EP147 only. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
__attribute__((constructor)) static void crash_in_ep147(void) {
    char comm[32] = {0};
    FILE *f = fopen("/proc/self/comm", "r");
    if (f) { if (!fgets(comm, sizeof comm, f)) comm[0] = 0; fclose(f); }
    if (strncmp(comm, "EP147", 5) == 0) abort();
}
