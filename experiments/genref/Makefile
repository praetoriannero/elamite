CC ?= cc
CFLAGS ?= -std=c99 -O2 -g -Wall -Wextra -Wpedantic -Wshadow \
          -Wstrict-prototypes -Wmissing-prototypes -Wconversion
SANFLAGS = -fsanitize=address,undefined -fno-omit-frame-pointer

.PHONY: all test sanitize example clean

all: test example

test: build/test_genref
	./build/test_genref

sanitize: build/test_genref_san
	./build/test_genref_san

example: build/graph_demo
	./build/graph_demo

build:
	mkdir -p build

build/test_genref: genref.c genref.h test_genref.c | build
	$(CC) $(CFLAGS) genref.c test_genref.c -o $@

build/test_genref_san: genref.c genref.h test_genref.c | build
	$(CC) $(CFLAGS) $(SANFLAGS) genref.c test_genref.c -o $@

build/graph_demo: genref.c genref.h graph_demo.c | build
	$(CC) $(CFLAGS) genref.c graph_demo.c -o $@

clean:
	rm -rf build
