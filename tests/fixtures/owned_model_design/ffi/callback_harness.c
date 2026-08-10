#include <stdint.h>

extern int32_t owned_model_callback(int32_t *context);

int main(void) {
    int32_t value = 41;
    return owned_model_callback(&value) == 42 ? 0 : 1;
}
