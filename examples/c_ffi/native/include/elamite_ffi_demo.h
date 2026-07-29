#ifndef ELAMITE_FFI_DEMO_H
#define ELAMITE_FFI_DEMO_H

#include <stdint.h>

typedef struct ffi_pair {
    int32_t left;
    int32_t right;
} ffi_pair;

typedef struct ffi_counter ffi_counter;

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
);

ffi_pair ffi_make_pair(int32_t left, int32_t right);
ffi_pair ffi_add_pairs(ffi_pair first, ffi_pair second);

ffi_counter *ffi_counter_open(int32_t initial_value);
void ffi_counter_add(ffi_counter *counter, int32_t amount);
int32_t ffi_counter_read(const ffi_counter *counter);
void ffi_counter_reset(ffi_counter *counter);

int32_t ffi_apply_callback(int32_t (*callback)(int32_t), int32_t value);
int32_t ffi_call_elamite_export(int32_t value);

#endif
