// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The C constructs no other example has (#168): a variable-length array and
// an alloca (dynamic stack frames), goto (a cleanup path, a backward retry
// and a jump into a loop body), a union, a bitfield struct, a variadic function, and a macro that
// expands to an `if` and a `return`.
//
// Each is small and does something observable, so the instrumented run
// covers some paths and leaves others: main never takes a goto to a
// cleanup label or into the loop, never makes the macro return, and never
// lets the retry run out of attempts.
#include <alloca.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

/* ---- dynamic stack frames ------------------------------------------- */

/* A variable-length array: the frame grows by n ints at run time. */
int vla_sum(int n) {
    int buf[n];
    for (int i = 0; i < n; i++) {
        buf[i] = i * 3;
    }
    int s = 0;
    for (int i = 0; i < n; i++) {
        s += buf[i];
    }
    return s;
}

/* alloca: the same, through the library call the compiler builds in. */
int alloca_fill(int n) {
    char *p = alloca((size_t)n);
    memset(p, 'a', (size_t)n);
    return p[n - 1];
}

/* A fixed frame that calls a dynamic one: its own frame is static, but its
   worst case is not bounded either. */
int calls_vla(int n) {
    int local[4] = {1, 2, 3, 4};
    return local[n & 3] + vla_sum(n);
}

/* ---- goto ------------------------------------------------------------- */

static int resource_a;
static int resource_b;

/* The C idiom: one exit path that undoes what was done, reached by goto
   from each step that can fail. main never makes a step fail. */
int goto_cleanup(int fail_at) {
    int rc = -1;
    resource_a = 1;
    if (fail_at == 1) {
        goto undo_a;
    }
    resource_b = 1;
    if (fail_at == 2) {
        goto undo_b;
    }
    rc = 0;
    return rc;
undo_b:
    resource_b = 0;
undo_a:
    resource_a = 0;
    return rc;
}

/* A backward goto: a retry loop written without a loop statement. */
int goto_retry(int succeed_on) {
    int attempt = 0;
again:
    attempt++;
    if (attempt < succeed_on && attempt < 5) {
        goto again;
    }
    return attempt;
}

/* A goto into the middle of a loop body: the loop then has two entries, and
   its control flow is irreducible. main enters it the ordinary way. */
int goto_into_loop(int n) {
    int i = 0;
    int s = 0;
    if (n > 3) {
        goto inside;
    }
    while (i < n) {
        s += i;
    inside:
        i++;
    }
    return s;
}

/* ---- a union ---------------------------------------------------------- */

union word {
    uint32_t u;
    float f;
    uint8_t bytes[4];
};

union word shared_word;

/* Write the union as one member and read it back as another. */
uint32_t float_bits(float f) {
    shared_word.f = f;
    return shared_word.u;
}

uint8_t low_byte(uint32_t v) {
    shared_word.u = v;
    return shared_word.bytes[0];
}

/* ---- a bitfield struct ------------------------------------------------ */

struct status {
    unsigned mode : 3;
    unsigned armed : 1;
    unsigned count : 4;
    unsigned spare : 24;
};

struct status unit_status;

/* Each field is a few bits of one word: a write to `armed` is a
   read-modify-write of the whole word. */
void arm(unsigned mode) {
    unit_status.mode = mode;
    unit_status.armed = 1;
    if (unit_status.count < 15) {
        unit_status.count++;
    }
}

int is_armed(void) {
    return unit_status.armed;
}

/* The same two types as locals, for the variables panel: a bitfield struct
   and a union on the stack. */
unsigned pack_local(unsigned mode) {
    struct status s = {0};
    s.mode = mode;
    s.armed = 1;
    union word w;
    w.f = 2.0f;
    return s.mode + s.armed + (w.u >> 24);
}

/* ---- a variadic function --------------------------------------------- */

/* Sums `count` int arguments. */
int sum_ints(int count, ...) {
    va_list ap;
    va_start(ap, count);
    int s = 0;
    for (int i = 0; i < count; i++) {
        s += va_arg(ap, int);
    }
    va_end(ap);
    return s;
}

/* ---- a macro that expands to control flow ---------------------------- */

/* Wrapped in a plain block, not the usual `do { ... } while (0)`: newer clang
   releases emit a coverage branch region for the `while (0)` condition, so the
   same source gave a different branch-outcome total on clang 19 and clang 21.
   A plain block is exact here -- REQUIRE is only ever a statement of its own,
   never an arm of an `if` -- and keeps the count portable across compilers. */
#define REQUIRE(cond)                                                      \
    {                                                                      \
        if (!(cond)) {                                                     \
            return -1;                                                     \
        }                                                                  \
    }

/* Three checks written as macro uses. main passes values that satisfy all
   three, so no macro ever returns. */
int checked_scale(int v, int lo, int hi) {
    REQUIRE(lo <= hi);
    REQUIRE(v >= lo);
    REQUIRE(v <= hi);
    return v * 2;
}

/* A decision of two conditions written inside a macro: llvm-cov records it,
   and its MC/DC conditions, at the #define (#269). main passes one value in
   range, so neither condition is shown to matter on its own. */
#define IN_RANGE(v, lo, hi) ((v) >= (lo) && (v) <= (hi))

int in_range(int v) {
    int r = 0;
    if (IN_RANGE(v, 0, 10)) {
        r = 1;
    }
    return r;
}

int main(void) {
    int acc = 0;
    acc += vla_sum(4);
    acc += alloca_fill(8);
    acc += calls_vla(3);
    acc += goto_cleanup(0);
    acc += goto_retry(3);
    acc += goto_into_loop(2);
    acc += (int)(float_bits(1.0f) >> 24);
    acc += low_byte(0x11223344u);
    arm(5);
    acc += is_armed();
    acc += (int)pack_local(3);
    acc += sum_ints(3, 10, 20, 30);
    acc += checked_scale(4, 0, 10);
    acc += in_range(4);
    printf("%d\n", acc);
    return 0;
}
