#include <stdio.h>


typedef enum {
    CASE_FOO,
    CASE_BAR,
} BazTag;

typedef union {
    short Foo;
    short Bar;
} BazValues;

typedef struct {
    BazTag tag;
    BazValues inner;
} BazVariant;

BazVariant elx__create_case_BazVariant_Foo() {
    BazVariant result;
    result.tag = CASE_FOO;
    result.inner.Foo = 1;
    return result;
}

int main() {
    BazVariant baz = elx__create_case_BazVariant_Foo();
    printf("baz(case=%d, %d)\n", baz.tag, baz.inner.Foo);

    return 0;
}
