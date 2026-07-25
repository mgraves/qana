/* demo.c — served by the C-subset grammar in c.rg.
   Block comments, // line comments, directives-as-syntax, structs,
   enums, nested declarators, and the full expression grammar —
   including the wall-3 doors: pointered typedef locals, casts,
   sizeof(type), and the comma operator. */

#include <stdio.h>
#define LIMIT 100
#define UNUSED_FLAG 1

#ifdef LIMIT
#endif

enum color { RED, GREEN = 5, BLUE };


typedef unsigned long word_t;

/* The tag is FORWARD-declarable (wall 4): this typedef references
   `struct point` before the definition below — C's tag namespace is
   order-free while values stay define-before-use. */
typedef struct point point_t;

struct point {
    int x;
    int y;
    unsigned flags : 4;
    word_t weight;
    struct point *next;
};

word_t global_words = 0;
point_t *head_node;

word_t twice(word_t w) {
    word_t doubled = w + w;
    return doubled;
}

int measure(word_t, point_t *stray);

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

static unsigned long checksum(const struct point *pt) {
    point_t *alias = head_node;           /* pointered typedef local  */
    word_t *cursor = &global_words, salt = 3;
    unsigned long bits = (unsigned long)pt->y;      /* keyword cast   */
    word_t widened = (word_t)(pt->x);     /* bare-name cast: call-shaped */
    unsigned span = sizeof(struct point *) + sizeof(unsigned long);
    unsigned width = sizeof(word_t);      /* bare name: sizeof a VALUE */
    int i, steps;

    for (i = 0, steps = 0; i < 4; ++i, ++steps)     /* comma clauses  */
        bits = bits << 1 ^ (unsigned long)i;
    bits = bits + span, width = width + steps;      /* comma operator */
    salt = *cursor + (word_t)(bits);
    return alias == 0 ? salt + width : salt * widened;
}

static int drain(int n) {
    int acc = 0;
retry:                        /* wall 5: labels bind (hoisted ns)   */
    if (n > 0) {
        acc += n;
        n -= 2;
        goto retry;           /* backward goto                      */
    }
    if (acc > LIMIT)
        goto done;            /* FORWARD goto — resolves hoisted    */
    acc = acc % LIMIT;
done:
    return acc;
}

int main(void) {
    struct point p;
    int total = 0;
    unsigned long big = 0xFFul;
    word_t local_words = twice(3);
    double ratio = 1.5e2;
    char *msg = "hello, world";
    unsigned long check = 0;
    int i;

    struct point q = { .x = 1, .y = 2 };
    p.x = 3;
    p.y = 4;
    switch (p.x) {
        case 1: total = q.x; break;
        default: total = q.y; break;
    }
    p.next = &p;

    for (i = 0; i < 10; ++i) {
        total += scale(i, 2) + p.next->x;
        while (total > 50 && total < 90)
            total -= 7;
    }

    do {
        total = total << 1 ^ (total & 0xF);
    } while (!(total >= LIMIT));

    check = checksum(&p);
    total ^= (int)check;
    total += drain(9);

    return total == 0 ? -1 : ~total + sizeof big;
}
