/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj
 *
 * The same shape as examples/c, built with GCC instead of clang, so the
 * gcov front end is exercised on real GCC output rather than a fixture
 * somebody wrote by hand.
 *
 * No headers: this must build with a GCC that has no macOS SDK wired up,
 * which is the ordinary state of a Homebrew GCC on a developer's Mac.
 */

/* @satisfies LLR-GCC-001 : altitude is held inside the envelope */
int clamp_alt(int v, int lo, int hi) {
    if (v < lo) return lo;
    if (v > hi) return hi;
    return v;
}

/* @satisfies LLR-GCC-002 : engagement needs gear and speed, or weight-on-wheels
 *
 * A short-circuit decision, so gcov reports three conditions on one line --
 * and `wow` is never evaluated when the left side already decided it. That
 * is the case that shows condition coverage is not MC/DC.
 */
int mode_ok(int gear, int speed, int wow) {
    return (gear && speed) || wow;
}

/* @satisfies LLR-GCC-003 : a loop, whose branch direction gcov reports
 * inverted relative to an `if` -- the reason butterfly does not name the
 * outcomes of a gcov decision `true` and `false`.
 */
int sum_to(int n) {
    int s = 0;
    for (int i = 0; i < n; i++) s += i;
    return s;
}

/* Deliberately never called: an uncovered function is a finding the example
 * should contain, not an accident. */
int unreachable_helper(int x) { return x * 2; }

/* Defined in util.c: the second translation unit, whose functions a reader
   that mis-indexed the export's file list used to drop silently. */
int rate_ok(int rate, int max_rate);

int main(void) {
    int a = clamp_alt(5, 0, 10);    /* inside  */
    int b = clamp_alt(-3, 0, 10);   /* below   */
    int c = mode_ok(1, 1, 0);       /* wow never evaluated */
    int d = sum_to(3);
    int e = rate_ok(10, 100);       /* the other translation unit */
    return (a + b + c + d + e) ? 0 : 0;
}
