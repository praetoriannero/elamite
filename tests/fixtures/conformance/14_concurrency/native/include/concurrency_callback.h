#ifndef ELAMITE_CONCURRENCY_CALLBACK_H
#define ELAMITE_CONCURRENCY_CALLBACK_H

static int elamite_conformance_callback(int (*callback)(int), int value) {
    return callback(value);
}

#endif
