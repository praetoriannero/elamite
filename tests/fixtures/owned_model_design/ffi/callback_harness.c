#include <pthread.h>
#include <stdint.h>

extern int32_t owned_model_callback(int32_t *context);

struct callback_job {
    int32_t value;
    int32_t result;
};

static void *invoke_callback(void *opaque) {
    struct callback_job *job = (struct callback_job *)opaque;
    job->result = owned_model_callback(&job->value);
    return NULL;
}

int main(void) {
    pthread_t thread;
    struct callback_job job = {41, 0};
    if (pthread_create(&thread, NULL, invoke_callback, &job) != 0) return 2;
    if (pthread_join(thread, NULL) != 0) return 3;
    return job.result == 42 && job.value == 42 ? 0 : 1;
}
