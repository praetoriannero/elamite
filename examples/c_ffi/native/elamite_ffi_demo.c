#include "include/elamite_ffi_demo.h"

struct ffi_counter {
    int32_t value;
};

/* Defined in Elamite with @exportc. */
extern int32_t elamite_triple(int32_t value);

int64_t ffi_scalar_checksum(
    int8_t i8_value,
    int16_t i16_value,
    int32_t i32_value,
    int64_t i64_value,
    uint8_t u8_value,
    uint16_t u16_value,
    uint32_t u32_value,
    uint64_t u64_value,
    intptr_t isize_value,
    uintptr_t usize_value,
    float f32_value,
    double f64_value
) {
    return (int64_t)i8_value
        + (int64_t)i16_value
        + (int64_t)i32_value
        + i64_value
        + (int64_t)u8_value
        + (int64_t)u16_value
        + (int64_t)u32_value
        + (int64_t)u64_value
        + (int64_t)isize_value
        + (int64_t)usize_value
        + (int64_t)f32_value
        + (int64_t)f64_value;
}

ffi_pair ffi_make_pair(int32_t left, int32_t right) {
    ffi_pair result = {left, right};
    return result;
}

ffi_pair ffi_add_pairs(ffi_pair first, ffi_pair second) {
    ffi_pair result = {
        first.left + second.left,
        first.right + second.right
    };
    return result;
}

ffi_counter *ffi_counter_open(int32_t initial_value) {
    static ffi_counter counter;
    counter.value = initial_value;
    return &counter;
}

void ffi_counter_add(ffi_counter *counter, int32_t amount) {
    counter->value += amount;
}

int32_t ffi_counter_read(const ffi_counter *counter) {
    return counter->value;
}

void ffi_counter_reset(ffi_counter *counter) {
    counter->value = 0;
}

int32_t ffi_apply_callback(int32_t (*callback)(int32_t), int32_t value) {
    return callback(value) + 5;
}

int32_t ffi_call_elamite_export(int32_t value) {
    return elamite_triple(value) + 1;
}
