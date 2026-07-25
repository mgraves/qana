/* demo.c — served by the C-subset grammar in c.rg.
   Block comments, // line comments, directives-as-syntax, structs,
   enums, nested declarators, and the full expression grammar. */

#include <stdio.h>
#define UNUSED_SENTINEL 1

enum { LIMIT = 100 };

enum color { RED, GREEN = 5, BLUE };

struct point {
    int x;
    int y;
    struct point *next;
};

static int scale(int v, int factor);
int apply(int (*op)(int x, int y), int a, int b);

static int scale(int v, int factor) {
    int result = v * factor + 1;
    if (result > LIMIT)
        result = LIMIT;
    else
        result = result % LIMIT;
    return result;
}

int apply(int (*op)(int x, int y), int a, int b) {
    return op(a, b);
}

int main(void) {
    struct point p;
    int total = 0;
    unsigned long big = 0xFFul;
    double ratio = 1.5e2;
    char *msg = "hello, world";
    int i;

    p.x = 3;
    p.y = 4;
    p.next = &p;

    for (i = 0; i < 10; ++i) {
        total += scale(i, 2) + p.next->x;
        while (total > 50 && total < 90)
            total -= 7;
    }

    do {
        total = total << 1 ^ (total & 0xF);
    } while (!(total >= LIMIT));

    return total == 0 ? -1 : ~total + sizeof big;
}
