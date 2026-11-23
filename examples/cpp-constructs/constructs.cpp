// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

// The everyday C++ the other examples avoid (#166):
//   * a class with a user-written constructor and destructor, and a
//     derived class with a virtual one  -> several object symbols per
//                                          source function (C1/C2, D0/D1/D2)
//   * overloaded operators, free and member
//   * lambdas, capturing and not, called directly, through a template and
//     through a function pointer
//   * new / delete, scalar and array
//   * an object leaving scope              -> a destructor call with no
//                                            source statement of its own
//
// Built without exceptions or RTTI, like examples/cpp: those are #165.

#include <cstdint>
#include <cstdio>

namespace fc {

volatile int32_t sink;

// A buffer owned by its channel: new[] in the constructor, delete[] in the
// destructor. The virtual destructor gives the class a deleting destructor.
class Channel {
public:
    explicit Channel(int32_t id);
    virtual ~Channel();
    int32_t &operator[](int32_t i);
    virtual int32_t gain() const { return 1; }
    // An inline member: a lambda in an inline function is mangled with a
    // name of its own ('lambda'(int, int)), not the $_N of a non-inline one.
    int32_t peak() const {
        auto larger = [](int32_t a, int32_t b) { return a > b ? a : b; };
        int32_t best = samples_[0];
        for (int32_t i = 1; i < 4; i++) {
            best = larger(best, samples_[i]);
        }
        return best;
    }

protected:
    int32_t id_;
    int32_t *samples_;
};

/* @satisfies LLR-CTOR-001 */
Channel::Channel(int32_t id) : id_(id), samples_(new int32_t[4]) {
    for (int32_t i = 0; i < 4; i++) {
        samples_[i] = 0;
    }
}

Channel::~Channel() {
    delete[] samples_;
    sink = id_;
}

/* @satisfies LLR-OPS-003 : bounds-checked element access */
int32_t &Channel::operator[](int32_t i) {
    if (i < 0 || i > 3) {
        return samples_[0];
    }
    return samples_[i];
}

class Boosted : public Channel {
public:
    Boosted(int32_t id, int32_t factor) : Channel(id), factor_(factor) {}
    ~Boosted() override { sink = factor_; }
    int32_t gain() const override { return factor_; }

private:
    int32_t factor_;
};

struct Vec2 {
    int32_t x;
    int32_t y;
    Vec2 &operator+=(const Vec2 &o) {
        x += o.x;
        y += o.y;
        return *this;
    }
};

/* @satisfies LLR-OPS-001 */
Vec2 operator+(Vec2 a, const Vec2 &b) {
    a += b;
    return a;
}

/* @satisfies LLR-OPS-002 : one decision, two conditions */
bool operator==(const Vec2 &a, const Vec2 &b) {
    return a.x == b.x && a.y == b.y;
}

template <typename F>
int32_t count_if(const int32_t *v, int32_t n, F pred) {
    int32_t hits = 0;
    for (int32_t i = 0; i < n; i++) {
        if (pred(v[i])) {
            hits++;
        }
    }
    return hits;
}

/* @satisfies LLR-LAM-001 */
int32_t lambdas(const int32_t *v, int32_t n, int32_t limit) {
    // capturing by value, called through a template
    int32_t above = count_if(v, n, [limit](int32_t x) { return x > limit; });
    // not capturing, converted to a function pointer and called through it
    int32_t (*twice)(int32_t) = [](int32_t x) { return 2 * x; };
    // capturing by reference, called directly
    int32_t total = 0;
    auto add = [&total](int32_t x) {
        if (x < 0) {
            return;
        }
        total += x;
    };
    for (int32_t i = 0; i < n; i++) {
        add(v[i]);
    }
    return above + twice(total);
}

/* @satisfies LLR-MEM-001 */
int32_t lifetimes(int32_t id) {
    // scalar new/delete through a base pointer: the deleting destructor
    Channel *c = new Boosted(id, 3);
    (*c)[1] = c->gain();
    int32_t g = (*c)[1];
    delete c;
    // an object on the stack: its destructor runs at the closing brace
    {
        Channel local(id + 1);
        local[2] = 5;
        g += local[2] + local.peak();
    }
    return g;
}

} // namespace fc

int main() {
    const int32_t v[] = {1, 5, 9, -2};
    fc::Vec2 a{1, 2};
    fc::Vec2 b{3, 4};
    fc::Vec2 c = a + b;
    bool same = (c == fc::Vec2{4, 6});
    int32_t l = fc::lambdas(v, 4, 4);
    int32_t m = fc::lifetimes(7);
    std::printf("%d %d %d\n", same ? 1 : 0, static_cast<int>(l), static_cast<int>(m));
    return 0;
}
