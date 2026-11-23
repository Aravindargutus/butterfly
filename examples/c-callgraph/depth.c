// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// A call graph with something to expand.
//
// Every other C example here is flat: examples/c is `main` calling nine
// leaves, so the Calls tab has exactly one hop to draw and expansion has
// nothing to open. That makes it useless for exercising the expandable call
// graph (#100) -- and worse, a test written against it passes while
// testing nothing.
//
// This one is deliberately shaped so that every node state the graph can
// draw actually occurs:
//
//   depth          main -> stage_one -> stage_two -> stage_three -> leaf
//   recursion      countdown calls itself
//   mutual         ping and pong call each other
//   indirect       dispatch calls through a function pointer
//   external       a call out of the covered source set
//
// The arithmetic is uninteresting on purpose. The shape is the point.

#include <stdio.h>
#include <string.h>

/* ---- a chain deep enough that one expansion is not the whole story ---- */

static int leaf(int v) {
    return v + 1;
}

static int stage_three(int v) {
    return leaf(v) * 2;
}

static int stage_two(int v) {
    return stage_three(v) + stage_three(v + 1);
}

static int stage_one(int v) {
    return stage_two(v) - 1;
}

/* ---- direct recursion: the graph must link back, not redraw ---- */

static int countdown(int n) {
    if (n <= 0) {
        return 0;
    }
    return n + countdown(n - 1);
}

/* ---- mutual recursion: the cycle spans two functions ---- */

static int pong(int n);

static int ping(int n) {
    if (n <= 0) {
        return 0;
    }
    return 1 + pong(n - 1);
}

static int pong(int n) {
    if (n <= 0) {
        return 0;
    }
    return 1 + ping(n - 1);
}

/* ---- an indirect call: a candidate target is a hypothesis, not a fact ---- */

typedef int (*op_fn)(int);

static int op_double(int v) {
    return v * 2;
}

static int op_negate(int v) {
    return -v;
}

// At file scope and const, so the addresses sit in a data section the
// address-forming analysis can find. Built on the stack inside the function
// instead, -O0 materialises them through the frame and the evidence records
// an indirect call site with no candidate targets at all -- which is honest,
// but leaves nothing for the graph to mark as a hypothesis.
static const op_fn OPS[2] = { op_double, op_negate };

static int dispatch(int which, int v) {
    return OPS[which & 1](v);
}

/* ---- an indirect call nothing can resolve: the pointer arrives as a
   parameter, so no address is formed here and no table is read here --
   the candidates all belong to the callers. The evidence records this
   site with no targets: missing evidence, not a leaf, and the graph must
   keep the site visible rather than drawing a function that calls
   nothing (review of #101). ---- */

static int apply(op_fn op, int v) {
    return op(v);
}

/* ---- a call that leaves the covered source set entirely ---- */

static int copy_len(const char *s) {
    char buf[32];
    strncpy(buf, s, sizeof buf - 1);
    buf[sizeof buf - 1] = '\0';
    return (int) strlen(buf);
}

int main(void) {
    int acc = 0;
    acc += stage_one(3);
    acc += countdown(4);
    acc += ping(3);
    acc += dispatch(0, 5);
    acc += dispatch(1, 5);
    acc += apply(op_double, 7);
    acc += apply(op_negate, 7);
    acc += copy_len("butterfly");
    printf("acc=%d\n", acc);
    return 0;
}
