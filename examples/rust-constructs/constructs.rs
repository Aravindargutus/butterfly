// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! butterfly Rust fixture: what makes Rust different from C (#167).
//! A generic function instantiated at two types, a trait called through
//! `dyn`, closures that capture, and a `macro_rules!` macro that expands to
//! a decision.
#![allow(dead_code)]

// ─────────────────────────────────────────────── generics

/// One source function, two monomorphised copies in the image.
#[inline(never)]
pub fn limit<T: PartialOrd + Copy>(v: T, lo: T, hi: T) -> T {
    if v < lo {
        return lo;
    }
    if v > hi {
        return hi;
    }
    v
}

// ─────────────────────────────────────────────── dyn dispatch

pub trait Law {
    fn apply(&self, x: i32) -> i32;
}

pub struct Gain(pub i32);
pub struct Deadband(pub i32);
/// Implements the trait, and no value of it is ever built.
pub struct Bypass;

impl Law for Gain {
    #[inline(never)]
    fn apply(&self, x: i32) -> i32 {
        x.wrapping_mul(self.0)
    }
}

impl Law for Deadband {
    #[inline(never)]
    fn apply(&self, x: i32) -> i32 {
        if x.abs() < self.0 {
            0
        } else {
            x
        }
    }
}

impl Law for Bypass {
    #[inline(never)]
    fn apply(&self, x: i32) -> i32 {
        x
    }
}

/// The call through the vtable.
#[inline(never)]
pub fn run_law(law: &dyn Law, x: i32) -> i32 {
    law.apply(x)
}

// ─────────────────────────────────────────────── closures

/// A generic taking an `FnMut`: one copy per closure type passed to it.
#[inline(never)]
pub fn each_sample<F: FnMut(i32)>(samples: &[i32], mut f: F) {
    for &s in samples {
        f(s);
    }
}

/// A generic taking an `FnOnce`.
#[inline(never)]
pub fn once<F: FnOnce() -> i32>(f: F) -> i32 {
    f()
}

#[inline(never)]
pub fn closures(samples: &[i32], threshold: i32) -> (i32, i32, i32) {
    // captures `total` by mutable reference and `threshold` by reference
    let mut total = 0;
    each_sample(samples, |s| {
        if s > threshold {
            total += s;
        }
    });
    // captures `count` by mutable reference
    let mut count = 0;
    each_sample(samples, |_| count += 1);
    // captures `owned` by move, and consumes it
    let owned = vec![total, count];
    let sum = once(move || owned.into_iter().sum());
    (total, count, sum)
}

// ─────────────────────────────────────────────── macro_rules!

/// Expands to a decision at every use.
macro_rules! within {
    ($v:expr, $lo:expr, $hi:expr) => {
        if $v >= $lo && $v <= $hi {
            1
        } else {
            0
        }
    };
}

#[inline(never)]
pub fn in_envelope(alt: i32, speed: i32) -> i32 {
    let a = within!(alt, 0, 45_000);
    let s = within!(speed, 60, 350);
    a + s
}

fn main() {
    // limit::<i32> takes the low clamp only; limit::<u64> the high one only.
    println!("{}", limit(-5i32, 0, 100));
    println!("{}", limit(500u64, 0, 100));

    let laws: [&dyn Law; 2] = [&Gain(3), &Deadband(10)];
    for l in laws {
        println!("{} {}", run_law(l, 4), run_law(l, 40));
    }

    println!("{:?}", closures(&[5, 50, 500], 10));

    println!("{}", in_envelope(30_000, 200));
    println!("{}", in_envelope(-1, 20));
}
